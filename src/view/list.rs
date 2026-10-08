use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use ratatui::{
    crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    layout::{Position, Rect},
    Frame,
};

use crate::{
    app::AppContext,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    view::{ListRefreshViewContext, RefreshViewContext},
    widget::commit_list::{
        is_double_click, BadgeClick, CommitList, CommitListState, HoveredCommit, SearchState,
    },
};

/// How long the mouse must rest on the more button of a row before the action menu opens.
const MORE_BUTTON_DWELL: Duration = Duration::from_millis(400);

/// A scrollbar drag in progress.
#[derive(Debug, Clone, Copy)]
struct ScrollbarDrag {
    grab_offset: u16,
}

/// A more button that is waiting for the dwell that opens the action menu.
#[derive(Debug, Clone, Copy)]
struct ArmedMoreButton {
    /// The token that identifies this arming, so that the dwell of an arming the mouse left is
    /// not mistaken for the dwell of a newer one.
    token: u64,
}

/// The mouse hover of the commit list, used to show the more button of the row under the cursor.
///
/// The hover remembers the cell of the mouse on the screen rather than the commit it was over, so
/// that the button stays on the row under the cursor when the viewport moves: the commit of a row
/// changes when the list is scrolled, resized or refreshed, while the mouse does not move and
/// reports nothing.
#[derive(Debug, Default)]
struct MoreButtonHover {
    /// The cell of the mouse while it is on the commit list, if any.
    cell: Option<Position>,
    /// The more button the mouse rests on, if any.
    armed: Option<ArmedMoreButton>,
    /// The counter that mints the token of the next arming.
    next_token: u64,
}

impl MoreButtonHover {
    /// Update the hover with the cell of the mouse.
    ///
    /// `on_more_button` tells whether the mouse is on the more button column of a row, which
    /// starts the dwell that opens the action menu. Returns the token of the dwell to wait for, or
    /// `None` when this did not start a new one.
    fn moved(&mut self, cell: Position, on_more_button: bool) -> Option<u64> {
        let moved_to_another_button = self.cell != Some(cell);
        self.cell = Some(cell);
        if !on_more_button {
            self.armed = None;
            return None;
        }
        if !moved_to_another_button {
            // The mouse is still on the same button, so the dwell continues
            return None;
        }
        let token = self.next_token;
        self.next_token = self.next_token.wrapping_add(1);
        self.armed = Some(ArmedMoreButton { token });
        Some(token)
    }

    /// Take the dwell with the given token, if it is still the current one.
    ///
    /// Returns the cell of the button it was armed on. A token that is not the current one is
    /// stale, because the mouse left the button or armed another one in the meantime, so it is
    /// ignored.
    fn take(&mut self, token: u64) -> Option<Position> {
        if !self.armed.is_some_and(|armed| armed.token == token) {
            return None;
        }
        let cell = self.cell;
        self.clear();
        cell
    }

    fn clear(&mut self) {
        self.cell = None;
        self.armed = None;
    }

    fn cell(&self) -> Option<Position> {
        self.cell
    }

    fn is_armed(&self) -> bool {
        self.armed.is_some()
    }

    /// The commit the more button is shown for, and whether the mouse rests on its button.
    ///
    /// The commit is the one displayed on the row of the mouse of the last rendered frame, which
    /// is looked up by the caller, so that the button follows the row under the cursor.
    fn hovered_commit(
        &self,
        index_at_row: Option<usize>,
        more_button_column: Option<u16>,
    ) -> Option<HoveredCommit> {
        let cell = self.cell?;
        Some(HoveredCommit {
            index: index_at_row?,
            // The button is reversed only while the mouse is on the button column
            armed: self.is_armed() && more_button_column == Some(cell.x),
        })
    }
}

#[derive(Debug)]
pub struct ListView<'a> {
    commit_list_state: Option<CommitListState<'a>>,

    list_area: Rect,
    dragging: Option<ScrollbarDrag>,
    /// The last click on a ref badge, used to detect double clicks.
    last_badge_click: Option<BadgeClick>,
    /// The mouse hover of the list, used to show the more button of the hovered row.
    hover: MoreButtonHover,

    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> ListView<'a> {
    pub fn new(
        commit_list_state: CommitListState<'a>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> ListView<'a> {
        ListView {
            commit_list_state: Some(commit_list_state),
            list_area: Rect::default(),
            dragging: None,
            last_badge_click: None,
            hover: MoreButtonHover::default(),
            ctx,
            tx,
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count;
        if let SearchState::Searching { .. } = self.as_list_state().search_state() {
            match event {
                UserEvent::Confirm => {
                    self.as_mut_list_state().apply_search();
                    self.update_matched_message();
                }
                UserEvent::Cancel => {
                    self.as_mut_list_state().cancel_search();
                    self.clear_search_query();
                }
                UserEvent::SearchTargetToggle => {
                    self.as_mut_list_state().toggle_search_target();
                    self.update_search_status();
                }
                UserEvent::IgnoreCaseToggle => {
                    self.as_mut_list_state().toggle_ignore_case();
                    self.update_search_status();
                }
                UserEvent::FuzzyToggle => {
                    self.as_mut_list_state().toggle_fuzzy();
                    self.update_search_status();
                }
                _ => {
                    self.as_mut_list_state().handle_search_input(key);
                    self.update_search_status();
                }
            }
            return;
        } else {
            match event {
                UserEvent::Quit => {
                    self.tx.send(AppEvent::Quit);
                }
                UserEvent::NavigateDown | UserEvent::SelectDown => {
                    for _ in 0..count {
                        self.as_mut_list_state().select_next();
                    }
                }
                UserEvent::NavigateUp | UserEvent::SelectUp => {
                    for _ in 0..count {
                        self.as_mut_list_state().select_prev();
                    }
                }
                UserEvent::GoToParent => {
                    for _ in 0..count {
                        self.as_mut_list_state().select_parent();
                    }
                }
                UserEvent::GoToTop => {
                    self.as_mut_list_state().select_first();
                }
                UserEvent::GoToBottom => {
                    self.as_mut_list_state().select_last();
                }
                UserEvent::ScrollDown => {
                    for _ in 0..count {
                        self.as_mut_list_state().scroll_down();
                    }
                }
                UserEvent::ScrollUp => {
                    for _ in 0..count {
                        self.as_mut_list_state().scroll_up();
                    }
                }
                UserEvent::PageDown => {
                    for _ in 0..count {
                        self.as_mut_list_state().scroll_down_page();
                    }
                }
                UserEvent::PageUp => {
                    for _ in 0..count {
                        self.as_mut_list_state().scroll_up_page();
                    }
                }
                UserEvent::HalfPageDown => {
                    for _ in 0..count {
                        self.as_mut_list_state().scroll_down_half();
                    }
                }
                UserEvent::HalfPageUp => {
                    for _ in 0..count {
                        self.as_mut_list_state().scroll_up_half();
                    }
                }
                UserEvent::SelectTop => {
                    self.as_mut_list_state().select_high();
                }
                UserEvent::SelectMiddle => {
                    self.as_mut_list_state().select_middle();
                }
                UserEvent::SelectBottom => {
                    self.as_mut_list_state().select_low();
                }
                UserEvent::ShortCopy => {
                    self.copy_commit_short_hash();
                }
                UserEvent::FullCopy => {
                    self.copy_commit_hash();
                }
                UserEvent::Search => {
                    self.as_mut_list_state().start_search();
                    self.update_search_status();
                }
                UserEvent::SearchTargetToggle => {
                    self.as_mut_list_state().toggle_search_target();
                    self.update_search_options_message();
                }
                UserEvent::IgnoreCaseToggle => {
                    self.as_mut_list_state().toggle_ignore_case();
                    self.update_search_options_message();
                }
                UserEvent::FuzzyToggle => {
                    self.as_mut_list_state().toggle_fuzzy();
                    self.update_search_options_message();
                }
                UserEvent::UserCommand(n) => {
                    self.tx.send(AppEvent::OpenUserCommand(n));
                }
                UserEvent::HelpToggle => {
                    self.tx.send(AppEvent::OpenHelp);
                }
                UserEvent::Cancel => {
                    self.as_mut_list_state().cancel_search();
                    self.clear_search_query();
                }
                UserEvent::Confirm => {
                    self.tx.send(AppEvent::OpenDetail);
                }
                UserEvent::RefList => {
                    self.tx.send(AppEvent::OpenRefs);
                }
                UserEvent::Actions => {
                    self.tx.send(AppEvent::OpenActions);
                }
                UserEvent::Refresh => {
                    self.refresh();
                }
                _ => {}
            }
        }

        if let SearchState::Applied { .. } = self.as_list_state().search_state() {
            match event {
                UserEvent::GoToNext => {
                    self.as_mut_list_state().select_next_match();
                    self.update_matched_message();
                }
                UserEvent::GoToPrevious => {
                    self.as_mut_list_state().select_prev_match();
                    self.update_matched_message();
                }
                _ => {}
            }
            // Do not return here
        }
    }

    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_left_click(mouse.column, mouse.row)
            }
            MouseEventKind::Drag(MouseButton::Left) => self.handle_left_drag(mouse.row),
            MouseEventKind::Up(MouseButton::Left) => {
                self.dragging = None;
            }
            MouseEventKind::Moved => self.handle_moved(mouse.column, mouse.row),
            MouseEventKind::Down(MouseButton::Right) => {
                // The menu is opened for the commit under the cursor
                if let Some(index) = self
                    .as_list_state()
                    .commit_index_at_row(self.list_area, mouse.row)
                {
                    self.as_mut_list_state().select_index_without_scroll(index);
                }
                self.tx.send(AppEvent::OpenActions);
            }
            MouseEventKind::ScrollDown => self.as_mut_list_state().scroll_wheel_down(),
            MouseEventKind::ScrollUp => self.as_mut_list_state().scroll_wheel_up(),
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.list_area = area;
        let hovered = self.hovered_commit(area);
        let commit_list = CommitList::new(self.ctx.clone())
            .scrollbar(true)
            .more_button(self.ctx.ui_config.common.mouse)
            .hovered(hovered);
        f.render_stateful_widget(commit_list, area, self.as_mut_list_state());
    }

    /// The commit the more button is shown for of the last rendered frame, if any.
    ///
    /// The commit is looked up from the cell of the mouse every frame, so that the button follows
    /// the row under the cursor when the viewport moves.
    fn hovered_commit(&self, area: Rect) -> Option<HoveredCommit> {
        let state = self.as_list_state();
        let index_at_row = self
            .hover
            .cell()
            .and_then(|cell| state.commit_index_at_row(area, cell.y));
        self.hover
            .hovered_commit(index_at_row, state.more_button_column())
    }

    pub fn update_layout(&mut self, area: Rect) {
        self.list_area = area;
        self.as_mut_list_state().update_height(area.height as usize);
    }

    pub fn prepare_graph_uploads(&mut self) {
        self.as_mut_list_state().ensure_visible_graph_uploaded();
    }
}

impl<'a> ListView<'a> {
    pub fn take_list_state(&mut self) -> CommitListState<'a> {
        self.commit_list_state.take().unwrap()
    }

    /// Select the commit on the clicked row, or handle the click on the scrollbar.
    ///
    /// A double click on the badge of a ref switches the repository to that ref; a single click
    /// only selects the commit. A click on the more button opens the action menu of the commit
    /// without waiting for the dwell.
    fn handle_left_click(&mut self, column: u16, row: u16) {
        // A new press always ends a previous drag
        self.dragging = None;
        if let Some(scrollbar_area) = self.scrollbar_area() {
            if column == scrollbar_area.left()
                && row >= scrollbar_area.top()
                && row < scrollbar_area.bottom()
                && self.start_scrollbar_drag(row - scrollbar_area.top())
            {
                return;
            }
        }

        if self.more_button_at(column, row) {
            // The click is not the start of a double click on a badge
            self.last_badge_click = None;
            if let Some(index) = self
                .as_list_state()
                .commit_index_at_row(self.list_area, row)
            {
                self.as_mut_list_state().select_index_without_scroll(index);
                // The menu of the button is anchored to the button itself
                self.tx
                    .send(AppEvent::OpenActionsAt(Rect::new(column, row, 1, 1)));
            }
            return;
        }

        let badge = self.as_list_state().ref_badge_at(column, row).cloned();
        let click = badge.as_ref().map(|badge| BadgeClick {
            rect: badge.rect,
            at: Instant::now(),
        });
        let double_click = is_double_click(self.last_badge_click, click);
        // A double click is not the start of another one, so it is forgotten
        self.last_badge_click = if double_click { None } else { click };

        // Clicking a row selects that commit; the viewport does not move
        if let Some(index) = self
            .as_list_state()
            .commit_index_at_row(self.list_area, row)
        {
            self.as_mut_list_state().select_index_without_scroll(index);
        }

        if let (true, Some(badge)) = (double_click, badge) {
            self.tx.send(AppEvent::SwitchToRef(Box::new(badge.r)));
        }
    }

    /// Update the hover with the position of the mouse, and start the dwell of the more button it
    /// is resting on.
    fn handle_moved(&mut self, column: u16, row: u16) {
        let on_more_button = self.more_button_at(column, row);
        if let Some(token) = self.hover.moved(Position::new(column, row), on_more_button) {
            self.spawn_more_button_dwell(token);
        }
    }

    /// Wait for the dwell of the given arming in the background.
    ///
    /// The arming is ignored when the event comes back after the mouse left the button, which is
    /// decided by the token when the event is handled.
    fn spawn_more_button_dwell(&self, token: u64) {
        let tx = self.tx.clone();
        thread::spawn(move || {
            thread::sleep(MORE_BUTTON_DWELL);
            tx.send_if_running(AppEvent::MoreButtonHoverElapsed(token));
        });
    }

    /// Whether the given screen position is on the more button of a row.
    fn more_button_at(&self, column: u16, row: u16) -> bool {
        self.as_list_state().more_button_column() == Some(column)
            && self
                .as_list_state()
                .commit_index_at_row(self.list_area, row)
                .is_some()
    }

    /// Open the action menu when the mouse rested on the more button of a row for long enough.
    ///
    /// Returns the area of the button the mouse rested on, which the menu is anchored to, or
    /// `None` when the dwell was stale, or when the row under the cursor has no commit anymore.
    /// The dwell is ignored when its token is not the current one, which means that the mouse left
    /// the button or armed another one in the meantime.
    pub fn take_armed_more_button(&mut self, token: u64) -> Option<Rect> {
        let cell = self.hover.take(token)?;
        // The menu is opened for the commit of the row under the cursor, like a right click. The
        // row is looked up again, because the list may have been scrolled while the mouse rested.
        let index = self
            .as_list_state()
            .commit_index_at_row(self.list_area, cell.y)?;
        self.as_mut_list_state().select_index_without_scroll(index);
        Some(Rect::new(cell.x, cell.y, 1, 1))
    }

    /// Clear the mouse hover, so that the more button is not rendered anymore.
    pub fn clear_hover(&mut self) {
        self.hover.clear();
    }

    /// Jump the thumb to the clicked row and start dragging it.
    ///
    /// Returns whether the click was handled by the scrollbar.
    fn start_scrollbar_drag(&mut self, bar_row: u16) -> bool {
        let Some(geometry) = self.as_list_state().scrollbar_geometry() else {
            // The whole list is visible, so the scrollbar is not shown
            return false;
        };
        let grab_offset = geometry.grab_offset(bar_row);
        let thumb_start = bar_row.saturating_sub(grab_offset);
        let state = self.as_mut_list_state();
        state.set_offset(state.offset_for_thumb_start(thumb_start));
        self.dragging = Some(ScrollbarDrag { grab_offset });
        true
    }

    fn handle_left_drag(&mut self, row: u16) {
        let Some(drag) = self.dragging else {
            return;
        };
        let Some(scrollbar_area) = self.scrollbar_area() else {
            return;
        };
        // The thumb follows the cursor along the track, so positions beyond the ends are clamped
        let bar_row =
            row.clamp(scrollbar_area.top(), scrollbar_area.bottom() - 1) - scrollbar_area.top();
        let thumb_start = bar_row.saturating_sub(drag.grab_offset);
        let state = self.as_mut_list_state();
        state.set_offset(state.offset_for_thumb_start(thumb_start));
    }

    /// The scrollbar column of the list area, where the mouse is handled by the scrollbar.
    fn scrollbar_area(&self) -> Option<Rect> {
        let area = self.list_area;
        (area.width > 0 && area.height > 0)
            .then(|| Rect::new(area.right() - 1, area.y, 1, area.height))
    }

    fn as_mut_list_state(&mut self) -> &mut CommitListState<'a> {
        self.commit_list_state.as_mut().unwrap()
    }

    pub fn as_list_state(&self) -> &CommitListState<'a> {
        self.commit_list_state.as_ref().unwrap()
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        self.as_mut_list_state().drain_pending_graph_uploads()
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        self.as_list_state().graph_image_ids_sorted()
    }

    fn update_search_status(&self) {
        if let SearchState::Searching { .. } = self.as_list_state().search_state() {
            let list_state = self.as_list_state();
            if let Some(query) = list_state.search_query_string() {
                let cursor_position = list_state.search_query_cursor_position();
                let options = list_state.search_options().status_string();
                self.tx.send(AppEvent::UpdateStatusInput {
                    message: query,
                    cursor_position,
                    metadata: options,
                });
            }
        }
    }

    fn update_search_options_message(&self) {
        if let SearchState::Applied { .. } = self.as_list_state().search_state() {
            self.update_matched_message();
        } else {
            let options = self.as_list_state().search_options().status_string();
            self.tx.send(AppEvent::UpdateStatusTransient(format!(
                "Search: {options}"
            )));
        }
    }

    fn clear_search_query(&self) {
        self.tx.send(AppEvent::ClearStatusLine);
    }

    fn update_matched_message(&self) {
        if let Some((msg, matched)) = self.as_list_state().matched_query_string() {
            let options = self.as_list_state().search_options().status_string();
            self.tx.send(AppEvent::UpdateSearchResult {
                message: msg,
                options,
                matched,
            });
        } else {
            self.tx.send(AppEvent::ClearStatusLine);
        }
    }

    fn copy_commit_short_hash(&self) {
        let selected = self.as_list_state().selected_commit_hash();
        self.copy_to_clipboard("Commit SHA (short)".into(), selected.as_short_hash().into());
    }

    fn copy_commit_hash(&self) {
        let selected = self.as_list_state().selected_commit_hash();
        self.copy_to_clipboard("Commit SHA".into(), selected.as_str().into());
    }

    fn copy_to_clipboard(&self, name: String, value: String) {
        self.tx.send(AppEvent::CopyToClipboard { name, value });
    }

    pub fn refresh(&self) {
        let list_state = self.as_list_state();
        let list_context = ListRefreshViewContext::from(list_state);
        let context = RefreshViewContext::List { list_context };
        self.tx.send(AppEvent::Refresh(context));
    }

    pub fn reset_commit_list_with(&mut self, list_context: &ListRefreshViewContext) {
        list_context.restore(self.as_mut_list_state());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cell of a more button of the list, used by the tests.
    const BUTTON: Position = Position::new(58, 2);
    /// The cell of the more button of another row.
    const OTHER_BUTTON: Position = Position::new(58, 3);
    /// The cell of a row of the list that is not on the more button column.
    const ROW: Position = Position::new(10, 2);
    /// The column the more button bar is rendered in.
    const BUTTON_COLUMN: u16 = 58;

    /// The commit the more button is shown for, with the given commit displayed on the row of the
    /// mouse.
    fn hovered(hover: &MoreButtonHover, index_at_row: Option<usize>) -> Option<HoveredCommit> {
        hover.hovered_commit(index_at_row, Some(BUTTON_COLUMN))
    }

    #[test]
    fn test_moving_to_a_row_shows_its_more_button_without_arming_it() {
        let mut hover = MoreButtonHover::default();
        assert_eq!(hover.moved(ROW, false), None);
        assert_eq!(hover.cell(), Some(ROW));
        assert_eq!(
            hovered(&hover, Some(3)),
            Some(HoveredCommit {
                index: 3,
                armed: false
            })
        );
        assert!(!hover.is_armed());
    }

    #[test]
    fn test_moving_on_a_more_button_starts_a_dwell() {
        let mut hover = MoreButtonHover::default();
        let token = hover.moved(BUTTON, true);
        assert_eq!(token, Some(0));
        assert_eq!(
            hovered(&hover, Some(3)),
            Some(HoveredCommit {
                index: 3,
                armed: true
            })
        );
        assert!(hover.is_armed());
    }

    #[test]
    fn test_resting_on_the_same_more_button_keeps_the_dwell() {
        let mut hover = MoreButtonHover::default();
        let token = hover.moved(BUTTON, true).unwrap();
        assert_eq!(hover.moved(BUTTON, true), None);
        assert_eq!(hover.take(token), Some(BUTTON));
    }

    #[test]
    fn test_moving_off_the_more_button_expires_the_dwell() {
        let mut hover = MoreButtonHover::default();
        let token = hover.moved(BUTTON, true).unwrap();
        assert_eq!(hover.moved(ROW, false), None);
        assert_eq!(hover.cell(), Some(ROW), "the row is still hovered");
        assert_eq!(
            hovered(&hover, Some(3)),
            Some(HoveredCommit {
                index: 3,
                armed: false
            }),
            "the button is shown without being reversed"
        );
        assert!(!hover.is_armed());
        assert_eq!(hover.take(token), None, "the expired dwell is ignored");
    }

    #[test]
    fn test_moving_out_of_the_list_hides_the_more_button_and_expires_the_dwell() {
        let mut hover = MoreButtonHover::default();
        let token = hover.moved(BUTTON, true).unwrap();
        let below_the_list = Position::new(10, 40);
        assert_eq!(hover.moved(below_the_list, false), None);
        assert!(!hover.is_armed());
        assert_eq!(
            hovered(&hover, None),
            None,
            "there is no commit on the row of the mouse"
        );
        assert_eq!(hover.take(token), None);
    }

    #[test]
    fn test_moving_to_another_more_button_expires_the_previous_dwell() {
        let mut hover = MoreButtonHover::default();
        let first = hover.moved(BUTTON, true).unwrap();
        let second = hover.moved(OTHER_BUTTON, true).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            hover.take(first),
            None,
            "the dwell of the row that was left is ignored"
        );
        assert_eq!(hover.take(second), Some(OTHER_BUTTON));
    }

    #[test]
    fn test_returning_to_a_more_button_starts_a_new_dwell() {
        let mut hover = MoreButtonHover::default();
        let first = hover.moved(BUTTON, true).unwrap();
        hover.moved(ROW, false);
        let second = hover.moved(BUTTON, true).unwrap();
        assert_ne!(first, second);
        assert_eq!(hover.take(first), None);
        assert_eq!(hover.take(second), Some(BUTTON));
    }

    #[test]
    fn test_taking_a_dwell_clears_the_hover() {
        let mut hover = MoreButtonHover::default();
        let token = hover.moved(BUTTON, true).unwrap();
        assert_eq!(hover.take(token), Some(BUTTON));
        assert_eq!(hover.cell(), None);
        assert!(!hover.is_armed());
        assert_eq!(hover.take(token), None, "a dwell is taken only once");
    }

    #[test]
    fn test_clearing_the_hover_expires_the_dwell() {
        let mut hover = MoreButtonHover::default();
        let token = hover.moved(BUTTON, true).unwrap();
        hover.clear();
        assert_eq!(hover.cell(), None);
        assert_eq!(hover.take(token), None);
    }

    #[test]
    fn test_the_more_button_follows_the_row_under_the_mouse_when_the_list_moves() {
        let mut hover = MoreButtonHover::default();
        hover.moved(BUTTON, false);
        // The list is scrolled, resized or refreshed while the mouse does not move, so the commit
        // displayed on the row of the mouse changes
        assert_eq!(
            hovered(&hover, Some(3)),
            Some(HoveredCommit {
                index: 3,
                armed: false
            })
        );
        assert_eq!(
            hovered(&hover, Some(7)),
            Some(HoveredCommit {
                index: 7,
                armed: false
            }),
            "the button follows the commit of the row, not the commit that was under the mouse"
        );
    }

    #[test]
    fn test_the_armed_more_button_is_reversed_only_while_the_bar_is_under_the_mouse() {
        let mut hover = MoreButtonHover::default();
        hover.moved(BUTTON, true);
        assert!(
            hover
                .hovered_commit(Some(3), Some(BUTTON_COLUMN))
                .unwrap()
                .armed
        );
        assert!(
            !hover
                .hovered_commit(Some(3), Some(BUTTON_COLUMN + 1))
                .unwrap()
                .armed,
            "the bar is not under the mouse after the layout changed"
        );
    }
}
