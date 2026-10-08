use std::rc::Rc;
use std::time::{Duration, Instant};

use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use laurier::highlight::highlight_matched_text;
use once_cell::sync::Lazy;
use ratatui::{
    buffer::Buffer,
    crossterm::event::{Event, KeyEvent},
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{List, ListItem, StatefulWidget, Widget},
};
use rustc_hash::{FxHashMap, FxHashSet};
use tui_input::{backend::crossterm::EventHandler, Input, InputRequest};

use crate::{
    app::AppContext,
    color::ColorTheme,
    config::UserListColumnType,
    git::{Commit, CommitHash, Head, Ref},
    graph::GraphImageManager,
    protocol::PreparedImage,
    search::{SearchOptions, SearchTarget},
};

static FUZZY_MATCHER: Lazy<SkimMatcherV2> = Lazy::new(|| SkimMatcherV2::default().respect_case());

const ELLIPSIS: &str = "...";

/// The marker rendered in front of the ref that HEAD points to, instead of the name of HEAD.
const HEAD_MARKER: &str = "●";

/// Geometry of the scrollbar rendered on the right edge of the commit list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarGeometry {
    /// Row of the top of the thumb, relative to the top of the scrollbar.
    pub thumb_start: u16,
    /// Number of rows the thumb occupies.
    pub thumb_len: u16,
}

impl ScrollbarGeometry {
    /// Whether the given row (relative to the top of the scrollbar) is on the thumb.
    pub fn contains_thumb(&self, row: u16) -> bool {
        row >= self.thumb_start && row < self.thumb_start + self.thumb_len
    }

    /// The grab offset for a press at the given row (relative to the top of the scrollbar).
    ///
    /// When the thumb is pressed, the cursor keeps its position within the thumb; when the track
    /// is pressed, the thumb jumps to the pressed row.
    pub fn grab_offset(&self, row: u16) -> u16 {
        if self.contains_thumb(row) {
            row - self.thumb_start
        } else {
            0
        }
    }
}

/// Divides `value` by `divisor`, rounding to the nearest integer.
///
/// `divisor` must not be zero.
fn div_round(value: usize, divisor: usize) -> usize {
    (value + divisor / 2) / divisor
}

/// The maximum delay between the two clicks of a double click.
const DOUBLE_CLICK_DELAY: Duration = Duration::from_millis(500);

/// The symbol of the more button, which opens the action menu of the row it is on.
const MORE_BUTTON: &str = "…";

/// The number of columns the more button needs, which is reserved at the right edge of the list.
///
/// The symbol is drawn in a single cell, so the bar is one column wide.
const MORE_BUTTON_WIDTH: u16 = 1;

/// The style of the name of a ref: reversed, so that the badge is drawn in the color of the type of
/// the ref on the background of the terminal.
const BADGE_MODIFIER: Modifier = Modifier::BOLD.union(Modifier::REVERSED);

/// A ref that is rendered as a badge in the commit list, and where it is rendered.
///
/// The rectangle is recorded while rendering, so that a click can be matched to the badge under
/// the cursor. The ref is owned by the badge, so that the rectangles of a frame stay usable until
/// the next frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefBadge {
    /// The area of the name of the ref, in screen coordinates.
    pub rect: Rect,
    /// The ref shown by the badge.
    pub r: Ref,
}

/// A left click on a ref badge, kept to detect double clicks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadgeClick {
    /// The rectangle of the badge that was clicked.
    pub rect: Rect,
    /// When the click happened.
    pub at: Instant,
}

/// Whether `current` completes a double click started by `previous`.
///
/// The two clicks must land on the same badge, which is identified by the rectangle it was
/// rendered in, and they must be close enough in time. A click that is not on a badge is not a
/// double click.
pub fn is_double_click(previous: Option<BadgeClick>, current: Option<BadgeClick>) -> bool {
    let (Some(previous), Some(current)) = (previous, current) else {
        return false;
    };
    previous.rect == current.rect
        && current.at.saturating_duration_since(previous.at) <= DOUBLE_CLICK_DELAY
}

/// The commit under the mouse, used to render the more button of its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoveredCommit {
    /// The absolute index of the hovered commit.
    pub index: usize,
    /// Whether the mouse is resting on the more button of the row, waiting for the dwell that
    /// opens the action menu.
    pub armed: bool,
}

/// Splits the area of the columns of the list from the column of the more button bar.
///
/// The more button bar is the rightmost column of `area`, and it is reserved only when it is
/// enabled and the remaining columns still have room for a single cell, so that the button is
/// always rendered whole and never overlaps the columns on its left. The column of the bar is also
/// returned, so that it can be recorded for the hit test of the mouse.
fn split_more_button_column(area: Rect, more_button: bool) -> (Rect, Option<u16>) {
    if !more_button || area.width <= MORE_BUTTON_WIDTH {
        return (area, None);
    }
    let columns_area = Rect {
        width: area.width - MORE_BUTTON_WIDTH,
        ..area
    };
    (columns_area, Some(area.right() - MORE_BUTTON_WIDTH))
}

#[derive(Debug)]
pub struct CommitInfo<'a> {
    commit: &'a Commit,
    refs: Vec<&'a Ref>,
    graph_color: Color,
}

impl<'a> CommitInfo<'a> {
    pub fn new(commit: &'a Commit, refs: Vec<&'a Ref>, graph_color: Color) -> Self {
        Self {
            commit,
            refs,
            graph_color,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchState {
    Inactive,
    Searching {
        start_index: usize,
        match_index: usize,
    },
    Applied {
        match_index: usize,
        total_match: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRefreshContext {
    query: String,
}

impl SearchState {
    fn update_match_index(&mut self, index: usize) {
        match self {
            SearchState::Searching { match_index, .. } => *match_index = index,
            SearchState::Applied { match_index, .. } => *match_index = index,
            _ => {}
        }
    }
}

#[derive(Debug, Default, Clone)]
struct SearchMatch {
    refs: FxHashMap<String, SearchMatchPosition>,
    subject: Option<SearchMatchPosition>,
    author_name: Option<SearchMatchPosition>,
    commit_hash: Option<SearchMatchPosition>,
    match_index: usize, // 1-based
}

impl SearchMatch {
    fn set(&mut self, c: &Commit, refs: &[&Ref], matcher: &SearchMatcher, target: SearchTarget) {
        self.refs = if matches!(target, SearchTarget::All | SearchTarget::Ref) {
            refs.iter()
                .filter(|r| !matches!(*r, Ref::Stash { .. }))
                .filter_map(|r| {
                    matcher
                        .matched_position(r.name())
                        .map(|pos| (r.name().into(), pos))
                })
                .collect()
        } else {
            FxHashMap::default()
        };
        self.subject = if matches!(target, SearchTarget::All | SearchTarget::Subject) {
            matcher.matched_position(&c.subject)
        } else {
            None
        };
        self.author_name = if matches!(target, SearchTarget::All | SearchTarget::Author) {
            matcher.matched_position(&c.author_name)
        } else {
            None
        };
        self.commit_hash = if matches!(target, SearchTarget::All | SearchTarget::Hash) {
            matcher.matched_position(c.commit_hash.as_short_hash())
        } else {
            None
        };
        self.match_index = 0;
    }

    fn matched(&self) -> bool {
        !self.refs.is_empty()
            || self.subject.is_some()
            || self.author_name.is_some()
            || self.commit_hash.is_some()
    }

    fn clear(&mut self) {
        self.refs.clear();
        self.subject = None;
        self.author_name = None;
        self.commit_hash = None;
    }
}

#[derive(Debug, Default, Clone)]
struct SearchMatchPosition {
    matched_indices: Vec<usize>,
}

impl SearchMatchPosition {
    fn new(matched_indices: Vec<usize>) -> Self {
        Self { matched_indices }
    }
}

struct SearchMatcher {
    query: String,
    ignore_case: bool,
    fuzzy: bool,
}

impl SearchMatcher {
    fn new(query: &str, ignore_case: bool, fuzzy: bool) -> Self {
        let query = if ignore_case {
            query.to_lowercase()
        } else {
            query.into()
        };
        Self {
            query,
            ignore_case,
            fuzzy,
        }
    }

    fn matched_position(&self, s: &str) -> Option<SearchMatchPosition> {
        if self.fuzzy {
            let result = if self.ignore_case {
                FUZZY_MATCHER.fuzzy_indices(&s.to_lowercase(), &self.query)
            } else {
                FUZZY_MATCHER.fuzzy_indices(s, &self.query)
            };
            result
                .map(|(_, indices)| indices)
                .map(SearchMatchPosition::new)
        } else {
            let result = if self.ignore_case {
                s.to_lowercase().find(&self.query)
            } else {
                s.find(&self.query)
            };
            result
                .map(|p| (p..(p + self.query.len())).collect())
                .map(SearchMatchPosition::new)
        }
    }
}

#[derive(Debug)]
pub struct CommitListState<'a> {
    commits: Vec<CommitInfo<'a>>,
    commit_hash_set: FxHashSet<&'a CommitHash>,
    graph_image_manager: GraphImageManager<'a>,
    graph_cell_width: u16,
    head: &'a Head,

    ref_name_to_commit_index_map: FxHashMap<&'a str, usize>,

    search_state: SearchState,
    search_options: SearchOptions,
    search_input: Input,
    search_matches: Vec<SearchMatch>,

    /// The badges of the last rendered frame, used to hit test clicks.
    ref_badges: Vec<RefBadge>,

    /// The column of the more button bar of the last rendered frame, or `None` when it is not
    /// shown.
    ///
    /// Like the ref badges, it is recorded while rendering, so that the hit test of the mouse
    /// always matches what is on the screen.
    more_button_column: Option<u16>,

    selected: usize,
    offset: usize,
    total: usize,
    height: usize,
    scrolloff: usize,
}

impl<'a> CommitListState<'a> {
    pub fn new(
        commits: Vec<CommitInfo<'a>>,
        graph_image_manager: GraphImageManager<'a>,
        graph_cell_width: u16,
        head: &'a Head,
        ref_name_to_commit_index_map: FxHashMap<&'a str, usize>,
        search_options: SearchOptions,
        scrolloff: usize,
    ) -> CommitListState<'a> {
        let total = commits.len();
        let commit_hash_set = commits.iter().map(|c| &c.commit.commit_hash).collect();
        CommitListState {
            commits,
            commit_hash_set,
            graph_image_manager,
            graph_cell_width,
            head,
            ref_name_to_commit_index_map,
            search_state: SearchState::Inactive,
            search_options,
            search_input: Input::default(),
            search_matches: vec![SearchMatch::default(); total],
            ref_badges: Vec::new(),
            more_button_column: None,
            selected: 0,
            offset: 0,
            total,
            height: 0,
            scrolloff,
        }
    }

    pub fn graph_area_cell_width(&self) -> u16 {
        self.graph_cell_width + 1 // right pad
    }

    pub fn update_height(&mut self, height: usize) {
        let was_visible = self.selected_row().is_some();
        self.height = height;
        self.offset = self.offset.min(self.max_offset());

        // Keep the selection visible when it was visible before the resize
        if was_visible && self.selected_row().is_none() {
            self.select_visible_index(self.selected);
        }
    }

    pub fn ensure_visible_graph_uploaded(&mut self) {
        self.commits
            .iter()
            .skip(self.offset)
            .take(self.height)
            .for_each(|commit_info| {
                self.graph_image_manager
                    .ensure_uploaded(&commit_info.commit.commit_hash);
            });
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        self.graph_image_manager.drain_pending_uploads()
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        let mut image_ids: Vec<u32> = self
            .graph_image_manager
            .image_ids()
            .iter()
            .copied()
            .collect();
        image_ids.sort_unstable();
        image_ids
    }

    pub fn select_next(&mut self) {
        if self.total > 0 {
            self.select_visible_index((self.selected + 1).min(self.total - 1));
        }
    }

    pub fn select_parent(&mut self) {
        if let Some(target_commit) = self.selected_commit_parent_hash().cloned() {
            if self.commit_hash_set.contains(&target_commit) {
                while target_commit.as_str() != self.selected_commit_hash().as_str() {
                    self.select_next();
                }
            }
        }
    }

    pub fn selected_commit_parent_hash(&self) -> Option<&CommitHash> {
        self.commits[self.selected]
            .commit
            .parent_commit_hashes
            .first()
    }

    pub fn select_prev(&mut self) {
        self.select_visible_index(self.selected.saturating_sub(1));
    }

    pub fn select_first(&mut self) {
        self.select_visible_index(0);
    }

    pub fn select_last(&mut self) {
        self.select_visible_index(self.total.saturating_sub(1));
    }

    /// Scroll the viewport down by one row.
    ///
    /// This is a pure scroll: it does not change the selected commit, which is allowed to go
    /// out of the viewport. A wheel event scrolls exactly one row: the scrolling of the wheel is
    /// not amplified, so that the viewport moves only as far as the events it receives.
    pub fn scroll_down(&mut self) {
        self.set_offset(self.offset.saturating_add(1));
    }

    /// Scroll the viewport up by one row.
    ///
    /// This is a pure scroll: it does not change the selected commit, which is allowed to go
    /// out of the viewport. A wheel event scrolls exactly one row, as in [`Self::scroll_down`].
    pub fn scroll_up(&mut self) {
        self.set_offset(self.offset.saturating_sub(1));
    }

    /// Move the viewport to the given offset, clamped to the valid range.
    ///
    /// This is a pure scroll: it does not change the selected commit.
    pub fn set_offset(&mut self, offset: usize) {
        self.offset = offset.min(self.max_offset());
    }

    pub fn scroll_down_page(&mut self) {
        self.scroll_down_height(self.height);
    }

    pub fn scroll_up_page(&mut self) {
        self.scroll_up_height(self.height);
    }

    pub fn scroll_down_half(&mut self) {
        self.scroll_down_height(self.height / 2);
    }

    pub fn scroll_up_half(&mut self) {
        self.scroll_up_height(self.height / 2);
    }

    fn scroll_down_height(&mut self, scroll_height: usize) {
        if self.height == 0 || self.total == 0 {
            return;
        }
        let index = self
            .selected
            .saturating_add(scroll_height)
            .min(self.total - 1);
        if self.offset + self.height + scroll_height < self.total {
            // The selection follows the viewport only when it is on the screen, so that a
            // selection that the wheel or the scrollbar scrolled out of the viewport is not
            // brought back into it, which would jump the viewport backwards.
            let selection_is_visible = self.selected_row().is_some();
            self.offset += scroll_height;
            if selection_is_visible {
                self.select_visible_index(index);
            } else {
                self.selected = index;
            }
        } else {
            let size = self.height.min(self.total);
            self.offset = self.total - size;
            // Move the selection by the number of rows actually scrolled, but never past the
            // last commit, so that further page scrolls reach the last commit.
            self.selected = index;
        }
    }

    fn scroll_up_height(&mut self, scroll_height: usize) {
        if self.height == 0 || self.total == 0 {
            return;
        }
        // As in `scroll_down_height`, a selection that is out of the viewport moves with the
        // viewport instead of being brought back into it.
        let selection_is_visible = self.selected_row().is_some();
        self.offset = self.offset.saturating_sub(scroll_height);
        let index = self.selected.saturating_sub(scroll_height);
        if self.offset > 0 && selection_is_visible {
            self.select_visible_index(index);
        } else {
            self.selected = index;
        }
    }

    pub fn select_high(&mut self) {
        self.select_visible_index(self.offset);
    }

    pub fn select_middle(&mut self) {
        let row = if self.total > self.height {
            self.height / 2
        } else {
            self.total / 2
        };
        self.select_visible_index((self.offset + row).min(self.total.saturating_sub(1)));
    }

    pub fn select_low(&mut self) {
        let row = if self.total > self.height {
            self.height.saturating_sub(1)
        } else {
            self.total.saturating_sub(1)
        };
        self.select_visible_index((self.offset + row).min(self.total.saturating_sub(1)));
    }

    /// Select the commit at the given absolute index, keeping the context specified by `scrolloff`.
    fn select_index(&mut self, index: usize) {
        if index < self.total {
            self.offset = index
                .saturating_sub(self.effective_scrolloff())
                .min(self.max_offset());
            self.selected = index;
        }
    }

    fn effective_scrolloff(&self) -> usize {
        self.scrolloff.min(self.height.saturating_sub(1) / 2)
    }

    fn max_offset(&self) -> usize {
        self.total.saturating_sub(self.height)
    }

    /// Select the commit at the given absolute index, scrolling the viewport to keep it visible.
    ///
    /// `scrolloff` is respected, so the viewport is scrolled only when the commit is outside the
    /// margin, or is not visible at all.
    fn select_visible_index(&mut self, index: usize) {
        if self.total == 0 {
            return;
        }
        let index = index.min(self.total - 1);
        if self.height == 0 {
            self.select_index(index);
            return;
        }

        let margin = self.effective_scrolloff();
        let bottom = self.height - 1 - margin;
        if index < self.offset.saturating_add(margin) {
            self.offset = index.saturating_sub(margin);
        } else if index > self.offset.saturating_add(bottom) {
            self.offset = index.saturating_sub(bottom).min(self.max_offset());
        }
        self.selected = index;
    }

    pub fn select_next_match(&mut self) {
        self.select_next_match_index(self.current_selected_index());
    }

    pub fn select_prev_match(&mut self) {
        self.select_prev_match_index(self.current_selected_index());
    }

    pub fn selected_commit_hash(&self) -> &CommitHash {
        &self.commits[self.current_selected_index()]
            .commit
            .commit_hash
    }

    /// The absolute index of the selected commit.
    fn current_selected_index(&self) -> usize {
        self.selected
    }

    /// The row of the selected commit within the viewport, or `None` when it is out of the viewport.
    pub fn selected_row(&self) -> Option<usize> {
        if self.total == 0 {
            return None;
        }
        self.selected
            .checked_sub(self.offset)
            .filter(|row| *row < self.height)
    }

    /// The absolute index of the commit displayed at the given screen row, if any.
    ///
    /// Rows outside the list area, and rows without a commit, are ignored.
    pub fn commit_index_at_row(&self, list_area: Rect, row: u16) -> Option<usize> {
        if row < list_area.y || row >= list_area.bottom() {
            return None;
        }
        let index = self.offset.checked_add((row - list_area.y) as usize)?;
        (index < self.total).then_some(index)
    }

    /// Replaces the recorded ref badges, which are collected while rendering.
    pub fn set_ref_badges(&mut self, badges: Vec<RefBadge>) {
        self.ref_badges = badges;
    }

    /// The ref badge at the given screen position, if the position is on one.
    pub fn ref_badge_at(&self, column: u16, row: u16) -> Option<&RefBadge> {
        self.ref_badges
            .iter()
            .find(|badge| badge.rect.contains(Position::new(column, row)))
    }

    /// Replaces the column of the more button bar, which is recorded while rendering.
    pub fn set_more_button_column(&mut self, column: Option<u16>) {
        self.more_button_column = column;
    }

    /// The column of the more button bar of the last rendered frame, or `None` when it is not
    /// shown.
    pub fn more_button_column(&self) -> Option<u16> {
        self.more_button_column
    }

    /// The scrollbar geometry, or `None` when the scrollbar is not shown.
    ///
    /// The scrollbar is shown only when the list is longer than the viewport.
    pub fn scrollbar_geometry(&self) -> Option<ScrollbarGeometry> {
        let thumb_len = self.scrollbar_thumb_len()?;
        let max_start = self.height - thumb_len;
        let thumb_start = if max_start == 0 {
            0
        } else {
            div_round(self.offset * max_start, self.total - self.height).min(max_start)
        };
        Some(ScrollbarGeometry {
            thumb_start: thumb_start as u16,
            thumb_len: thumb_len as u16,
        })
    }

    /// The viewport offset corresponding to the given scrollbar thumb position.
    ///
    /// This is the reverse mapping of [`Self::scrollbar_geometry`], and is used for dragging and
    /// clicking the scrollbar. The result is clamped to the valid offset range.
    pub fn offset_for_thumb_start(&self, thumb_start: u16) -> usize {
        let Some(thumb_len) = self.scrollbar_thumb_len() else {
            return 0;
        };
        let max_start = self.height - thumb_len;
        if max_start == 0 {
            // The thumb occupies the whole track, so there is no position information
            return 0;
        }
        let thumb_start = (thumb_start as usize).min(max_start);
        div_round(thumb_start * (self.total - self.height), max_start).min(self.total - self.height)
    }

    /// The length of the scrollbar thumb in rows, or `None` when the scrollbar is not shown.
    fn scrollbar_thumb_len(&self) -> Option<usize> {
        if self.height == 0 || self.total <= self.height {
            return None;
        }
        Some(div_round(self.height * self.height, self.total).clamp(1, self.height))
    }

    /// Select the commit at the given absolute index, without scrolling the viewport.
    ///
    /// This is used when a row is clicked: the clicked commit is already visible.
    pub fn select_index_without_scroll(&mut self, index: usize) {
        if index < self.total {
            self.selected = index;
        }
    }

    /// The selected absolute index, the viewport offset and the viewport height.
    pub fn current_list_status(&self) -> (usize, usize, usize) {
        (self.selected, self.offset, self.height)
    }

    pub fn reset_height(&mut self, height: usize) {
        self.height = height;
    }

    /// Restore the selected absolute index and the viewport offset recorded before a refresh.
    pub fn restore_selected_index(&mut self, index: usize, offset: usize) {
        self.offset = offset.min(self.max_offset());
        self.selected = index.min(self.total.saturating_sub(1));
    }

    pub fn commit_index_of_hash(&self, commit_hash: &CommitHash) -> Option<usize> {
        if !self.commit_hash_set.contains(commit_hash) {
            return None;
        }
        self.commits
            .iter()
            .position(|commit_info| commit_info.commit.commit_hash == *commit_hash)
    }

    pub fn select_ref(&mut self, ref_name: &str) {
        if let Some(&index) = self.ref_name_to_commit_index_map.get(ref_name) {
            self.select_index(index);
        }
    }

    pub fn select_commit_hash(&mut self, commit_hash: &CommitHash) {
        if let Some(index) = self.commit_index_of_hash(commit_hash) {
            self.select_index(index);
        }
    }

    pub fn search_state(&self) -> SearchState {
        self.search_state
    }

    pub fn search_options(&self) -> SearchOptions {
        self.search_options
    }

    pub fn restore_search_options(&mut self, options: SearchOptions) {
        self.search_options = options;
    }

    pub fn start_search(&mut self) {
        let match_index = match self.search_state {
            SearchState::Inactive => {
                self.search_input.reset();
                self.clear_search_matches();
                0
            }
            SearchState::Applied { match_index, .. } => {
                self.search_input.handle(InputRequest::GoToEnd);
                match_index
            }
            SearchState::Searching { .. } => return,
        };
        self.search_state = SearchState::Searching {
            start_index: self.current_selected_index(),
            match_index,
        };
    }

    pub fn handle_search_input(&mut self, key: KeyEvent) {
        if let SearchState::Searching { start_index, .. } = self.search_state {
            self.search_input.handle_event(&Event::Key(key));
            self.update_search_matches();
            self.select_current_or_next_match_index(start_index);
        }
    }

    pub fn apply_search(&mut self) {
        if let SearchState::Searching { match_index, .. } = self.search_state {
            if self.search_input.value().is_empty() {
                self.cancel_search();
            } else {
                let total_match = self.search_matches.iter().filter(|m| m.matched()).count();
                self.search_state = SearchState::Applied {
                    match_index,
                    total_match,
                };
            }
        }
    }

    pub fn search_refresh_context(&self) -> Option<SearchRefreshContext> {
        if let SearchState::Applied { .. } = self.search_state {
            Some(SearchRefreshContext {
                query: self.search_input.value().into(),
            })
        } else {
            None
        }
    }

    pub fn restore_search(&mut self, context: &SearchRefreshContext) {
        self.search_input = Input::new(context.query.clone());
        self.update_search_matches();

        let total_match = self.search_matches.iter().filter(|m| m.matched()).count();
        self.search_state = SearchState::Applied {
            // The selected commit may not match after refresh; next/previous updates this value.
            match_index: 0,
            total_match,
        };

        if total_match > 0 {
            let current_index = self.current_selected_index();
            if self.search_matches[current_index].matched() {
                self.search_state
                    .update_match_index(self.search_matches[current_index].match_index);
            }
        }
    }

    pub fn cancel_search(&mut self) {
        if let SearchState::Searching { .. } | SearchState::Applied { .. } = self.search_state {
            self.search_state = SearchState::Inactive;
            self.search_input.reset();
            self.clear_search_matches();
        }
    }

    pub fn toggle_ignore_case(&mut self) {
        self.search_options.ignore_case = !self.search_options.ignore_case;
        self.update_search_after_options_change();
    }

    pub fn toggle_fuzzy(&mut self) {
        self.search_options.fuzzy = !self.search_options.fuzzy;
        self.update_search_after_options_change();
    }

    pub fn toggle_search_target(&mut self) {
        self.search_options.target = self.search_options.target.next();
        self.update_search_after_options_change();
    }

    pub fn search_query_string(&self) -> Option<String> {
        if let SearchState::Searching { .. } = self.search_state {
            let query = self.search_input.value();
            Some(format!("/{query}"))
        } else {
            None
        }
    }

    pub fn matched_query_string(&self) -> Option<(String, bool)> {
        if let SearchState::Applied {
            match_index,
            total_match,
            ..
        } = self.search_state
        {
            let query = self.search_input.value();
            if total_match == 0 {
                let msg = format!("No matches found (query: \"{query}\")");
                Some((msg, false))
            } else {
                let msg = format!("Match {match_index} of {total_match} (query: \"{query}\")");
                Some((msg, true))
            }
        } else {
            None
        }
    }

    pub fn search_query_cursor_position(&self) -> u16 {
        self.search_input.visual_cursor() as u16 + 1 // add 1 for "/"
    }

    fn update_search_matches(&mut self) {
        if self.search_input.value().is_empty() {
            self.clear_search_matches();
            self.search_state.update_match_index(0);
            return;
        }
        let matcher = SearchMatcher::new(
            self.search_input.value(),
            self.search_options.ignore_case,
            self.search_options.fuzzy,
        );
        let mut match_index = 1;
        for (i, commit_info) in self.commits.iter().enumerate() {
            let m = &mut self.search_matches[i];
            m.set(
                commit_info.commit,
                commit_info.refs.as_slice(),
                &matcher,
                self.search_options.target,
            );
            if m.matched() {
                m.match_index = match_index;
                match_index += 1;
            }
        }
    }

    fn update_search_after_options_change(&mut self) {
        match self.search_state {
            SearchState::Inactive => {}
            SearchState::Searching { start_index, .. } => {
                self.update_search_matches();
                self.select_current_or_next_match_index(start_index);
            }
            SearchState::Applied { .. } => {
                let current_index = self.current_selected_index();
                self.update_search_matches();
                let total_match = self.search_matches.iter().filter(|m| m.matched()).count();
                self.search_state = SearchState::Applied {
                    match_index: 0,
                    total_match,
                };
                if total_match > 0 {
                    self.select_current_or_next_match_index(current_index);
                }
            }
        }
    }

    fn clear_search_matches(&mut self) {
        self.search_matches.iter_mut().for_each(|m| m.clear());
    }

    fn select_current_or_next_match_index(&mut self, current_index: usize) {
        if self.search_matches[current_index].matched() {
            self.select_index(current_index);
            self.search_state
                .update_match_index(self.search_matches[current_index].match_index);
        } else {
            self.select_next_match_index(current_index)
        }
    }

    fn select_next_match_index(&mut self, current_index: usize) {
        let mut i = (current_index + 1) % self.total;
        while i != current_index {
            if self.search_matches[i].matched() {
                self.select_index(i);
                self.search_state
                    .update_match_index(self.search_matches[i].match_index);
                return;
            }
            if i == self.total - 1 {
                i = 0;
            } else {
                i += 1;
            }
        }
    }

    fn select_prev_match_index(&mut self, current_index: usize) {
        let mut i = (current_index + self.total - 1) % self.total;
        while i != current_index {
            if self.search_matches[i].matched() {
                self.select_index(i);
                self.search_state
                    .update_match_index(self.search_matches[i].match_index);
                return;
            }
            if i == 0 {
                i = self.total - 1;
            } else {
                i -= 1;
            }
        }
    }

    fn prepared_image(&self, commit_info: &'a CommitInfo) -> &PreparedImage {
        self.graph_image_manager
            .prepared_image(&commit_info.commit.commit_hash)
    }
}

pub struct CommitList<'a> {
    ctx: Rc<AppContext>,
    scrollbar: bool,
    /// Whether the more button bar is reserved at the right edge of the list.
    more_button: bool,
    /// The commit under the mouse, whose row shows the more button.
    hovered: Option<HoveredCommit>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> CommitList<'a> {
    pub fn new(ctx: Rc<AppContext>) -> Self {
        Self {
            ctx,
            scrollbar: false,
            more_button: false,
            hovered: None,
            _marker: std::marker::PhantomData,
        }
    }

    /// Render the scrollbar on the right edge of the list.
    pub fn scrollbar(mut self, scrollbar: bool) -> Self {
        self.scrollbar = scrollbar;
        self
    }

    /// Reserve the rightmost column of the list for the more button bar.
    ///
    /// The bar is reserved only when it is enabled and the list is wide enough, so that a narrow
    /// terminal does not lose its last column.
    pub fn more_button(mut self, more_button: bool) -> Self {
        self.more_button = more_button;
        self
    }

    /// Show the more button on the row of the given commit, which is the commit under the mouse.
    pub fn hovered(mut self, hovered: Option<HoveredCommit>) -> Self {
        self.hovered = hovered;
        self
    }
}

impl<'a> StatefulWidget for CommitList<'a> {
    type State = CommitListState<'a>;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        self.update_state(area, state);

        let [content_area, scrollbar_area] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(1)]).areas(area);
        let list_area = if self.scrollbar { content_area } else { area };

        let (columns_area, more_button_column) =
            split_more_button_column(list_area, self.more_button);
        state.set_more_button_column(more_button_column);

        let columns = self.visible_columns();
        let constraints = calc_cell_widths(
            columns_area.width,
            self.ctx.ui_config.list.subject_min_width,
            state.graph_area_cell_width(),
            self.ctx.ui_config.list.name_width,
            self.ctx.ui_config.list.date_width,
            &columns,
        );
        let chunks = Layout::horizontal(constraints).split(columns_area);

        // The badges are collected while rendering, so that a click can be matched to the badge
        // that was rendered under the cursor
        let mut ref_badges = Vec::new();
        for (i, col) in columns.iter().enumerate() {
            match col {
                UserListColumnType::Graph => {
                    self.render_graph(buf, chunks[i], state);
                }
                UserListColumnType::Marker => {
                    self.render_marker(buf, chunks[i], state);
                }
                UserListColumnType::Subject => {
                    ref_badges = self.render_subject(buf, chunks[i], state);
                }
                UserListColumnType::Name => {
                    self.render_name(buf, chunks[i], state);
                }
                UserListColumnType::Hash => {
                    self.render_hash(buf, chunks[i], state);
                }
                UserListColumnType::Date => {
                    self.render_date(buf, chunks[i], state);
                }
            }
        }
        state.set_ref_badges(ref_badges);

        if let Some(column) = more_button_column {
            self.render_more_buttons(buf, list_area, column, state);
        }

        if self.scrollbar {
            self.render_scrollbar(buf, scrollbar_area, state);
        }
    }
}

impl CommitList<'_> {
    fn update_state(&self, area: Rect, state: &mut CommitListState) {
        state.update_height(area.height as usize);
    }

    /// The columns of the list, without the ones that are hidden by the action menu.
    ///
    /// The hidden columns are dropped before the layout is calculated, so that the other columns
    /// keep their place and their width.
    fn visible_columns(&self) -> Vec<UserListColumnType> {
        let commit_list_config = self.ctx.commit_list_config.get();
        self.ctx
            .ui_config
            .list
            .columns
            .iter()
            .filter(|column| !commit_list_config.hides(column))
            .cloned()
            .collect()
    }

    /// Render the more button bar, which shows the more button of the row under the mouse.
    ///
    /// The cells of the bar are always drawn, so that the background of the selected row is not
    /// interrupted at the right edge of the list. The bar is the last column of the area of the
    /// list, which already excludes the scrollbar, so the button is never covered by the columns
    /// on its left nor by the scrollbar on its right.
    fn render_more_buttons(
        &self,
        buf: &mut Buffer,
        area: Rect,
        column: u16,
        state: &CommitListState,
    ) {
        // The bar is not rendered when it is outside of the buffer, which may happen when the
        // layout is not up to date with the terminal
        if area.is_empty() || column >= buf.area.right() {
            return;
        }
        let hovered_row = self
            .hovered
            .and_then(|hovered| hovered.index.checked_sub(state.offset))
            .filter(|row| *row < state.height);
        for row in 0..area.height as usize {
            if state.offset + row >= state.total {
                break;
            }
            let y = area.top() + row as u16;
            if y >= buf.area.bottom() {
                break;
            }
            let hovered = hovered_row == Some(row);
            let mut style = Style::default();
            if Some(row) == state.selected_row() {
                style = style
                    .bg(self.ctx.color_theme.list_selected_bg)
                    .fg(self.ctx.color_theme.list_selected_fg);
            } else if hovered {
                style = style.fg(self.ctx.color_theme.list_subject_fg);
            }
            if hovered && self.hovered.is_some_and(|hovered| hovered.armed) {
                style = style.add_modifier(Modifier::REVERSED);
            }
            let symbol = if hovered { MORE_BUTTON } else { " " };
            let cell = &mut buf[(column, y)];
            cell.set_symbol(symbol).set_style(style);
        }
    }

    fn render_scrollbar(&self, buf: &mut Buffer, area: Rect, state: &CommitListState) {
        if area.is_empty() {
            return;
        }
        let Some(geometry) = state.scrollbar_geometry() else {
            // The whole list is visible, so the scrollbar is not rendered
            return;
        };
        for row in 0..area.height {
            let (symbol, color) = if geometry.contains_thumb(row) {
                ("█", self.ctx.color_theme.list_scrollbar_thumb_fg)
            } else {
                ("│", self.ctx.color_theme.list_scrollbar_track_fg)
            };
            let cell = &mut buf[(area.left(), area.top() + row)];
            cell.set_symbol(symbol)
                .set_style(Style::default().fg(color));
        }
    }

    fn render_graph(&self, buf: &mut Buffer, area: Rect, state: &CommitListState) {
        if area.is_empty() {
            return;
        }
        self.rendering_commit_info_iter(state)
            .for_each(|(i, commit_info)| {
                let prepared_image = state.prepared_image(commit_info);
                let max_graph_width = area.width.saturating_sub(1) as usize;
                let y = area.top() + i as u16;
                for (x, image_cell) in prepared_image
                    .cells()
                    .iter()
                    .take(max_graph_width)
                    .enumerate()
                {
                    let cell = &mut buf[(area.left() + x as u16, y)];
                    cell.set_symbol(image_cell.symbol());
                    cell.set_style(image_cell.style());
                    cell.set_skip(image_cell.skip());
                }
            });
    }

    fn render_marker(&self, buf: &mut Buffer, area: Rect, state: &CommitListState) {
        if area.is_empty() {
            return;
        }
        let items: Vec<ListItem> = self
            .rendering_commit_info_iter(state)
            .map(|(_, commit_info)| ListItem::new("│".fg(commit_info.graph_color)))
            .collect();
        Widget::render(List::new(items), area, buf)
    }

    /// Renders the subject column, and returns the badges of the refs it contains.
    fn render_subject(
        &self,
        buf: &mut Buffer,
        area: Rect,
        state: &CommitListState,
    ) -> Vec<RefBadge> {
        let max_width = (area.width as usize).saturating_sub(2);
        if area.is_empty() || max_width == 0 {
            return Vec::new();
        }
        let mut badges = Vec::new();
        let items: Vec<ListItem> = self
            .rendering_commit_info_iter(state)
            .map(|(i, commit_info)| {
                let refs_spans = refs_spans(
                    commit_info,
                    state.head,
                    &state.search_matches[state.offset + i].refs,
                    &self.ctx.color_theme,
                );
                for name in &refs_spans.names {
                    badges.push(RefBadge {
                        rect: ref_badge_rect(area, i, name.offset, name.width),
                        r: name.r.clone(),
                    });
                }
                let mut spans = refs_spans.spans;
                let ref_spans_width: usize = spans.iter().map(|s| s.width()).sum();
                let max_width = max_width.saturating_sub(ref_spans_width);
                let commit = commit_info.commit;
                if max_width > ELLIPSIS.len() {
                    let truncate = console::measure_text_width(&commit.subject) > max_width;
                    let subject = if truncate {
                        console::truncate_str(&commit.subject, max_width, ELLIPSIS).to_string()
                    } else {
                        commit.subject.to_string()
                    };

                    let sub_spans =
                        if let Some(pos) = state.search_matches[state.offset + i].subject.clone() {
                            highlighted_spans(
                                subject.into(),
                                pos,
                                self.ctx.color_theme.list_subject_fg,
                                Modifier::empty(),
                                &self.ctx.color_theme,
                                truncate,
                            )
                        } else {
                            vec![subject.fg(self.ctx.color_theme.list_subject_fg)]
                        };

                    spans.extend(sub_spans)
                }
                self.to_commit_list_item(i, spans, state)
            })
            .collect();
        Widget::render(List::new(items), area, buf);
        badges
    }

    fn render_name(&self, buf: &mut Buffer, area: Rect, state: &CommitListState) {
        let max_width = (area.width as usize).saturating_sub(2);
        if area.is_empty() || max_width == 0 {
            return;
        }
        let items: Vec<ListItem> = self
            .rendering_commit_iter(state)
            .map(|(i, commit)| {
                let (name, padding, truncate) = right_aligned_name(&commit.author_name, max_width);
                let mut spans =
                    if let Some(pos) = state.search_matches[state.offset + i].author_name.clone() {
                        highlighted_spans(
                            name.into(),
                            pos,
                            self.ctx.color_theme.list_name_fg,
                            Modifier::empty(),
                            &self.ctx.color_theme,
                            truncate,
                        )
                    } else {
                        vec![name.fg(self.ctx.color_theme.list_name_fg)]
                    };
                if padding > 0 {
                    spans.insert(0, Span::raw(" ".repeat(padding)));
                }
                self.to_commit_list_item(i, spans, state)
            })
            .collect();
        Widget::render(List::new(items), area, buf);
    }

    fn render_hash(&self, buf: &mut Buffer, area: Rect, state: &CommitListState) {
        if area.is_empty() {
            return;
        }
        let items: Vec<ListItem> = self
            .rendering_commit_iter(state)
            .map(|(i, commit)| {
                let hash = commit.commit_hash.as_short_hash();
                let spans =
                    if let Some(pos) = state.search_matches[state.offset + i].commit_hash.clone() {
                        highlighted_spans(
                            hash.into(),
                            pos,
                            self.ctx.color_theme.list_hash_fg,
                            Modifier::empty(),
                            &self.ctx.color_theme,
                            false,
                        )
                    } else {
                        vec![hash.fg(self.ctx.color_theme.list_hash_fg)]
                    };
                self.to_commit_list_item(i, spans, state)
            })
            .collect();
        Widget::render(List::new(items), area, buf);
    }

    fn render_date(&self, buf: &mut Buffer, area: Rect, state: &CommitListState) {
        if area.is_empty() {
            return;
        }
        let items: Vec<ListItem> = self
            .rendering_commit_iter(state)
            .map(|(i, commit)| {
                let date = &commit.author_date;
                let date_str = if self.ctx.ui_config.list.date_local {
                    let local = date.with_timezone(&chrono::Local);
                    local
                        .format(&self.ctx.ui_config.list.date_format)
                        .to_string()
                } else {
                    date.format(&self.ctx.ui_config.list.date_format)
                        .to_string()
                };
                self.to_commit_list_item(
                    i,
                    vec![date_str.fg(self.ctx.color_theme.list_date_fg)],
                    state,
                )
            })
            .collect();
        Widget::render(List::new(items), area, buf);
    }

    fn rendering_commit_info_iter<'a>(
        &'a self,
        state: &'a CommitListState,
    ) -> impl Iterator<Item = (usize, &'a CommitInfo<'a>)> {
        state
            .commits
            .iter()
            .skip(state.offset)
            .take(state.height)
            .enumerate()
    }

    fn rendering_commit_iter<'a>(
        &'a self,
        state: &'a CommitListState,
    ) -> impl Iterator<Item = (usize, &'a Commit)> {
        self.rendering_commit_info_iter(state)
            .map(|(i, commit_info)| (i, commit_info.commit))
    }

    fn to_commit_list_item<'a, 'b>(
        &'b self,
        i: usize,
        spans: Vec<Span<'a>>,
        state: &'b CommitListState,
    ) -> ListItem<'a> {
        let mut spans = spans;
        spans.insert(0, Span::raw(" "));
        spans.push(Span::raw(" "));
        let mut line = Line::from(spans);
        if Some(i) == state.selected_row() {
            line = line
                .bg(self.ctx.color_theme.list_selected_bg)
                .fg(self.ctx.color_theme.list_selected_fg);
        }
        ListItem::new(line)
    }
}

/// The spans of the refs of a commit, and where the name of each ref is rendered.
struct RefsSpans<'a> {
    spans: Vec<Span<'a>>,
    /// The position of the name of each ref within `spans`.
    names: Vec<RefNameSpan<'a>>,
}

/// The position of the name of a ref within the spans of the refs of a commit.
struct RefNameSpan<'a> {
    /// The width of the text that precedes the name, in cells.
    offset: usize,
    /// The width of the name, in cells.
    width: usize,
    r: &'a Ref,
}

/// The spans of the refs of a commit.
///
/// The name of each ref is rendered as a reversed badge, colored by the type of the ref, and the
/// badges are separated by spaces. The badges are grouped by type: local branches, remote branches,
/// and tags. The ref that HEAD points to is marked with a dot in front of it, instead of the name of
/// HEAD.
fn refs_spans<'a>(
    commit_info: &'a CommitInfo,
    head: &'a Head,
    refs_matches: &'a FxHashMap<String, SearchMatchPosition>,
    color_theme: &'a ColorTheme,
) -> RefsSpans<'a> {
    let refs = &commit_info.refs;

    if refs.len() == 1 {
        if let Ref::Stash { name, .. } = refs[0] {
            return RefsSpans {
                spans: vec![
                    Span::raw(name).fg(color_theme.list_ref_stash_fg).bold(),
                    Span::raw(" "),
                ],
                names: Vec::new(),
            };
        }
    }

    let mut ref_spans: Vec<(Vec<Span>, &Ref, usize)> = refs
        .iter()
        .filter_map(|r| match *r {
            Ref::Branch { name, .. } => {
                let fg = color_theme.list_ref_branch_fg;
                Some((name, fg, *r))
            }
            Ref::RemoteBranch { name, .. } => {
                let fg = color_theme.list_ref_remote_branch_fg;
                Some((name, fg, *r))
            }
            Ref::Tag { name, .. } => {
                let fg = color_theme.list_ref_tag_fg;
                Some((name, fg, *r))
            }
            Ref::Stash { .. } => None,
        })
        .map(|(name, fg, r)| {
            let spans = refs_matches
                .get(name)
                .map(|pos| {
                    highlighted_spans(
                        name.into(),
                        pos.clone(),
                        fg,
                        BADGE_MODIFIER,
                        color_theme,
                        false,
                    )
                })
                .unwrap_or_else(|| vec![Span::raw(name).fg(fg).add_modifier(BADGE_MODIFIER)]);
            let width: usize = spans.iter().map(Span::width).sum();
            (spans, r, width)
        })
        .collect();

    // The badges are grouped by type: local branches first, then remote branches, and tags last.
    // The sort is stable, so that the refs of a group keep the order they are loaded in.
    ref_spans.sort_by_key(|(_, r, _)| ref_group(r));

    let mut spans: Vec<Span> = Vec::new();
    let mut names: Vec<RefNameSpan> = Vec::new();

    if let Head::Detached { target } = head {
        if commit_info.commit.commit_hash == *target {
            spans.push(head_marker(color_theme));
            // The separator is also pushed when there is no ref, so that the subject does not
            // stick to the marker
            spans.push(separator(color_theme));
        }
    }

    for (ref_spans, r, width) in ref_spans {
        if head_points_to(head, r) {
            // The marker is pushed right in front of the branch that HEAD points to, so that a tag
            // with the same name cannot be mistaken for it
            spans.push(head_marker(color_theme));
            spans.push(separator(color_theme));
        }
        let offset: usize = spans.iter().map(Span::width).sum();
        spans.extend(ref_spans);
        names.push(RefNameSpan { offset, width, r });
        spans.push(separator(color_theme));
    }

    RefsSpans { spans, names }
}

/// The order in which the types of refs are rendered: local branches first, then remote branches,
/// and tags last.
fn ref_group(r: &Ref) -> u8 {
    match r {
        Ref::Branch { .. } => 0,
        Ref::RemoteBranch { .. } => 1,
        Ref::Tag { .. } => 2,
        Ref::Stash { .. } => 3,
    }
}

/// Whether HEAD points to the given ref.
///
/// A tag can have the same name as the branch that HEAD points to, so only a local branch matches.
fn head_points_to(head: &Head, r: &Ref) -> bool {
    match head {
        Head::Branch { name } => matches!(r, Ref::Branch { .. }) && r.name() == name,
        Head::Detached { .. } | Head::None => false,
    }
}

/// The marker rendered in front of the ref that HEAD points to.
fn head_marker(color_theme: &ColorTheme) -> Span<'static> {
    Span::raw(HEAD_MARKER).fg(color_theme.list_head_fg).bold()
}

/// The space that separates the refs of a commit.
fn separator(color_theme: &ColorTheme) -> Span<'static> {
    Span::raw(" ").fg(color_theme.list_ref_paren_fg)
}

/// The area of the name of a ref in the subject column.
///
/// The name is rendered at `offset` cells from the start of the row, after the space that precedes
/// the refs of the commit. The name is clipped to the column, because the list truncates the items
/// to the width of the column.
fn ref_badge_rect(area: Rect, row: usize, offset: usize, width: usize) -> Rect {
    let row = area.top().saturating_add(row as u16);
    let left = area.left().saturating_add(1).saturating_add(offset as u16);
    let right = left.saturating_add(width as u16).min(area.right());
    Rect::new(left, row, right.saturating_sub(left), 1)
}

fn highlighted_spans(
    s: Span<'_>,
    pos: SearchMatchPosition,
    base_fg: Color,
    base_modifier: Modifier,
    color_theme: &ColorTheme,
    truncate: bool,
) -> Vec<Span<'static>> {
    let mut hm = highlight_matched_text(vec![s])
        .matched_indices(pos.matched_indices)
        .not_matched_style(Style::default().fg(base_fg).add_modifier(base_modifier))
        .matched_style(
            Style::default()
                .fg(color_theme.list_match_fg)
                .bg(color_theme.list_match_bg)
                .add_modifier(base_modifier),
        );
    if truncate {
        hm = hm.ellipsis(ELLIPSIS);
    }
    hm.into_spans()
}

/// Right-aligns an author name inside a cell of `max_width` columns.
///
/// The name is truncated when it does not fit in the cell.
/// Returns the possibly truncated name, the number of leading spaces that pushes
/// it to the right edge of the cell, and whether the name was truncated.
///
/// The leading spaces are prepended after the search highlight is computed,
/// because adding them to the highlighted text would shift the matched indices.
fn right_aligned_name(name: &str, max_width: usize) -> (String, usize, bool) {
    let truncate = console::measure_text_width(name) > max_width;
    let name = if truncate {
        console::truncate_str(name, max_width, ELLIPSIS).to_string()
    } else {
        name.to_string()
    };
    let padding = max_width.saturating_sub(console::measure_text_width(&name));
    (name, padding, truncate)
}

fn calc_cell_widths(
    area_width: u16,
    subject_min_width: u16,
    graph_width: u16,
    name_width: u16,
    date_width: u16,
    columns: &[UserListColumnType],
) -> Vec<Constraint> {
    let pad = 2;
    let (
        mut graph_cell_width,
        mut marker_cell_width,
        mut name_cell_width,
        mut hash_cell_width,
        mut date_cell_width,
    ) = (0, 0, 0, 0, 0);

    for col in columns {
        match col {
            UserListColumnType::Graph => {
                graph_cell_width = graph_width;
            }
            UserListColumnType::Marker => {
                marker_cell_width = 1;
            }
            UserListColumnType::Name => {
                name_cell_width = name_width + pad;
            }
            UserListColumnType::Hash => {
                hash_cell_width = 7 + pad;
            }
            UserListColumnType::Date => {
                date_cell_width = date_width + pad;
            }
            UserListColumnType::Subject => {}
        }
    }

    let mut total_width = graph_cell_width
        + marker_cell_width
        + hash_cell_width
        + name_cell_width
        + date_cell_width
        + subject_min_width;

    if total_width > area_width {
        total_width = total_width.saturating_sub(name_cell_width);
        name_cell_width = 0;
    }
    if total_width > area_width {
        total_width = total_width.saturating_sub(date_cell_width);
        date_cell_width = 0;
    }
    if total_width > area_width {
        hash_cell_width = 0;
    }

    let mut constraints = Vec::new();
    for col in columns {
        match col {
            UserListColumnType::Graph => {
                constraints.push(Constraint::Length(graph_cell_width));
            }
            UserListColumnType::Marker => {
                constraints.push(Constraint::Length(marker_cell_width));
            }
            UserListColumnType::Subject => {
                constraints.push(Constraint::Min(0));
            }
            UserListColumnType::Name => {
                constraints.push(Constraint::Length(name_cell_width));
            }
            UserListColumnType::Hash => {
                constraints.push(Constraint::Length(hash_cell_width));
            }
            UserListColumnType::Date => {
                constraints.push(Constraint::Length(date_cell_width));
            }
        }
    }
    constraints
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::PathBuf;

    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    use crate::{
        color::GraphColorSet,
        config::{ActionsConfig, CoreConfig, GraphColorConfig, UiCommitListConfig, UiConfig},
        git::Repository,
        graph::{calc_graph, CellWidthType, GraphImageWidthMode, GraphStyle},
        keybind::KeyBind,
        protocol::ImageProtocol,
        view::ListRefreshViewContext,
    };

    use super::*;

    fn with_commit_list_state<R>(
        subjects: &[&str],
        f: impl FnOnce(&mut CommitListState<'_>) -> R,
    ) -> R {
        let commits: Vec<Commit> = subjects
            .iter()
            .enumerate()
            .map(|(i, subject)| Commit {
                commit_hash: CommitHash::from(format!("{:040x}", i + 1).as_str()),
                subject: (*subject).into(),
                ..Commit::default()
            })
            .collect();
        with_commits(commits, f)
    }

    /// The state of a list of the given commits, for the tests.
    fn with_commits<R>(commits: Vec<Commit>, f: impl FnOnce(&mut CommitListState<'_>) -> R) -> R {
        let count = commits.len();
        let commit_hashes = commits.iter().map(|c| c.commit_hash.clone()).collect();
        let commit_map = commits
            .into_iter()
            .map(|c| (c.commit_hash.clone(), c))
            .collect();
        let repository = Repository::new(
            PathBuf::new(),
            commit_map,
            FxHashMap::default(),
            FxHashMap::default(),
            FxHashMap::default(),
            Head::None,
            commit_hashes,
        );
        let graph = calc_graph(&repository, None);
        let graph_color_set = GraphColorSet::new(&GraphColorConfig::default());
        let graph_image_manager = GraphImageManager::new(
            &graph,
            &graph_color_set,
            CellWidthType::Double,
            GraphStyle::Rounded,
            GraphImageWidthMode::Compact,
            ImageProtocol::Iterm2,
        );
        let commit_infos = graph
            .commits
            .iter()
            .map(|commit| {
                CommitInfo::new(commit, repository.refs(&commit.commit_hash), Color::Reset)
            })
            .collect();
        let mut state = CommitListState::new(
            commit_infos,
            graph_image_manager,
            0,
            repository.head(),
            FxHashMap::default(),
            SearchOptions::default(),
            0,
        );
        state.reset_height(count);
        f(&mut state)
    }

    fn input_search_query(state: &mut CommitListState<'_>, query: &str) {
        state.start_search();
        for c in query.chars() {
            state.handle_search_input(KeyEvent::from(KeyCode::Char(c)));
        }
    }

    #[test]
    fn test_scrolloff_keeps_context_for_selection() {
        with_commit_list_state(&["commit"; 12], |state| {
            state.reset_height(6);
            state.scrolloff = 2;

            for _ in 0..4 {
                state.select_next();
            }
            assert_eq!(state.current_list_status(), (4, 1, 6));

            state.select_prev();
            state.select_prev();
            assert_eq!(state.current_list_status(), (2, 0, 6));

            state.select_index(7);
            assert_eq!(state.current_list_status(), (7, 5, 6));

            state.select_last();
            assert_eq!(state.current_list_status(), (11, 6, 6));
            state.select_prev();
            assert_eq!(state.current_list_status(), (10, 6, 6));
            assert_eq!(state.selected_row(), Some(4));
        });
    }

    #[test]
    fn test_scrolloff_is_limited_by_list_height() {
        with_commit_list_state(&["commit"; 8], |state| {
            state.reset_height(2);
            state.scrolloff = 10;

            state.select_next();
            state.select_next();
            assert_eq!(state.current_list_status(), (2, 1, 2));

            state.select_index(6);
            state.restore_selected_index(6, 5);
            assert_eq!(state.current_list_status(), (6, 5, 2));
            assert_eq!(state.selected_row(), Some(1));
        });
    }

    #[test]
    fn test_page_scrolloff_keeps_context_until_final_page() {
        with_commit_list_state(&["commit"; 30], |state| {
            state.reset_height(10);
            state.scrolloff = 2;

            state.scroll_down_page();
            assert_eq!(state.current_list_status(), (10, 8, 10));
            state.scroll_down_page();
            assert_eq!(state.current_list_status(), (20, 18, 10));
            state.scroll_down_page();
            assert_eq!(state.current_list_status(), (29, 20, 10));
            state.scroll_down_page();
            assert_eq!(state.current_list_status(), (29, 20, 10));

            state.scroll_up_page();
            assert_eq!(state.current_list_status(), (19, 12, 10));
            state.scroll_up_page();
            assert_eq!(state.current_list_status(), (9, 2, 10));
            state.scroll_up_page();
            assert_eq!(state.current_list_status(), (0, 0, 10));
        });
    }

    #[test]
    fn test_half_page_scroll_reaches_last_commit_after_final_page() {
        with_commit_list_state(&["commit"; 26], |state| {
            state.reset_height(10);
            state.scrolloff = 2;
            state.select_index(12);

            state.scroll_down_half();
            assert_eq!(state.current_list_status(), (17, 15, 10));
            state.scroll_down_half();
            assert_eq!(state.current_list_status(), (22, 16, 10));
            state.scroll_down_half();
            assert_eq!(state.current_list_status(), (25, 16, 10));
        });
    }

    #[test]
    fn test_full_page_scroll_reaches_last_commit_after_final_page() {
        with_commit_list_state(&["commit"; 25], |state| {
            state.reset_height(10);
            state.scrolloff = 2;
            state.select_index(12);

            state.scroll_down_page();
            assert_eq!(state.current_list_status(), (22, 15, 10));
            state.scroll_down_page();
            assert_eq!(state.current_list_status(), (24, 15, 10));
        });
    }

    #[test]
    fn test_restore_selected_index_restores_absolute_index_and_offset() {
        with_commit_list_state(&["commit"; 30], |state| {
            state.reset_height(10);
            state.scrolloff = 2;
            state.select_index(15);
            assert_eq!(state.current_list_status(), (15, 13, 10));

            state.restore_selected_index(15, 8);
            assert_eq!(state.current_list_status(), (15, 8, 10));
            assert_eq!(state.selected_row(), Some(7));
            assert_eq!(state.current_selected_index(), 15);

            // The selection may be restored out of the viewport
            state.restore_selected_index(15, 20);
            assert_eq!(state.current_list_status(), (15, 20, 10));
            assert_eq!(state.selected_row(), None);

            // The index and the offset are clamped to the list
            state.restore_selected_index(100, 100);
            assert_eq!(state.current_list_status(), (29, 20, 10));
        });
    }

    #[test]
    fn test_pure_scroll_does_not_change_the_selected_commit() {
        with_commit_list_state(&["commit"; 20], |state| {
            state.reset_height(5);
            state.select_index(6);
            assert_eq!(state.current_list_status(), (6, 6, 5));
            assert_eq!(state.selected_row(), Some(0));

            // Scrolling down pushes the selected commit out of the viewport
            for _ in 0..6 {
                state.scroll_down();
            }
            assert_eq!(state.current_list_status(), (6, 12, 5));
            assert_eq!(state.selected_row(), None);

            state.scroll_down();
            assert_eq!(state.current_list_status(), (6, 13, 5));
            assert_eq!(state.selected_row(), None);

            // Scrolling up brings the same commit back into the viewport
            for _ in 0..3 {
                state.scroll_up();
            }
            assert_eq!(state.current_list_status(), (6, 10, 5));
            assert_eq!(state.selected_row(), None);
            for _ in 0..4 {
                state.scroll_up();
            }
            assert_eq!(state.current_list_status(), (6, 6, 5));
            assert_eq!(state.selected_row(), Some(0));
        });
    }

    #[test]
    fn test_pure_scroll_is_clamped_to_the_list_edges() {
        with_commit_list_state(&["commit"; 20], |state| {
            state.reset_height(5);
            state.select_index(3);
            assert_eq!(state.current_list_status(), (3, 3, 5));
            assert_eq!(state.selected_row(), Some(0));

            for _ in 0..20 {
                state.scroll_up();
            }
            assert_eq!(state.current_list_status(), (3, 0, 5));
            assert_eq!(state.selected_row(), Some(3));

            for _ in 0..20 {
                state.scroll_down();
            }
            assert_eq!(state.current_list_status(), (3, 15, 5));
            assert_eq!(state.selected_row(), None);

            state.set_offset(100);
            assert_eq!(state.current_list_status(), (3, 15, 5));
        });
    }

    #[test]
    fn test_navigation_scrolls_the_selection_back_into_view_with_scrolloff() {
        with_commit_list_state(&["commit"; 20], |state| {
            state.reset_height(5);
            state.scrolloff = 1;
            state.select_index(6);
            assert_eq!(state.current_list_status(), (6, 5, 5));
            assert_eq!(state.selected_row(), Some(1));

            for _ in 0..8 {
                state.scroll_down();
            }
            assert_eq!(state.current_list_status(), (6, 13, 5));
            assert_eq!(state.selected_row(), None);

            // The selection is scrolled back into view, keeping the scrolloff margin above it
            state.select_next();
            assert_eq!(state.current_list_status(), (7, 6, 5));
            assert_eq!(state.selected_row(), Some(1));

            state.select_prev();
            assert_eq!(state.current_list_status(), (6, 5, 5));
            assert_eq!(state.selected_row(), Some(1));
        });
    }

    #[test]
    fn test_navigation_scrolls_a_selection_below_the_viewport_back_into_view() {
        with_commit_list_state(&["commit"; 20], |state| {
            state.reset_height(5);
            state.select_index(15);
            state.set_offset(0);
            assert_eq!(state.current_list_status(), (15, 0, 5));
            assert_eq!(state.selected_row(), None);

            state.select_prev();
            assert_eq!(state.current_list_status(), (14, 10, 5));
            assert_eq!(state.selected_row(), Some(4));
        });
    }

    #[test]
    fn test_select_high_middle_low_is_relative_to_the_viewport() {
        with_commit_list_state(&["commit"; 30], |state| {
            state.reset_height(6);
            state.select_index(12);
            assert_eq!(state.current_list_status(), (12, 12, 6));

            state.select_high();
            assert_eq!(state.current_list_status(), (12, 12, 6));
            assert_eq!(state.selected_row(), Some(0));

            state.scroll_down();
            state.scroll_down();
            assert_eq!(state.current_list_status(), (12, 14, 6));

            state.select_high();
            assert_eq!(state.current_list_status(), (14, 14, 6));
            state.select_middle();
            assert_eq!(state.current_list_status(), (17, 14, 6));
            state.select_low();
            assert_eq!(state.current_list_status(), (19, 14, 6));
            assert_eq!(state.selected_row(), Some(5));
        });
    }

    #[test]
    fn test_select_high_middle_low_with_a_list_shorter_than_the_viewport() {
        with_commit_list_state(&["commit"; 3], |state| {
            state.reset_height(5);

            state.select_last();
            assert_eq!(state.current_list_status(), (2, 0, 5));
            assert_eq!(state.selected_row(), Some(2));

            state.select_high();
            assert_eq!(state.current_list_status(), (0, 0, 5));
            state.select_middle();
            assert_eq!(state.current_list_status(), (1, 0, 5));
            state.select_low();
            assert_eq!(state.current_list_status(), (2, 0, 5));
            assert_eq!(state.selected_row(), Some(2));
        });
    }

    #[test]
    fn test_select_low_does_not_exceed_the_last_commit() {
        with_commit_list_state(&["commit"; 8], |state| {
            state.reset_height(5);
            state.select_index(6);
            assert_eq!(state.current_list_status(), (6, 3, 5));

            state.select_low();
            assert_eq!(state.current_list_status(), (7, 3, 5));
            assert_eq!(state.selected_row(), Some(4));

            state.select_middle();
            assert_eq!(state.current_list_status(), (5, 3, 5));
        });
    }

    #[test]
    fn test_resize_keeps_the_selection_and_clamps_the_offset() {
        with_commit_list_state(&["commit"; 20], |state| {
            state.reset_height(10);
            state.select_index(15);
            assert_eq!(state.current_list_status(), (15, 10, 10));
            assert_eq!(state.selected_row(), Some(5));

            // Growing the viewport clamps the offset to the new maximum
            state.update_height(12);
            assert_eq!(state.current_list_status(), (15, 8, 12));
            assert_eq!(state.selected_row(), Some(7));

            // Shrinking the viewport scrolls the visible selection back into view
            state.update_height(5);
            assert_eq!(state.current_list_status(), (15, 11, 5));
            assert_eq!(state.selected_row(), Some(4));

            // Repeated calls with the same height do not change the state
            state.update_height(5);
            assert_eq!(state.current_list_status(), (15, 11, 5));
        });
    }

    #[test]
    fn test_resize_keeps_an_out_of_view_selection_out_of_view() {
        with_commit_list_state(&["commit"; 20], |state| {
            state.reset_height(5);
            state.select_index(15);
            state.set_offset(0);
            assert_eq!(state.selected_row(), None);

            state.update_height(3);
            assert_eq!(state.current_list_status(), (15, 0, 3));
            assert_eq!(state.selected_row(), None);
        });
    }

    #[test]
    fn test_refresh_restores_the_selected_index_and_the_offset() {
        let context = with_commit_list_state(&["commit"; 30], |state| {
            state.reset_height(10);
            state.scrolloff = 2;
            state.select_index(15);
            assert_eq!(state.current_list_status(), (15, 13, 10));
            ListRefreshViewContext::from(&*state)
        });
        assert_eq!(context.selected, 15);
        assert_eq!(context.offset, 13);
        assert!(!context.scroll_to_top);

        with_commit_list_state(&["commit"; 30], |state| {
            state.reset_height(10);
            context.restore(state);
            assert_eq!(state.current_list_status(), (15, 13, 10));
            assert_eq!(state.selected_row(), Some(2));
        });
    }

    #[test]
    fn test_refresh_scrolls_to_top_when_the_list_was_at_the_top() {
        let context = with_commit_list_state(&["commit"; 4], |state| {
            state.reset_height(5);
            ListRefreshViewContext::from(&*state)
        });
        assert!(context.scroll_to_top);

        // The refreshed list has a new commit on top
        with_commit_list_state(
            &["new commit", "commit", "commit", "commit", "commit"],
            |state| {
                state.reset_height(5);
                state.select_last();
                context.restore(state);
                assert_eq!(state.current_list_status(), (0, 0, 5));
                assert_eq!(state.selected_row(), Some(0));
                assert_eq!(state.commits[0].commit.subject, "new commit");
            },
        );
    }

    #[test]
    fn test_refresh_falls_back_to_the_recorded_index_when_the_commit_is_gone() {
        let context = with_commit_list_state(&["a", "b", "c", "d"], |state| {
            state.reset_height(10);
            state.select_index(2);
            ListRefreshViewContext::from(&*state)
        });

        with_commit_list_state(&["a", "b"], |state| {
            state.reset_height(10);
            context.restore(state);
            // The index is clamped to the last commit of the refreshed list
            assert_eq!(state.current_list_status(), (1, 0, 10));
            assert_eq!(state.selected_row(), Some(1));
        });
    }

    #[test]
    fn test_commit_index_of_hash() {
        with_commit_list_state(&["a", "b", "c"], |state| {
            let existing = CommitHash::from(format!("{:040x}", 2).as_str());
            let missing = CommitHash::from(format!("{:040x}", 9).as_str());
            assert_eq!(state.commit_index_of_hash(&existing), Some(1));
            assert_eq!(state.commit_index_of_hash(&missing), None);
        });
    }

    #[test]
    fn test_commit_index_at_row() {
        let list_area = Rect::new(0, 2, 80, 5);
        with_commit_list_state(&["commit"; 20], |state| {
            state.reset_height(list_area.height as usize);

            // offset = 0
            assert_eq!(state.commit_index_at_row(list_area, 2), Some(0));
            assert_eq!(state.commit_index_at_row(list_area, 6), Some(4));

            // rows outside the list area are ignored
            assert_eq!(state.commit_index_at_row(list_area, 1), None);
            assert_eq!(state.commit_index_at_row(list_area, 7), None);

            // offset > 0
            state.set_offset(5);
            assert_eq!(state.commit_index_at_row(list_area, 2), Some(5));
            assert_eq!(state.commit_index_at_row(list_area, 6), Some(9));
        });
    }

    #[test]
    fn test_commit_index_at_row_without_a_commit() {
        let list_area = Rect::new(0, 0, 80, 5);

        // rows below the last commit are ignored
        with_commit_list_state(&["commit"; 3], |state| {
            state.reset_height(list_area.height as usize);
            assert_eq!(state.commit_index_at_row(list_area, 2), Some(2));
            assert_eq!(state.commit_index_at_row(list_area, 3), None);
            assert_eq!(state.commit_index_at_row(list_area, 4), None);
        });

        with_commit_list_state(&[], |state| {
            state.reset_height(list_area.height as usize);
            assert_eq!(state.commit_index_at_row(list_area, 0), None);
        });
    }

    #[test]
    fn test_scrollbar_geometry_is_not_shown_when_the_list_fits() {
        with_commit_list_state(&["commit"; 5], |state| {
            state.reset_height(5);
            assert_eq!(state.scrollbar_geometry(), None);

            state.reset_height(6);
            assert_eq!(state.scrollbar_geometry(), None);

            state.reset_height(0);
            assert_eq!(state.scrollbar_geometry(), None);
        });

        with_commit_list_state(&[], |state| {
            state.reset_height(5);
            assert_eq!(state.scrollbar_geometry(), None);
        });
    }

    #[test]
    fn test_scrollbar_geometry_thumb_length_and_position() {
        with_commit_list_state(&["commit"; 20], |state| {
            state.reset_height(5);
            // thumb_len = round(5 * 5 / 20) = 1
            assert_eq!(
                state.scrollbar_geometry(),
                Some(ScrollbarGeometry {
                    thumb_start: 0,
                    thumb_len: 1,
                })
            );

            state.set_offset(7);
            assert_eq!(
                state.scrollbar_geometry(),
                Some(ScrollbarGeometry {
                    thumb_start: 2,
                    thumb_len: 1,
                })
            );

            state.set_offset(15);
            assert_eq!(
                state.scrollbar_geometry(),
                Some(ScrollbarGeometry {
                    thumb_start: 4,
                    thumb_len: 1,
                })
            );
        });

        // one commit beyond the viewport
        with_commit_list_state(&["commit"; 6], |state| {
            state.reset_height(5);
            // thumb_len = round(5 * 5 / 6) = 4
            assert_eq!(
                state.scrollbar_geometry(),
                Some(ScrollbarGeometry {
                    thumb_start: 0,
                    thumb_len: 4,
                })
            );

            state.set_offset(1);
            assert_eq!(
                state.scrollbar_geometry(),
                Some(ScrollbarGeometry {
                    thumb_start: 1,
                    thumb_len: 4,
                })
            );
        });

        // the thumb always occupies at least one row
        with_commit_list_state(&["commit"; 100], |state| {
            state.reset_height(5);
            // thumb_len = round(5 * 5 / 100) = 0 -> 1
            assert_eq!(
                state.scrollbar_geometry(),
                Some(ScrollbarGeometry {
                    thumb_start: 0,
                    thumb_len: 1,
                })
            );
        });
    }

    #[test]
    fn test_scrollbar_geometry_with_a_single_visible_row() {
        with_commit_list_state(&["commit"; 3], |state| {
            state.reset_height(1);
            // The thumb occupies the whole track, so there is no position information
            assert_eq!(
                state.scrollbar_geometry(),
                Some(ScrollbarGeometry {
                    thumb_start: 0,
                    thumb_len: 1,
                })
            );
            assert_eq!(state.offset_for_thumb_start(0), 0);
        });
    }

    #[test]
    fn test_scrollbar_offset_round_trip() {
        let cases = [
            (20, 5), // thumb_len = 1
            (6, 5),  // thumb_len = 4
            (30, 7), // thumb_len = 2
            (100, 20),
        ];
        for (total, height) in cases {
            let subjects = vec!["commit"; total];
            with_commit_list_state(&subjects, |state| {
                state.reset_height(height);
                let geometry = state.scrollbar_geometry().unwrap();
                let max_start = height - geometry.thumb_len as usize;
                let max_offset = total - height;

                // thumb position -> offset -> thumb position
                for thumb_start in 0..=max_start {
                    let offset = state.offset_for_thumb_start(thumb_start as u16);
                    state.set_offset(offset);
                    assert_eq!(
                        state.scrollbar_geometry().unwrap().thumb_start,
                        thumb_start as u16,
                        "total={total} height={height} thumb_start={thumb_start}"
                    );
                }

                // offset -> thumb position -> offset (within the granularity of the scrollbar)
                let granularity = max_offset.div_ceil(max_start.max(1));
                for offset in 0..=max_offset {
                    state.set_offset(offset);
                    let thumb_start = state.scrollbar_geometry().unwrap().thumb_start;
                    let restored = state.offset_for_thumb_start(thumb_start);
                    assert!(
                        restored.abs_diff(offset) <= granularity,
                        "total={total} height={height} offset={offset} restored={restored}"
                    );
                }
            });
        }
    }

    #[test]
    fn test_scrollbar_grab_offset() {
        let geometry = ScrollbarGeometry {
            thumb_start: 3,
            thumb_len: 2,
        };
        assert!(!geometry.contains_thumb(2));
        assert!(geometry.contains_thumb(3));
        assert!(geometry.contains_thumb(4));
        assert!(!geometry.contains_thumb(5));

        // the cursor keeps its position within the thumb
        assert_eq!(geometry.grab_offset(3), 0);
        assert_eq!(geometry.grab_offset(4), 1);
        // the thumb jumps to the pressed row when the track is pressed
        assert_eq!(geometry.grab_offset(1), 0);
        assert_eq!(geometry.grab_offset(6), 0);
    }

    #[test]
    fn test_scrollbar_drag_clamps_to_the_list_edges() {
        with_commit_list_state(&["commit"; 20], |state| {
            state.reset_height(5);
            let max_offset = 15;

            assert_eq!(state.offset_for_thumb_start(0), 0);
            assert_eq!(state.offset_for_thumb_start(u16::MAX), max_offset);

            state.set_offset(state.offset_for_thumb_start(u16::MAX));
            assert_eq!(state.current_list_status(), (0, max_offset, 5));
        });

        // nothing to scroll
        with_commit_list_state(&["commit"; 5], |state| {
            state.reset_height(5);
            assert_eq!(state.offset_for_thumb_start(0), 0);
            assert_eq!(state.offset_for_thumb_start(u16::MAX), 0);

            state.set_offset(state.offset_for_thumb_start(u16::MAX));
            assert_eq!(state.current_list_status(), (0, 0, 5));
        });
    }

    #[test]
    fn test_wheel_scroll_does_not_change_the_selected_commit() {
        with_commit_list_state(&["commit"; 20], |state| {
            state.reset_height(5);
            state.select_index(7);
            assert_eq!(state.current_list_status(), (7, 7, 5));

            // one wheel event scrolls exactly one row
            state.scroll_down();
            assert_eq!(state.current_list_status(), (7, 8, 5));
            state.scroll_down();
            assert_eq!(state.current_list_status(), (7, 9, 5));

            // the selection is left behind by the scroll, without being changed
            state.scroll_down();
            assert_eq!(state.current_list_status(), (7, 10, 5));
            assert_eq!(state.selected_row(), None);

            // clamped to the end of the list
            for _ in 0..20 {
                state.scroll_down();
            }
            assert_eq!(
                state.current_list_status(),
                (7, 15, 5),
                "scrolling down at the end of the list does not move the viewport"
            );

            // and to the top of the list
            state.scroll_up();
            assert_eq!(state.current_list_status(), (7, 14, 5));
            for _ in 0..20 {
                state.scroll_up();
            }
            assert_eq!(
                state.current_list_status(),
                (7, 0, 5),
                "scrolling up at the top of the list does not move the viewport"
            );
            assert_eq!(state.selected_row(), None);

            // scrolling back brings the same commit into view
            for _ in 0..7 {
                state.scroll_down();
            }
            assert_eq!(state.current_list_status(), (7, 7, 5));
            assert_eq!(state.selected_row(), Some(0));
        });
    }

    #[test]
    fn test_the_viewport_does_not_move_when_there_is_nothing_to_scroll() {
        // The whole list is visible, so the maximum offset is zero
        with_commit_list_state(&["commit"; 3], |state| {
            state.reset_height(5);
            for _ in 0..5 {
                state.scroll_down();
                state.scroll_down_page();
                state.scroll_down_half();
                state.scroll_up();
                state.scroll_up_page();
                state.scroll_up_half();
                state.select_next();
                assert_eq!(state.current_list_status().1, 0, "the offset stays at zero");
            }
        });

        // The list is exactly as long as the viewport
        with_commit_list_state(&["commit"; 5], |state| {
            state.reset_height(5);
            state.scroll_down();
            state.scroll_down_page();
            assert_eq!(state.current_list_status().1, 0, "the offset stays at zero");
        });

        // There is no commit
        with_commit_list_state(&[], |state| {
            state.reset_height(5);
            state.scroll_down();
            state.scroll_down_page();
            state.scroll_down_half();
            state.scroll_up();
            state.scroll_up_page();
            state.select_last();
            state.select_next();
            assert_eq!(state.current_list_status(), (0, 0, 5));
        });
    }

    #[test]
    fn test_a_page_scroll_that_is_larger_than_the_remaining_distance_stops_at_the_end() {
        with_commit_list_state(&["commit"; 20], |state| {
            state.reset_height(5);
            assert_eq!(state.current_list_status(), (0, 0, 5));

            // only two rows are left below the viewport, but a page is five rows
            state.set_offset(13);
            state.scroll_down_page();
            assert_eq!(
                state.current_list_status(),
                (5, 15, 5),
                "the offset stops exactly at the last one"
            );

            // the offset does not grow past the end, even though the selection still moves
            state.scroll_down_page();
            assert_eq!(state.current_list_status(), (10, 15, 5));
            state.scroll_down_page();
            assert_eq!(state.current_list_status(), (15, 15, 5));
        });
    }

    #[test]
    fn test_page_scroll_moves_the_selection_with_the_viewport() {
        with_commit_list_state(&["commit"; 20], |state| {
            state.reset_height(5);
            state.select_index(7);
            assert_eq!(state.current_list_status(), (7, 7, 5));

            // the selection keeps its row in the viewport
            state.scroll_down_page();
            assert_eq!(state.current_list_status(), (12, 12, 5));
            assert_eq!(state.selected_row(), Some(0));
            state.scroll_up_page();
            assert_eq!(state.current_list_status(), (7, 7, 5));
            assert_eq!(state.selected_row(), Some(0));

            // further page scrolls reach the last commit
            state.select_last();
            assert_eq!(state.current_list_status(), (19, 15, 5));
            state.scroll_up_page();
            assert_eq!(state.current_list_status(), (14, 10, 5));
            assert_eq!(state.selected_row(), Some(4));
        });
    }

    #[test]
    fn test_page_down_does_not_jump_the_viewport_back_to_the_selection() {
        with_commit_list_state(&["commit"; 40], |state| {
            state.reset_height(5);

            // The wheel scrolls the viewport without moving the selection, which leaves the
            // selected commit above the viewport
            for _ in 0..20 {
                state.scroll_down();
            }
            assert_eq!(state.current_list_status(), (0, 20, 5));
            assert_eq!(state.selected_row(), None);

            // A page down moves the viewport and the selection by a page: it does not scroll the
            // viewport back up to the selected commit
            state.scroll_down_page();
            assert_eq!(state.current_list_status(), (5, 25, 5));
            assert_eq!(state.selected_row(), None);
        });
    }

    #[test]
    fn test_page_up_does_not_jump_the_viewport_forward_to_the_selection() {
        with_commit_list_state(&["commit"; 40], |state| {
            state.reset_height(5);
            state.select_index(35);
            assert_eq!(state.current_list_status(), (35, 35, 5));

            // The scrollbar scrolls the viewport without moving the selection, which leaves the
            // selected commit below the viewport
            for _ in 0..25 {
                state.scroll_up();
            }
            assert_eq!(state.current_list_status(), (35, 10, 5));
            assert_eq!(state.selected_row(), None);

            // A page up moves the viewport and the selection by a page: it does not scroll the
            // viewport down to the selected commit
            state.scroll_up_page();
            assert_eq!(state.current_list_status(), (30, 5, 5));
            assert_eq!(state.selected_row(), None);
        });
    }

    /// Every offset changing operation stays within `[0, total - height]`, and the scrollbar
    /// stays on its track, for lists and viewports of any size.
    #[test]
    fn test_scrolling_never_leaves_the_valid_offset_range() {
        for total in 0..=7usize {
            for height in 0..=7usize {
                let subjects = vec!["commit"; total];
                with_commit_list_state(&subjects, |state| {
                    state.reset_height(height);
                    let max_offset = total.saturating_sub(height);
                    let operations: [fn(&mut CommitListState); 14] = [
                        |s| s.scroll_down(),
                        |s| s.scroll_up(),
                        |s| s.scroll_down_page(),
                        |s| s.scroll_up_page(),
                        |s| s.scroll_down_half(),
                        |s| s.scroll_up_half(),
                        |s| s.select_next(),
                        |s| s.select_prev(),
                        |s| s.select_first(),
                        |s| s.select_last(),
                        |s| s.select_high(),
                        |s| s.select_middle(),
                        |s| s.select_low(),
                        |s| s.set_offset(s.offset_for_thumb_start(u16::MAX)),
                    ];
                    for round in 0..3 {
                        for (i, operation) in operations.iter().enumerate() {
                            operation(state);
                            let (_, offset, _) = state.current_list_status();
                            assert!(
                                offset <= max_offset,
                                "total={total} height={height} round={round} operation={i}: \
                                 offset={offset} > max={max_offset}"
                            );
                            if let Some(geometry) = state.scrollbar_geometry() {
                                let track_end = geometry.thumb_start + geometry.thumb_len;
                                assert!(
                                    track_end <= height as u16,
                                    "total={total} height={height} round={round} operation={i}: \
                                     the thumb ends at {track_end}, past the track of {height}"
                                );
                            }
                        }
                    }
                });
            }
        }
    }

    #[test]
    fn test_scrollbar_drag_does_not_change_the_selected_commit() {
        with_commit_list_state(&["commit"; 20], |state| {
            state.reset_height(5);
            state.select_index(2);
            assert_eq!(state.current_list_status(), (2, 2, 5));

            // drag the thumb to the bottom of the track
            let geometry = state.scrollbar_geometry().unwrap();
            let thumb_start = 5 - geometry.thumb_len;
            state.set_offset(state.offset_for_thumb_start(thumb_start));
            assert_eq!(state.current_list_status(), (2, 15, 5));
            assert_eq!(state.selected_row(), None);

            // drag the thumb back to the top of the track
            state.set_offset(state.offset_for_thumb_start(0));
            assert_eq!(state.current_list_status(), (2, 0, 5));
            assert_eq!(state.selected_row(), Some(2));
        });
    }

    #[test]
    fn test_selecting_by_click_does_not_scroll_the_viewport() {
        with_commit_list_state(&["commit"; 20], |state| {
            state.reset_height(5);
            state.set_offset(5);
            assert_eq!(state.current_list_status(), (0, 5, 5));

            state.select_index_without_scroll(9);
            assert_eq!(state.current_list_status(), (9, 5, 5));
            assert_eq!(state.selected_row(), Some(4));

            state.select_index_without_scroll(5);
            assert_eq!(state.current_list_status(), (5, 5, 5));
            assert_eq!(state.selected_row(), Some(0));

            // out of range indexes are ignored
            state.select_index_without_scroll(20);
            assert_eq!(state.current_list_status(), (5, 5, 5));
        });
    }

    #[test]
    fn test_zero_scrolloff_preserves_page_scroll_behavior() {
        with_commit_list_state(&["commit"; 25], |state| {
            state.reset_height(10);
            state.select_index(12);
            assert_eq!(state.current_list_status(), (12, 12, 10));

            state.scroll_down_page();
            assert_eq!(state.current_list_status(), (22, 15, 10));
            state.scroll_down_page();
            assert_eq!(state.current_list_status(), (24, 15, 10));
        });
    }

    #[test]
    fn test_restore_search_recalculates_matches_with_applied_options() {
        let (context, options) = with_commit_list_state(&["Fix parser", "other"], |state| {
            input_search_query(state, "fx");
            state.toggle_ignore_case();
            state.toggle_fuzzy();
            state.apply_search();

            (
                state.search_refresh_context().unwrap(),
                state.search_options(),
            )
        });

        assert_eq!(context, SearchRefreshContext { query: "fx".into() });
        assert_eq!(
            options,
            SearchOptions {
                target: SearchTarget::All,
                ignore_case: true,
                fuzzy: true,
            }
        );

        with_commit_list_state(&["unrelated", "FIX new", "fix parser"], |state| {
            state.restore_search_options(options);
            state.restore_search(&context);

            assert_eq!(state.search_refresh_context(), Some(context.clone()));
            assert_eq!(
                state.commits[state.current_selected_index()].commit.subject,
                "unrelated"
            );

            state.select_next_match();
            assert_eq!(
                state.matched_query_string(),
                Some(("Match 1 of 2 (query: \"fx\")".into(), true))
            );
            assert_eq!(
                state.commits[state.current_selected_index()].commit.subject,
                "FIX new"
            );

            state.select_next_match();
            assert_eq!(
                state.matched_query_string(),
                Some(("Match 2 of 2 (query: \"fx\")".into(), true))
            );
            assert_eq!(
                state.commits[state.current_selected_index()].commit.subject,
                "fix parser"
            );
        });
    }

    #[test]
    fn test_restore_search_options_without_applied_search() {
        let options = with_commit_list_state(&["FIX"], |state| {
            state.toggle_ignore_case();
            state.search_options()
        });

        with_commit_list_state(&["FIX"], |state| {
            state.restore_search_options(options);
            input_search_query(state, "fix");
            state.apply_search();

            assert_eq!(
                state.matched_query_string(),
                Some(("Match 1 of 1 (query: \"fix\")".into(), true))
            );
        });
    }

    #[test]
    fn test_restore_search_keeps_selected_match_position() {
        let context = with_commit_list_state(&["fix"], |state| {
            input_search_query(state, "fix");
            state.apply_search();
            state.search_refresh_context().unwrap()
        });

        with_commit_list_state(&["first", "second", "fix", "last"], |state| {
            state.reset_height(2);
            state.select_index(2);
            state.scroll_up();
            assert_eq!(state.current_list_status(), (2, 1, 2));

            state.restore_search(&context);

            assert_eq!(state.current_list_status(), (2, 1, 2));
            assert_eq!(
                state.matched_query_string(),
                Some(("Match 1 of 1 (query: \"fix\")".into(), true))
            );
        });
    }

    #[test]
    fn test_search_options_are_reused_for_next_search() {
        with_commit_list_state(&["FIX"], |state| {
            input_search_query(state, "fix");
            state.toggle_ignore_case();
            state.apply_search();
            state.cancel_search();

            input_search_query(state, "fix");
            state.apply_search();

            assert_eq!(
                state.matched_query_string(),
                Some(("Match 1 of 1 (query: \"fix\")".into(), true))
            );
        });
    }

    #[test]
    fn test_resume_search_preserves_query_options_matches_and_view_position() {
        with_commit_list_state(&["other", "Fix parser", "FIX graph", "last"], |state| {
            state.reset_height(2);
            state.toggle_ignore_case();
            state.toggle_fuzzy();
            state.toggle_search_target();
            input_search_query(state, "fx");
            state.handle_search_input(KeyEvent::from(KeyCode::Left));
            state.apply_search();
            state.select_next_match();
            state.scroll_up();

            let options = state.search_options();
            let position = state.current_list_status();
            let result = state.matched_query_string();
            let matches: Vec<_> = state
                .search_matches
                .iter()
                .map(|m| m.subject.as_ref().map(|p| p.matched_indices.clone()))
                .collect();
            assert_eq!(result, Some(("Match 2 of 2 (query: \"fx\")".into(), true)));

            state.start_search();

            assert_eq!(state.search_query_string(), Some("/fx".into()));
            assert_eq!(state.search_query_cursor_position(), 3);
            assert_eq!(state.search_options(), options);
            assert_eq!(state.current_list_status(), position);
            assert_eq!(
                state
                    .search_matches
                    .iter()
                    .map(|m| m.subject.as_ref().map(|p| p.matched_indices.clone()))
                    .collect::<Vec<_>>(),
                matches
            );

            state.apply_search();
            assert_eq!(state.matched_query_string(), result);
            assert_eq!(state.current_list_status(), position);
        });
    }

    #[test]
    fn test_resume_search_edits_from_current_selection() {
        with_commit_list_state(
            &["fix parser older", "fix graph", "fix parser newer"],
            |state| {
                input_search_query(state, "fix");
                state.apply_search();
                state.select_next_match();
                assert_eq!(state.current_selected_index(), 1);

                state.start_search();
                for c in " parser".chars() {
                    state.handle_search_input(KeyEvent::from(KeyCode::Char(c)));
                }
                assert_eq!(state.current_selected_index(), 2);
                assert!(!state.search_matches[1].matched());
                assert!(state.search_matches[2].matched());

                state.handle_search_input(KeyEvent::from(KeyCode::Backspace));
                assert_eq!(state.search_query_string(), Some("/fix parse".into()));
                state.apply_search();
                assert_eq!(
                    state.matched_query_string(),
                    Some(("Match 2 of 2 (query: \"fix parse\")".into(), true))
                );
            },
        );
    }

    #[test]
    fn test_resume_search_does_not_move_selection_off_an_unmatched_commit() {
        with_commit_list_state(&["fix", "other", "fix again"], |state| {
            input_search_query(state, "fix");
            state.apply_search();
            state.select_next();
            let position = state.current_list_status();
            let result = state.matched_query_string();

            state.start_search();
            state.apply_search();

            assert_eq!(state.current_list_status(), position);
            assert_eq!(state.current_selected_index(), 1);
            assert_eq!(state.matched_query_string(), result);
        });
    }

    #[test]
    fn test_resume_search_without_matches_can_be_corrected() {
        with_commit_list_state(&["fix", "other"], |state| {
            input_search_query(state, "fixx");
            state.apply_search();
            let result = state.matched_query_string();

            state.start_search();
            assert_eq!(state.search_query_string(), Some("/fixx".into()));
            state.apply_search();
            assert_eq!(state.matched_query_string(), result);

            state.start_search();
            state.handle_search_input(KeyEvent::from(KeyCode::Backspace));
            state.apply_search();
            assert_eq!(
                state.matched_query_string(),
                Some(("Match 1 of 1 (query: \"fix\")".into(), true))
            );
        });
    }

    #[test]
    fn test_cancel_resumed_search_starts_next_search_empty() {
        with_commit_list_state(&["fix", "other"], |state| {
            input_search_query(state, "fix");
            state.apply_search();
            state.start_search();
            state.cancel_search();

            assert_eq!(state.search_state(), SearchState::Inactive);
            assert!(state.search_matches.iter().all(|m| !m.matched()));
            state.start_search();
            assert_eq!(state.search_query_string(), Some("/".into()));
        });
    }

    #[test]
    fn test_clear_resumed_query_clears_matches_and_applies_inactive_search() {
        for fuzzy in [false, true] {
            with_commit_list_state(&["fix", "other"], |state| {
                if fuzzy {
                    state.toggle_fuzzy();
                }
                input_search_query(state, "fix");
                state.apply_search();
                state.start_search();
                state.handle_search_input(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));

                assert_eq!(state.search_query_string(), Some("/".into()));
                assert!(state.search_matches.iter().all(|m| !m.matched()));
                state.apply_search();
                assert_eq!(state.search_state(), SearchState::Inactive);
                assert_eq!(state.matched_query_string(), None);

                state.start_search();
                assert_eq!(state.search_query_string(), Some("/".into()));
                state.handle_search_input(KeyEvent::from(KeyCode::Char('o')));
                state.apply_search();
                assert_eq!(
                    state.matched_query_string(),
                    Some(("Match 1 of 1 (query: \"o\")".into(), true))
                );
            });
        }
    }

    #[test]
    fn test_resume_search_moves_cursor_to_end_of_long_unicode_query() {
        let query = "検索".repeat(40);
        with_commit_list_state(&[query.as_str()], |state| {
            input_search_query(state, &query);
            state.handle_search_input(KeyEvent::from(KeyCode::Home));
            state.apply_search();

            state.start_search();
            assert_eq!(state.search_query_string(), Some(format!("/{query}")));
            assert_eq!(state.search_query_cursor_position(), 161);
            state.handle_search_input(KeyEvent::from(KeyCode::Char('追')));
            assert_eq!(state.search_query_string(), Some(format!("/{query}追")));
        });
    }

    #[test]
    fn test_search_options_can_be_changed_before_search() {
        with_commit_list_state(&["FIX"], |state| {
            state.toggle_ignore_case();
            input_search_query(state, "fix");
            state.apply_search();

            assert_eq!(
                state.matched_query_string(),
                Some(("Match 1 of 1 (query: \"fix\")".into(), true))
            );
        });
    }

    #[test]
    fn test_search_option_string_after_toggles() {
        with_commit_list_state(&["fix"], |state| {
            state.toggle_ignore_case();
            assert_eq!(
                state.search_options().status_string(),
                "[all] [ignore-case] [substring]"
            );
            state.toggle_ignore_case();
            assert_eq!(
                state.search_options().status_string(),
                "[all] [case-sensitive] [substring]"
            );
            state.toggle_fuzzy();
            assert_eq!(
                state.search_options().status_string(),
                "[all] [case-sensitive] [fuzzy]"
            );
            state.toggle_fuzzy();
            assert_eq!(
                state.search_options().status_string(),
                "[all] [case-sensitive] [substring]"
            );
            state.toggle_search_target();
            assert_eq!(
                state.search_options().status_string(),
                "[subject] [case-sensitive] [substring]"
            );
        });
    }

    #[test]
    fn test_search_target_matches_only_the_selected_field() {
        let commit = Commit {
            commit_hash: CommitHash::from("abcdef0123456789abcdef0123456789abcdef01"),
            subject: "subject-match".into(),
            author_name: "author-match".into(),
            ..Commit::default()
        };
        let reference = Ref::Branch {
            name: "ref-match".into(),
            target: commit.commit_hash.clone(),
        };
        let refs = [&reference];
        let cases = [
            (SearchTarget::All, "subject-match", true),
            (SearchTarget::All, "author-match", true),
            (SearchTarget::All, "ref-match", true),
            (SearchTarget::All, "abcdef0", true),
            (SearchTarget::Subject, "subject-match", true),
            (SearchTarget::Subject, "author-match", false),
            (SearchTarget::Author, "author-match", true),
            (SearchTarget::Author, "ref-match", false),
            (SearchTarget::Ref, "ref-match", true),
            (SearchTarget::Ref, "abcdef0", false),
            (SearchTarget::Hash, "abcdef0", true),
            (SearchTarget::Hash, "subject-match", false),
        ];

        for (target, query, expected) in cases {
            let matcher = SearchMatcher::new(query, false, false);
            let mut search_match = SearchMatch::default();
            search_match.set(&commit, &refs, &matcher, target);
            assert_eq!(search_match.matched(), expected, "{target:?}: {query}");
        }
    }

    #[test]
    fn test_search_target_change_clears_previous_field_matches() {
        let commit = Commit {
            commit_hash: CommitHash::from("abcdef0123456789abcdef0123456789abcdef01"),
            subject: "match".into(),
            author_name: "match".into(),
            ..Commit::default()
        };
        let reference = Ref::Branch {
            name: "match".into(),
            target: commit.commit_hash.clone(),
        };
        let refs = [&reference];
        let matcher = SearchMatcher::new("match", false, false);
        let mut search_match = SearchMatch::default();

        search_match.set(&commit, &refs, &matcher, SearchTarget::All);
        assert!(!search_match.refs.is_empty());
        assert!(search_match.subject.is_some());
        assert!(search_match.author_name.is_some());

        search_match.set(&commit, &refs, &matcher, SearchTarget::Hash);
        assert!(search_match.refs.is_empty());
        assert!(search_match.subject.is_none());
        assert!(search_match.author_name.is_none());
    }

    #[test]
    fn test_applied_search_target_toggle_recalculates_matches() {
        with_commit_list_state(&["fix", "other"], |state| {
            input_search_query(state, "fix");
            state.apply_search();

            state.toggle_search_target();
            assert_eq!(state.search_options().target, SearchTarget::Subject);
            assert_eq!(
                state.matched_query_string(),
                Some(("Match 1 of 1 (query: \"fix\")".into(), true))
            );

            state.toggle_search_target();
            assert_eq!(state.search_options().target, SearchTarget::Author);
            assert_eq!(
                state.matched_query_string(),
                Some(("No matches found (query: \"fix\")".into(), false))
            );
        });
    }

    #[test]
    fn test_applied_search_options_keep_selected_match() {
        with_commit_list_state(&["FIX", "fix"], |state| {
            input_search_query(state, "fix");
            state.apply_search();
            state.toggle_ignore_case();

            assert_eq!(
                state.commits[state.current_selected_index()].commit.subject,
                "fix"
            );
            assert_eq!(
                state.matched_query_string(),
                Some(("Match 2 of 2 (query: \"fix\")".into(), true))
            );
        });
    }

    #[test]
    fn test_applied_search_options_select_next_match() {
        with_commit_list_state(&["FIX", "fix"], |state| {
            state.toggle_ignore_case();
            input_search_query(state, "fix");
            state.apply_search();
            state.toggle_ignore_case();

            assert_eq!(
                state.commits[state.current_selected_index()].commit.subject,
                "fix"
            );
            assert_eq!(
                state.matched_query_string(),
                Some(("Match 1 of 1 (query: \"fix\")".into(), true))
            );
        });
    }

    #[test]
    fn test_applied_search_options_keep_selection_when_no_matches() {
        with_commit_list_state(&["fix", "other"], |state| {
            state.toggle_fuzzy();
            input_search_query(state, "fx");
            state.apply_search();
            let selected = state.current_selected_index();

            state.toggle_fuzzy();

            assert_eq!(state.current_selected_index(), selected);
            assert_eq!(
                state.matched_query_string(),
                Some(("No matches found (query: \"fx\")".into(), false))
            );
        });
    }

    #[test]
    fn test_calc_cell_widths_all_columns() {
        let area_width = 80;
        let subject_min_width = 20;
        let graph_width = 6;
        let name_width = 10;
        let date_width = 15;
        let columns = vec![
            UserListColumnType::Graph,
            UserListColumnType::Marker,
            UserListColumnType::Subject,
            UserListColumnType::Name,
            UserListColumnType::Hash,
            UserListColumnType::Date,
        ];

        let actual = calc_cell_widths(
            area_width,
            subject_min_width,
            graph_width,
            name_width,
            date_width,
            &columns,
        );

        let expected = vec![
            Constraint::Length(6),  // Graph
            Constraint::Length(1),  // Marker
            Constraint::Min(0),     // Subject
            Constraint::Length(12), // Name (10 + 2 pad)
            Constraint::Length(9),  // Hash (7 + 2 pad)
            Constraint::Length(17), // Date (15 + 2 pad)
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_calc_cell_width_all_columns_small_area_remove_name_date_hash() {
        let area_width = 30;
        let subject_min_width = 20;
        let graph_width = 6;
        let name_width = 10;
        let date_width = 15;
        let columns = vec![
            UserListColumnType::Graph,
            UserListColumnType::Marker,
            UserListColumnType::Subject,
            UserListColumnType::Name,
            UserListColumnType::Hash,
            UserListColumnType::Date,
        ];

        let actual = calc_cell_widths(
            area_width,
            subject_min_width,
            graph_width,
            name_width,
            date_width,
            &columns,
        );

        // Graph + Marker + Subject + Hash = 6 + 1 + 20 + 9 = 36 > 30
        // => Name, Date, and Hash are removed
        let expected = vec![
            Constraint::Length(6), // Graph
            Constraint::Length(1), // Marker
            Constraint::Min(0),    // Subject
            Constraint::Length(0), // Name removed
            Constraint::Length(0), // Hash removed
            Constraint::Length(0), // Date removed
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_calc_cell_width_all_columns_small_area_remove_name_date() {
        let area_width = 40;
        let subject_min_width = 20;
        let graph_width = 6;
        let name_width = 10;
        let date_width = 15;
        let columns = vec![
            UserListColumnType::Graph,
            UserListColumnType::Marker,
            UserListColumnType::Subject,
            UserListColumnType::Name,
            UserListColumnType::Hash,
            UserListColumnType::Date,
        ];

        let actual = calc_cell_widths(
            area_width,
            subject_min_width,
            graph_width,
            name_width,
            date_width,
            &columns,
        );

        // Graph + Marker + Subject + Hash = 6 + 1 + 20 + 9 = 36
        // Graph + Marker + Subject + Date + Hash = 6 + 1 + 20 + 17 + 9 = 53 > 40
        // => Name and Date are removed
        let expected = vec![
            Constraint::Length(6), // Graph
            Constraint::Length(1), // Marker
            Constraint::Min(0),    // Subject
            Constraint::Length(0), // Name removed
            Constraint::Length(9), // Hash (7 + 2 pad)
            Constraint::Length(0), // Date removed
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_calc_cell_width_all_columns_small_area_remove_name() {
        let area_width = 60;
        let subject_min_width = 20;
        let graph_width = 6;
        let name_width = 10;
        let date_width = 15;
        let columns = vec![
            UserListColumnType::Graph,
            UserListColumnType::Marker,
            UserListColumnType::Subject,
            UserListColumnType::Name,
            UserListColumnType::Hash,
            UserListColumnType::Date,
        ];

        let actual = calc_cell_widths(
            area_width,
            subject_min_width,
            graph_width,
            name_width,
            date_width,
            &columns,
        );

        // Graph + Marker + Subject + Date + Hash = 6 + 1 + 20 + 17 + 9 = 53 <= 60
        // Graph + Marker + Subject + Name + Date + Hash = 6 + 1 + 20 + 12 + 17 + 9 = 65 > 60
        // => Name is removed
        let expected = vec![
            Constraint::Length(6),  // Graph
            Constraint::Length(1),  // Marker
            Constraint::Min(0),     // Subject
            Constraint::Length(0),  // Name removed
            Constraint::Length(9),  // Hash (7 + 2 pad)
            Constraint::Length(17), // Date (15 + 2 pad)
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_calc_cell_width_columns_order() {
        let area_width = 80;
        let subject_min_width = 20;
        let graph_width = 6;
        let name_width = 10;
        let date_width = 15;
        let columns = vec![
            UserListColumnType::Date,
            UserListColumnType::Subject,
            UserListColumnType::Hash,
            UserListColumnType::Graph,
        ];

        let actual = calc_cell_widths(
            area_width,
            subject_min_width,
            graph_width,
            name_width,
            date_width,
            &columns,
        );

        let expected = vec![
            Constraint::Length(17), // Date (15 + 2 pad)
            Constraint::Min(0),     // Subject
            Constraint::Length(9),  // Hash (7 + 2 pad)
            Constraint::Length(6),  // Graph
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_right_aligned_name_pads_a_short_name() {
        let (name, padding, truncate) = right_aligned_name("author", 12);
        assert_eq!(name, "author");
        assert_eq!(padding, 6);
        assert!(!truncate);
        // The cell is filled even after right alignment
        assert_eq!(padding + console::measure_text_width(&name), 12);
    }

    #[test]
    fn test_right_aligned_name_does_not_pad_a_name_filling_the_cell() {
        let (name, padding, truncate) = right_aligned_name("author_name!", 12);
        assert_eq!(name, "author_name!");
        assert_eq!(padding, 0);
        assert!(!truncate);
    }

    #[test]
    fn test_right_aligned_name_truncates_a_long_name_without_padding() {
        let (name, padding, truncate) = right_aligned_name("a-very-long-author-name", 12);
        assert_eq!(name, "a-very-lo...");
        assert_eq!(padding, 0);
        assert!(truncate);
    }

    #[test]
    fn test_right_aligned_name_pads_by_display_width() {
        // "山田" is 4 columns wide, but 6 bytes
        let (name, padding, truncate) = right_aligned_name("山田", 12);
        assert_eq!(name, "山田");
        assert_eq!(padding, 8);
        assert!(!truncate);
    }

    fn commit(hash: u8) -> Commit {
        Commit {
            commit_hash: CommitHash::from(format!("{hash:040x}").as_str()),
            subject: "subject".into(),
            ..Commit::default()
        }
    }

    fn branch(name: &str) -> Ref {
        Ref::Branch {
            name: name.into(),
            target: CommitHash::default(),
        }
    }

    fn remote_branch(name: &str) -> Ref {
        Ref::RemoteBranch {
            name: name.into(),
            target: CommitHash::default(),
        }
    }

    fn tag(name: &str) -> Ref {
        Ref::Tag {
            name: name.into(),
            target: CommitHash::default(),
        }
    }

    fn text_of(spans: &[Span]) -> String {
        spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn test_refs_are_rendered_as_badges_separated_by_spaces() {
        let commit = commit(1);
        let refs = [branch("main"), remote_branch("origin/main"), tag("v1.0")];
        let info = CommitInfo::new(&commit, refs.iter().collect(), Color::Reset);
        let head = Head::Branch {
            name: "main".into(),
        };
        let theme = ColorTheme::default();
        let matches = FxHashMap::default();

        let spans = refs_spans(&info, &head, &matches, &theme);

        let text = text_of(&spans.spans);
        // HEAD is marked with a dot in front of the branch it points to, not with its name
        assert_eq!(text, "● main origin/main v1.0 ");
        assert!(
            !text.contains([',', '(', ')', ';']),
            "the refs are separated by spaces, not by commas or brackets"
        );
    }

    #[test]
    fn test_the_refs_are_grouped_by_type() {
        let commit = commit(1);
        // the refs are loaded in an arbitrary order
        let refs = [
            tag("v1.0"),
            remote_branch("origin/main"),
            branch("main"),
            tag("v0.9"),
            remote_branch("upstream/main"),
            branch("develop"),
        ];
        let info = CommitInfo::new(&commit, refs.iter().collect(), Color::Reset);
        let theme = ColorTheme::default();
        let matches = FxHashMap::default();

        let spans = refs_spans(&info, &Head::None, &matches, &theme);

        // The local branches come first, then the remote branches, and the tags last. The refs of a
        // group keep the order they are loaded in.
        assert_eq!(
            text_of(&spans.spans),
            "main develop origin/main upstream/main v1.0 v0.9 "
        );
        assert_eq!(
            spans
                .names
                .iter()
                .map(|name| name.r.name())
                .collect::<Vec<_>>(),
            vec![
                "main",
                "develop",
                "origin/main",
                "upstream/main",
                "v1.0",
                "v0.9"
            ]
        );
    }

    #[test]
    fn test_the_refs_of_a_detached_head_are_grouped_by_type() {
        let commit = commit(1);
        let refs = [tag("v1.0"), remote_branch("origin/main"), branch("main")];
        let info = CommitInfo::new(&commit, refs.iter().collect(), Color::Reset);
        let head = Head::Detached {
            target: commit.commit_hash.clone(),
        };
        let theme = ColorTheme::default();
        let matches = FxHashMap::default();

        let spans = refs_spans(&info, &head, &matches, &theme);

        assert_eq!(text_of(&spans.spans), "● main origin/main v1.0 ");
        assert_eq!(spans.names.len(), 3);
    }

    #[test]
    fn test_a_single_stash_keeps_its_own_format() {
        let commit = commit(1);
        let refs = [Ref::Stash {
            name: "stash@{0}".into(),
            message: "WIP".into(),
            target: CommitHash::default(),
        }];
        let info = CommitInfo::new(&commit, refs.iter().collect(), Color::Reset);
        let head = Head::None;
        let theme = ColorTheme::default();
        let matches = FxHashMap::default();

        let spans = refs_spans(&info, &head, &matches, &theme);

        assert_eq!(text_of(&spans.spans), "stash@{0} ");
        assert_eq!(spans.spans[0].style.fg, Some(theme.list_ref_stash_fg));
        assert!(!spans.spans[0]
            .style
            .add_modifier
            .contains(Modifier::REVERSED));
        assert!(spans.names.is_empty(), "a stash cannot be switched to");
    }

    #[test]
    fn test_a_ref_badge_is_reversed_and_keeps_the_color_of_its_type() {
        let commit = commit(1);
        let refs = [branch("main"), remote_branch("origin/main"), tag("v1.0")];
        let info = CommitInfo::new(&commit, refs.iter().collect(), Color::Reset);
        let head = Head::Branch {
            name: "main".into(),
        };
        let theme = ColorTheme::default();
        let matches = FxHashMap::default();

        let spans = refs_spans(&info, &head, &matches, &theme);

        let badges: Vec<(String, Color)> = spans
            .spans
            .iter()
            .filter(|span| span.style.add_modifier.contains(Modifier::REVERSED))
            .map(|span| (span.content.to_string(), span.style.fg.unwrap()))
            .collect();
        assert_eq!(
            badges,
            vec![
                ("main".to_string(), theme.list_ref_branch_fg),
                ("origin/main".to_string(), theme.list_ref_remote_branch_fg),
                ("v1.0".to_string(), theme.list_ref_tag_fg),
            ]
        );

        // the marker of HEAD is not a badge
        let marker = &spans.spans[0];
        assert_eq!(marker.content.as_ref(), HEAD_MARKER);
        assert_eq!(marker.style.fg, Some(theme.list_head_fg));
        assert!(!marker.style.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn test_the_head_marker_is_a_single_cell() {
        // The marker shifts the badges that follow it, so it must be exactly one cell wide
        assert_eq!(Span::raw(HEAD_MARKER).width(), 1);
    }

    #[test]
    fn test_the_head_marker_is_in_front_of_the_branch_that_head_points_to() {
        let commit = commit(1);
        let refs = [branch("develop"), branch("main"), tag("v1.0")];
        let info = CommitInfo::new(&commit, refs.iter().collect(), Color::Reset);
        let head = Head::Branch {
            name: "main".into(),
        };
        let theme = ColorTheme::default();
        let matches = FxHashMap::default();

        let spans = refs_spans(&info, &head, &matches, &theme);

        // the marker is not at the beginning of the refs: it follows the badge of another branch
        assert_eq!(text_of(&spans.spans), "develop ● main v1.0 ");
        assert_eq!(spans.names.len(), 3);
        assert_eq!(spans.names[0].r.name(), "develop");
        assert_eq!(spans.names[0].offset, 0);
        // "develop " then the marker and its separator
        assert_eq!(spans.names[1].r.name(), "main");
        assert_eq!(spans.names[1].offset, "develop ".len() + 2);
        assert_eq!(spans.names[2].r.name(), "v1.0");
    }

    #[test]
    fn test_the_head_marker_of_a_detached_commit_is_separated_from_the_refs_by_a_space() {
        let commit = commit(1);
        let refs = [tag("v1.0")];
        let info = CommitInfo::new(&commit, refs.iter().collect(), Color::Reset);
        let head = Head::Detached {
            target: commit.commit_hash.clone(),
        };
        let theme = ColorTheme::default();
        let matches = FxHashMap::default();

        let spans = refs_spans(&info, &head, &matches, &theme);

        assert_eq!(text_of(&spans.spans), "● v1.0 ");
        assert_eq!(spans.spans[0].content.as_ref(), HEAD_MARKER);
        assert_eq!(spans.spans[0].style.fg, Some(theme.list_head_fg));
        assert_eq!(spans.names.len(), 1);
        // the marker and the space that separates it from the first badge
        assert_eq!(spans.names[0].offset, 2);
        assert_eq!(spans.names[0].width, "v1.0".len());
    }

    #[test]
    fn test_the_head_marker_of_a_detached_commit_without_refs_is_followed_by_a_space() {
        let commit = commit(1);
        let info = CommitInfo::new(&commit, vec![], Color::Reset);
        let head = Head::Detached {
            target: commit.commit_hash.clone(),
        };
        let theme = ColorTheme::default();
        let matches = FxHashMap::default();

        let spans = refs_spans(&info, &head, &matches, &theme);

        // The subject must not stick to the marker of a detached HEAD
        assert_eq!(text_of(&spans.spans), "● ");
        assert!(spans.names.is_empty());
    }

    #[test]
    fn test_only_the_branch_that_head_points_to_gets_the_head_marker() {
        let commit = commit(1);
        // a tag can have the same name as the branch that HEAD points to
        let refs = [tag("main"), branch("main")];
        let info = CommitInfo::new(&commit, refs.iter().collect(), Color::Reset);
        let head = Head::Branch {
            name: "main".into(),
        };
        let theme = ColorTheme::default();
        let matches = FxHashMap::default();

        let spans = refs_spans(&info, &head, &matches, &theme);

        assert_eq!(text_of(&spans.spans), "● main main ");
        assert_eq!(spans.names.len(), 2, "both refs are badges");
        assert!(
            matches!(spans.names[0].r, Ref::Branch { .. }),
            "the branch is rendered first, and the marker belongs to it"
        );
        assert_eq!(spans.names[0].offset, 2, "the marker and its separator");
        assert!(matches!(spans.names[1].r, Ref::Tag { .. }));

        // a remote branch is not the branch that HEAD points to, even with the same name
        let refs = [remote_branch("main")];
        let info = CommitInfo::new(&commit, refs.iter().collect(), Color::Reset);
        let head = Head::Branch {
            name: "main".into(),
        };

        let spans = refs_spans(&info, &head, &matches, &theme);

        assert_eq!(text_of(&spans.spans), "main ");
        assert_eq!(spans.names[0].offset, 0);
    }

    #[test]
    fn test_a_commit_without_a_ref_has_no_span() {
        let commit = commit(1);
        let info = CommitInfo::new(&commit, Vec::new(), Color::Reset);
        let head = Head::Branch {
            name: "main".into(),
        };
        let theme = ColorTheme::default();
        let matches = FxHashMap::default();

        let spans = refs_spans(&info, &head, &matches, &theme);

        assert!(text_of(&spans.spans).is_empty());
        assert!(spans.names.is_empty());
    }

    #[test]
    fn test_a_searched_ref_is_still_a_single_badge() {
        let commit = commit(1);
        let refs = [branch("feature")];
        let info = CommitInfo::new(&commit, refs.iter().collect(), Color::Reset);
        let head = Head::None;
        let theme = ColorTheme::default();
        let matches = FxHashMap::from_iter([(
            "feature".to_string(),
            SearchMatchPosition::new((0..3).collect()),
        )]);

        let spans = refs_spans(&info, &head, &matches, &theme);

        // the highlighted name is split into several spans, but it stays one badge
        assert!(spans.spans.len() > 1, "the match splits the name");
        assert!(spans
            .spans
            .iter()
            .filter(|span| span.content.as_ref() != " ")
            .all(|span| span.style.add_modifier.contains(Modifier::REVERSED)));
        assert_eq!(spans.names.len(), 1);
        assert_eq!(spans.names[0].offset, 0);
        assert_eq!(spans.names[0].width, "feature".len());
    }

    #[test]
    fn test_the_head_marker_shifts_the_rectangles_of_the_badges() {
        let commit = commit(1);
        let refs = [branch("main"), remote_branch("origin/main")];
        let info = CommitInfo::new(&commit, refs.iter().collect(), Color::Reset);
        let head = Head::Branch {
            name: "main".into(),
        };
        let theme = ColorTheme::default();
        let matches = FxHashMap::default();

        let spans = refs_spans(&info, &head, &matches, &theme);
        assert_eq!(text_of(&spans.spans), "● main origin/main ");

        let area = Rect::new(10, 3, 40, 1);
        // the item starts with a space, then the marker and its separator
        assert_eq!(
            ref_badge_rect(area, 0, spans.names[0].offset, spans.names[0].width),
            Rect::new(13, 3, 4, 1),
            "the badge of the branch that HEAD points to follows the marker"
        );
        assert_eq!(
            ref_badge_rect(area, 0, spans.names[1].offset, spans.names[1].width),
            Rect::new(18, 3, 11, 1),
            "the badges that follow the marker are shifted by it as well"
        );
    }

    #[test]
    fn test_ref_badge_rect_of_a_row_with_several_refs() {
        let area = Rect::new(10, 3, 40, 5);
        // the item starts with a space, then the badges
        assert_eq!(
            ref_badge_rect(area, 0, 0, 4),
            Rect::new(11, 3, 4, 1),
            "the first badge starts after the space that precedes the refs"
        );
        assert_eq!(
            ref_badge_rect(area, 1, 5, 11),
            Rect::new(16, 4, 11, 1),
            "the second badge follows the first one and its separator"
        );
    }

    #[test]
    fn test_ref_badge_rect_is_clipped_to_the_column() {
        let area = Rect::new(10, 3, 20, 5);
        // the badge starts at 16 and would end at 31, but the column ends at 30
        assert_eq!(ref_badge_rect(area, 0, 5, 15), Rect::new(16, 3, 14, 1));
        // a badge that is completely beyond the column is not visible
        assert_eq!(ref_badge_rect(area, 0, 30, 5), Rect::new(41, 3, 0, 1));
    }

    #[test]
    fn test_ref_badge_at_finds_the_badge_under_the_cursor() {
        with_commit_list_state(&["a", "b"], |state| {
            state.set_ref_badges(vec![
                RefBadge {
                    rect: Rect::new(11, 3, 4, 1),
                    r: branch("main"),
                },
                RefBadge {
                    rect: Rect::new(16, 3, 11, 1),
                    r: remote_branch("origin/main"),
                },
            ]);

            assert_eq!(
                state.ref_badge_at(11, 3).map(|badge| badge.r.name()),
                Some("main")
            );
            assert_eq!(
                state.ref_badge_at(14, 3).map(|badge| badge.r.name()),
                Some("main"),
                "the last cell of a badge is part of it"
            );
            assert_eq!(
                state.ref_badge_at(16, 3).map(|badge| badge.r.name()),
                Some("origin/main")
            );
            assert_eq!(
                state.ref_badge_at(26, 3).map(|badge| badge.r.name()),
                Some("origin/main")
            );

            // the space between two badges
            assert!(state.ref_badge_at(15, 3).is_none());
            // outside of the badges
            assert!(state.ref_badge_at(10, 3).is_none());
            assert!(state.ref_badge_at(27, 3).is_none());
            // another row
            assert!(state.ref_badge_at(11, 4).is_none());
            assert!(state.ref_badge_at(11, 2).is_none());
        });
    }

    #[test]
    fn test_ref_badge_at_without_a_recorded_badge() {
        with_commit_list_state(&["a"], |state| {
            assert!(state.ref_badge_at(11, 3).is_none());
        });
    }

    fn badge_click(rect: Rect, at: Instant) -> Option<BadgeClick> {
        Some(BadgeClick { rect, at })
    }

    #[test]
    fn test_a_second_click_on_the_same_badge_is_a_double_click() {
        let rect = Rect::new(11, 3, 4, 1);
        let now = Instant::now();
        assert!(is_double_click(
            badge_click(rect, now),
            badge_click(rect, now + Duration::from_millis(1))
        ));
        assert!(
            is_double_click(
                badge_click(rect, now),
                badge_click(rect, now + DOUBLE_CLICK_DELAY)
            ),
            "a click at the end of the delay is still a double click"
        );
    }

    #[test]
    fn test_a_click_on_another_badge_is_not_a_double_click() {
        let now = Instant::now();
        assert!(!is_double_click(
            badge_click(Rect::new(11, 3, 4, 1), now),
            badge_click(Rect::new(16, 3, 11, 1), now)
        ));
        assert!(
            !is_double_click(
                badge_click(Rect::new(11, 3, 4, 1), now),
                badge_click(Rect::new(11, 4, 4, 1), now)
            ),
            "the same badge on another row is another badge"
        );
    }

    #[test]
    fn test_a_click_after_the_delay_is_not_a_double_click() {
        let rect = Rect::new(11, 3, 4, 1);
        let now = Instant::now();
        assert!(!is_double_click(
            badge_click(rect, now),
            badge_click(rect, now + DOUBLE_CLICK_DELAY + Duration::from_millis(1))
        ));
    }

    #[test]
    fn test_a_click_that_is_not_on_a_badge_is_not_a_double_click() {
        let rect = Rect::new(11, 3, 4, 1);
        let now = Instant::now();
        assert!(!is_double_click(badge_click(rect, now), None));
        assert!(!is_double_click(None, badge_click(rect, now)));
        assert!(!is_double_click(None, None));
    }

    fn test_app_context() -> Rc<AppContext> {
        test_app_context_with(UiCommitListConfig::default())
    }

    /// The context of a commit list whose optional columns are set as given.
    fn test_app_context_with(commit_list_config: UiCommitListConfig) -> Rc<AppContext> {
        let ui_config = UiConfig::default();
        Rc::new(AppContext {
            keybind: KeyBind::default(),
            core_config: CoreConfig::default(),
            ui_config,
            actions_config: ActionsConfig::default(),
            color_theme: ColorTheme::default(),
            image_protocol: ImageProtocol::Iterm2,
            commit_list_config: Cell::new(commit_list_config),
        })
    }

    /// Renders the commit list on a test buffer, and gives the buffer and the recorded more
    /// button column to the closure.
    fn with_rendered_commit_list<R>(
        subjects: &[&str],
        width: u16,
        height: u16,
        more_button: bool,
        hovered: Option<HoveredCommit>,
        f: impl FnOnce(&Buffer, Option<u16>) -> R,
    ) -> R {
        with_commit_list_state(subjects, |state| {
            let area = Rect::new(0, 0, width, height);
            let mut buf = Buffer::empty(area);
            state.ensure_visible_graph_uploaded();
            let commit_list = CommitList::new(test_app_context())
                .scrollbar(true)
                .more_button(more_button)
                .hovered(hovered);
            commit_list.render(area, &mut buf, state);
            f(&buf, state.more_button_column())
        })
    }

    #[test]
    fn test_the_more_button_column_is_reserved_at_the_left_of_the_scrollbar() {
        let content = Rect::new(10, 3, 20, 5);
        assert_eq!(
            split_more_button_column(content, true),
            (Rect::new(10, 3, 19, 5), Some(29))
        );
        assert_eq!(
            split_more_button_column(content, false),
            (content, None),
            "the column is not reserved when the more button is disabled"
        );
    }

    #[test]
    fn test_the_more_button_column_is_not_reserved_when_there_is_no_room() {
        for area in [Rect::new(10, 3, 0, 5), Rect::new(10, 3, 1, 5)] {
            assert_eq!(
                split_more_button_column(area, true),
                (area, None),
                "{area:?}"
            );
        }
        assert_eq!(
            split_more_button_column(Rect::new(0, 0, 2, 1), true),
            (Rect::new(0, 0, 1, 1), Some(1)),
            "a column is still reserved when only one column is left for the content"
        );
    }

    #[test]
    fn test_the_more_button_is_rendered_only_on_the_hovered_row() {
        let buffer = with_rendered_commit_list(
            &["a", "b", "c"],
            60,
            5,
            true,
            Some(HoveredCommit {
                index: 1,
                armed: false,
            }),
            |buf, column| {
                assert_eq!(column, Some(58));
                buf.clone()
            },
        );
        assert_eq!(buffer[(58, 1)].symbol(), MORE_BUTTON);
        assert_eq!(buffer[(58, 0)].symbol(), " ");
        assert_eq!(buffer[(58, 2)].symbol(), " ");
        assert_eq!(buffer[(58, 3)].symbol(), " ", "a row without a commit");
    }

    #[test]
    fn test_the_more_button_is_not_rendered_without_a_hovered_row() {
        let buffer =
            with_rendered_commit_list(&["a", "b", "c"], 60, 5, true, None, |buf, _| buf.clone());
        for row in 0..5 {
            assert_eq!(buffer[(58, row)].symbol(), " ", "row {row}");
        }
    }

    #[test]
    fn test_the_more_button_column_is_not_reserved_when_the_more_button_is_disabled() {
        let buffer = with_rendered_commit_list(
            &["a", "b", "c"],
            60,
            5,
            false,
            Some(HoveredCommit {
                index: 1,
                armed: false,
            }),
            |buf, column| {
                assert_eq!(column, None);
                buf.clone()
            },
        );
        for row in 0..5 {
            assert_eq!(buffer[(58, row)].symbol(), " ", "row {row}");
        }
    }

    #[test]
    fn test_the_more_button_bar_keeps_the_background_of_the_selected_row() {
        let buffer = with_rendered_commit_list(
            &["a", "b", "c"],
            60,
            5,
            true,
            Some(HoveredCommit {
                index: 1,
                armed: false,
            }),
            |buf, _| buf.clone(),
        );
        let selected = buffer[(58, 0)].style();
        assert_eq!(selected.bg, Some(Color::DarkGray));
        assert_eq!(selected.fg, Some(Color::White));
        assert_eq!(
            buffer[(58, 1)].style().bg,
            Some(Color::Reset),
            "a row that is not selected has no background"
        );
    }

    #[test]
    fn test_the_armed_more_button_is_reversed() {
        let buffer = with_rendered_commit_list(
            &["a", "b", "c"],
            60,
            5,
            true,
            Some(HoveredCommit {
                index: 1,
                armed: true,
            }),
            |buf, _| buf.clone(),
        );
        assert_eq!(buffer[(58, 1)].symbol(), MORE_BUTTON);
        assert!(buffer[(58, 1)]
            .style()
            .add_modifier
            .contains(Modifier::REVERSED));
        assert_eq!(buffer[(58, 2)].style().add_modifier, Modifier::empty());
    }

    #[test]
    fn test_the_more_button_is_not_rendered_when_the_list_is_too_narrow() {
        for width in 0..=2 {
            with_rendered_commit_list(&["a"], width, 3, true, None, |buf, column| {
                assert_eq!(column, None, "width {width}");
                assert!(!buf.content.iter().any(|cell| cell.symbol() == MORE_BUTTON));
            });
        }
    }

    /// Renders the commit list of a repository whose first commit carries the given refs, and whose
    /// commits have long author names, on a test buffer.
    ///
    /// The badges and the author names compete with the other columns for the width of the row,
    /// which is the case where the more button must stay whole.
    fn with_rendered_commit_list_with_refs<R>(
        subjects: &[&str],
        refs: &[Ref],
        width: u16,
        height: u16,
        hovered: Option<HoveredCommit>,
        f: impl FnOnce(&Buffer, Option<u16>) -> R,
    ) -> R {
        let commits: Vec<Commit> = subjects
            .iter()
            .enumerate()
            .map(|(i, subject)| Commit {
                commit_hash: CommitHash::from(format!("{:040x}", i + 1).as_str()),
                subject: (*subject).into(),
                author_name: format!("A rather long author name {i}"),
                ..Commit::default()
            })
            .collect();
        let commit_hashes = commits.iter().map(|c| c.commit_hash.clone()).collect();
        let commit_map = commits
            .into_iter()
            .map(|c| (c.commit_hash.clone(), c))
            .collect();
        let repository = Repository::new(
            PathBuf::new(),
            commit_map,
            FxHashMap::default(),
            FxHashMap::default(),
            FxHashMap::default(),
            Head::None,
            commit_hashes,
        );
        let graph = calc_graph(&repository, None);
        let graph_color_set = GraphColorSet::new(&GraphColorConfig::default());
        let graph_image_manager = GraphImageManager::new(
            &graph,
            &graph_color_set,
            CellWidthType::Double,
            GraphStyle::Rounded,
            GraphImageWidthMode::Compact,
            ImageProtocol::Iterm2,
        );
        let commit_infos = graph
            .commits
            .iter()
            .enumerate()
            .map(|(i, commit)| {
                if i == 0 {
                    CommitInfo::new(commit, refs.iter().collect(), Color::Reset)
                } else {
                    CommitInfo::new(commit, repository.refs(&commit.commit_hash), Color::Reset)
                }
            })
            .collect();
        let mut state = CommitListState::new(
            commit_infos,
            graph_image_manager,
            0,
            repository.head(),
            FxHashMap::default(),
            SearchOptions::default(),
            0,
        );
        state.reset_height(subjects.len());
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        state.ensure_visible_graph_uploaded();
        let commit_list = CommitList::new(test_app_context())
            .scrollbar(true)
            .more_button(true)
            .hovered(hovered);
        commit_list.render(area, &mut buf, &mut state);
        f(&buf, state.more_button_column())
    }

    #[test]
    fn test_the_more_button_is_rendered_whole_on_every_row_and_width() {
        let refs = [branch("main"), remote_branch("origin/main"), tag("v1.0.0")];
        for width in 3..=120u16 {
            for index in 0..4usize {
                with_rendered_commit_list_with_refs(
                    &["a very long subject that goes on and on", "b", "c", "d"],
                    &refs,
                    width,
                    5,
                    Some(HoveredCommit {
                        index,
                        armed: false,
                    }),
                    |buf, column| {
                        let column = column.expect("the bar is reserved");
                        assert_eq!(
                            buf[(column, index as u16)].symbol(),
                            MORE_BUTTON,
                            "width {width}, row {index}"
                        );
                        assert_eq!(
                            column,
                            buf.area.width - 2,
                            "width {width}: the bar is the last column of the content area"
                        );
                        assert!(
                            column + 1 < buf.area.width,
                            "width {width}: the scrollbar keeps its own column"
                        );
                    },
                );
            }
        }
    }

    #[test]
    fn test_the_more_button_is_rendered_whole_on_the_first_and_the_last_row() {
        let refs = [branch("main"), tag("v1.0.0")];
        for index in [0usize, 3] {
            with_rendered_commit_list_with_refs(
                &["a rather long subject", "b", "c", "d"],
                &refs,
                40,
                4,
                Some(HoveredCommit { index, armed: true }),
                |buf, column| {
                    let column = column.expect("the bar is reserved");
                    assert_eq!(buf[(column, index as u16)].symbol(), MORE_BUTTON);
                    assert!(
                        buf[(column, index as u16)]
                            .style()
                            .add_modifier
                            .contains(Modifier::REVERSED),
                        "row {index} is the armed button"
                    );
                },
            );
        }
    }

    #[test]
    fn test_the_more_button_is_rendered_on_the_row_of_the_hovered_commit_when_the_list_is_scrolled()
    {
        with_commit_list_state(&["a", "b", "c", "d", "e", "f"], |state| {
            let area = Rect::new(0, 0, 60, 3);
            let mut buf = Buffer::empty(area);
            state.ensure_visible_graph_uploaded();
            // The list is scrolled, so the first row of the viewport shows the third commit
            state.offset = 2;
            let commit_list = CommitList::new(test_app_context())
                .scrollbar(true)
                .more_button(true)
                .hovered(Some(HoveredCommit {
                    index: 2,
                    armed: false,
                }));
            commit_list.render(area, &mut buf, state);
            let column = state.more_button_column().expect("the bar is reserved");
            assert_eq!(
                buf[(column, 0)].symbol(),
                MORE_BUTTON,
                "the button is on the row of the hovered commit, not on its absolute row"
            );
            assert_eq!(buf[(column, 1)].symbol(), " ");
        })
    }

    #[test]
    fn test_the_more_button_bar_does_not_overlap_the_scrollbar() {
        with_commit_list_state(&["a", "b", "c", "d", "e", "f"], |state| {
            let area = Rect::new(0, 0, 60, 3);
            let mut buf = Buffer::empty(area);
            state.ensure_visible_graph_uploaded();
            let commit_list = CommitList::new(test_app_context())
                .scrollbar(true)
                .more_button(true)
                .hovered(Some(HoveredCommit {
                    index: 1,
                    armed: false,
                }));
            commit_list.render(area, &mut buf, state);
            let column = state.more_button_column().expect("the bar is reserved");
            let scrollbar = column + 1;
            assert!(
                matches!(buf[(scrollbar, 1)].symbol(), "│" | "█"),
                "the scrollbar keeps its own column, which is the one after the bar"
            );
            assert_eq!(
                buf[(column, 1)].symbol(),
                MORE_BUTTON,
                "the button is rendered next to the scrollbar, not under it"
            );
        })
    }

    #[test]
    fn test_the_more_button_is_not_rendered_on_a_row_without_a_commit() {
        with_rendered_commit_list(
            &["a", "b", "c"],
            60,
            5,
            true,
            Some(HoveredCommit {
                index: 2,
                armed: true,
            }),
            |buf, column| {
                let column = column.expect("the bar is reserved");
                assert_eq!(buf[(column, 2)].symbol(), MORE_BUTTON);
                for row in 3..5 {
                    assert_eq!(buf[(column, row)].symbol(), " ", "row {row}");
                }
            },
        );
    }

    /// The row of a one commit list rendered with the given column visibility.
    fn rendered_row(commit: &Commit, commit_list_config: UiCommitListConfig) -> String {
        let commits = vec![commit.clone()];
        with_commits(commits, |state| {
            let area = Rect::new(0, 0, 60, 1);
            let mut buf = Buffer::empty(area);
            state.ensure_visible_graph_uploaded();
            let commit_list = CommitList::new(test_app_context_with(commit_list_config));
            StatefulWidget::render(commit_list, area, &mut buf, state);
            (area.left()..area.right())
                .map(|x| buf[(x, area.top())].symbol())
                .collect()
        })
    }

    #[test]
    fn test_the_columns_hidden_from_the_action_menu_are_not_rendered() {
        let commit = Commit {
            commit_hash: CommitHash::from("abcdef0123456789abcdef0123456789abcdef01"),
            subject: "the subject".into(),
            author_name: "the author".into(),
            author_date: "2024-01-02T03:04:05+00:00"
                .parse::<chrono::DateTime<chrono::FixedOffset>>()
                .unwrap(),
            ..Commit::default()
        };
        let short_hash = commit.commit_hash.as_short_hash().to_string();
        let date = commit
            .author_date
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string();

        let all = rendered_row(&commit, UiCommitListConfig::default());
        for expected in ["the subject", "the author", &short_hash, &date] {
            assert!(all.contains(expected), "{expected:?} is rendered: {all:?}");
        }
        let subject_column = all.find("the subject").unwrap();

        let cases = [
            (
                UiCommitListConfig {
                    show_date: false,
                    ..Default::default()
                },
                date.as_str(),
            ),
            (
                UiCommitListConfig {
                    show_hash: false,
                    ..Default::default()
                },
                short_hash.as_str(),
            ),
            (
                UiCommitListConfig {
                    show_author: false,
                    ..Default::default()
                },
                "the author",
            ),
        ];
        for (commit_list_config, hidden) in cases {
            let row = rendered_row(&commit, commit_list_config);
            assert!(
                !row.contains(hidden),
                "{hidden:?} is not rendered when its column is hidden: {row:?}"
            );
            assert!(
                row.contains("the subject"),
                "the subject is still rendered: {row:?}"
            );
            assert_eq!(
                row.find("the subject"),
                Some(subject_column),
                "the columns before the hidden one do not move: {row:?}"
            );
            assert_eq!(
                row[..subject_column],
                all[..subject_column],
                "the graph and the marker are not shifted: {row:?}"
            );
        }
    }
}
