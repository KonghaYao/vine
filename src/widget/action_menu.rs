use std::rc::Rc;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, StatefulWidget, Widget},
};

use crate::{app::AppContext, git_action::ActionMenuEntry};

/// The horizontal line that separates two groups of entries of the menu.
const SEPARATOR: &str = "─";
/// The mark in front of an entry that shows a column which is shown.
const CHECKED_MARK: &str = "✓";
/// The mark in front of an entry that shows a column which is hidden.
const UNCHECKED_MARK: &str = " ";

/// Where the popup of the action menu is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActionMenuPlacement {
    /// The popup is centered on the screen, like a dialog.
    ///
    /// This is the placement of the menu that is opened with the keyboard or a right click.
    #[default]
    Center,
    /// The popup floats next to the given cell of the screen, like a tooltip.
    ///
    /// This is the placement of the menu that is opened from the more button of a commit list row,
    /// so that the menu does not cover the list it was opened from.
    Anchor(Position),
}

/// State of the action menu popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionMenuState {
    title: String,
    entries: Vec<ActionMenuEntry>,
    selected: usize,
    offset: usize,
    /// The rectangle of each entry of the last rendered frame, in screen coordinates.
    ///
    /// The rectangle of an entry that is not rendered is empty. The rectangles are recorded while
    /// rendering, so that a click can be matched to the entry that is on the screen, in the same
    /// way as the ref badges of the commit list.
    entry_rects: Vec<Rect>,
    /// The area of the popup of the last rendered frame, in screen coordinates.
    ///
    /// The area is empty until the menu is rendered.
    rendered_area: Rect,
}

impl ActionMenuState {
    pub fn new(title: String, entries: Vec<ActionMenuEntry>) -> Self {
        Self {
            title,
            entries,
            selected: 0,
            offset: 0,
            entry_rects: Vec::new(),
            rendered_area: Rect::default(),
        }
    }

    pub fn selected_entry(&self) -> Option<&ActionMenuEntry> {
        self.entries.get(self.selected)
    }

    /// Whether the entry with the given index can be selected.
    ///
    /// A separator is part of the menu, but it is not an action, so it is never selected.
    fn is_selectable(&self, index: usize) -> bool {
        self.entries
            .get(index)
            .is_some_and(ActionMenuEntry::is_selectable)
    }

    pub fn select_next(&mut self) {
        let next = (self.selected + 1..self.entries.len()).find(|i| self.is_selectable(*i));
        if let Some(next) = next {
            self.selected = next;
        }
    }

    pub fn select_prev(&mut self) {
        let prev = (0..self.selected).rev().find(|i| self.is_selectable(*i));
        if let Some(prev) = prev {
            self.selected = prev;
        }
    }

    /// Select the entry with the given index, if it can be selected.
    ///
    /// The separators are skipped, so that the mouse never highlights one.
    pub fn select(&mut self, index: usize) {
        if self.is_selectable(index) {
            self.selected = index;
        }
    }

    /// The index of the entry rendered at the given screen position of the last rendered frame.
    ///
    /// The separators are not entries for the mouse: no position of the menu is matched to one of
    /// them, so hovering or clicking one does nothing.
    pub fn entry_at(&self, column: u16, row: u16) -> Option<usize> {
        let position = Position::new(column, row);
        self.entry_rects
            .iter()
            .enumerate()
            .find(|(i, rect)| rect.contains(position) && self.is_selectable(*i))
            .map(|(i, _)| i)
    }

    /// Whether the given screen position of the last rendered frame is inside the popup.
    ///
    /// This is used to tell a click outside the popup, which closes the menu, from a click on the
    /// popup itself, which is ignored: the menu never lets a click through.
    pub fn contains(&self, column: u16, row: u16) -> bool {
        self.rendered_area.contains(Position::new(column, row))
    }
}

pub struct ActionMenu {
    ctx: Rc<AppContext>,
    placement: ActionMenuPlacement,
}

impl ActionMenu {
    pub fn new(ctx: Rc<AppContext>) -> Self {
        Self {
            ctx,
            placement: ActionMenuPlacement::default(),
        }
    }

    /// Place the popup next to the given cell of the screen, like a tooltip.
    pub fn placement(mut self, placement: ActionMenuPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// The area of the popup in the given area.
    pub fn popup_area(&self, area: Rect, state: &ActionMenuState) -> Rect {
        popup_area(area, &state.title, &state.entries, self.placement)
    }
}

impl StatefulWidget for ActionMenu {
    type State = ActionMenuState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let area = self.popup_area(area, state);
        state.rendered_area = area;
        Widget::render(Clear, area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.ctx.color_theme.divider_fg)
            .title(
                Span::raw(format!(" {} ", state.title))
                    .fg(self.ctx.color_theme.help_block_title_fg),
            )
            .padding(Padding::horizontal(1));
        let inner = block.inner(area);
        Widget::render(block, area, buf);
        state.entry_rects.clear();
        if inner.is_empty() {
            return;
        }

        let height = inner.height as usize;
        if state.selected < state.offset {
            state.offset = state.selected;
        } else if state.selected >= state.offset + height {
            state.offset = state.selected + 1 - height;
        }

        // The entries that are visible are recorded, so that the mouse can be matched to them
        state.entry_rects = (0..state.entries.len())
            .map(|i| {
                if i >= state.offset && i < state.offset + height {
                    Rect::new(inner.x, inner.y + (i - state.offset) as u16, inner.width, 1)
                } else {
                    Rect::default()
                }
            })
            .collect();

        let lines = state
            .entries
            .iter()
            .skip(state.offset)
            .take(height)
            .map(|entry| self.entry_line(entry, inner.width))
            .collect::<Vec<_>>();
        let paragraph = Paragraph::new(lines);
        Widget::render(paragraph, inner, buf);

        let selected_line = Rect::new(
            inner.x,
            inner.y + (state.selected - state.offset) as u16,
            inner.width,
            1,
        );
        buf.set_style(
            selected_line,
            (
                self.ctx.color_theme.list_selected_fg,
                self.ctx.color_theme.list_selected_bg,
                Modifier::BOLD,
            ),
        );
    }
}

impl ActionMenu {
    /// The line rendered for an entry of the menu.
    fn entry_line(&self, entry: &ActionMenuEntry, width: u16) -> Line<'static> {
        match entry {
            ActionMenuEntry::Separator => Line::from(Span::styled(
                SEPARATOR.repeat(width as usize),
                self.ctx.color_theme.divider_fg,
            )),
            // The mark is read from the current settings, so that a toggled entry shows its new
            // state as soon as the menu is rendered again
            ActionMenuEntry::Toggle { label, column } => {
                let mark = if self.ctx.commit_list_config.get().shows(*column) {
                    CHECKED_MARK
                } else {
                    UNCHECKED_MARK
                };
                Line::raw(format!("{mark} {label}"))
            }
            ActionMenuEntry::SubMenu { label, .. } => Line::raw(format!("{label} ▸")),
            ActionMenuEntry::Action { label, .. } => Line::raw(label.clone()),
        }
    }
}

fn popup_area(
    area: Rect,
    title: &str,
    entries: &[ActionMenuEntry],
    placement: ActionMenuPlacement,
) -> Rect {
    let max_label_width = entries
        .iter()
        .map(|e| {
            console::measure_text_width(e.label())
                + if e.has_submenu() { 2 } else { 0 }
                + if e.column_toggle().is_some() { 2 } else { 0 }
        })
        .max()
        .unwrap_or_default();
    let width = (max_label_width + 2 /* padding */ + 2/* borders */)
        .max(console::measure_text_width(title) + 4)
        .min(area.width as usize) as u16;
    let height = (entries.len() + 2/* borders */)
        .max(3)
        .min(area.height as usize) as u16;

    match placement {
        ActionMenuPlacement::Center => centered_popup_area(area, width, height),
        ActionMenuPlacement::Anchor(anchor) => anchored_popup_area(area, width, height, anchor),
    }
}

fn centered_popup_area(area: Rect, width: u16, height: u16) -> Rect {
    let [_, center_area, _] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(width),
        Constraint::Min(0),
    ])
    .areas(area);
    let [_, popup_area, _] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(height),
        Constraint::Min(1),
    ])
    .areas(center_area);
    popup_area
}

/// The area of the popup when it floats next to the anchor cell, like a tooltip.
///
/// The popup prefers to be on the left of the anchor and below it, which is the free space when the
/// anchor is the more button of a commit list row: the menu grows towards the middle of the screen.
/// It is flipped to the other side when that side does not have enough room, and it is clamped to
/// the area, so that a popup near the bottom or the right edge, or after a resize, stays complete.
fn anchored_popup_area(area: Rect, width: u16, height: u16, anchor: Position) -> Rect {
    let x = if anchor.x.saturating_sub(area.x) >= width {
        anchor.x - width
    } else {
        anchor.x.saturating_add(1)
    }
    .min(area.right().saturating_sub(width))
    .max(area.x);

    let y = if anchor.y.saturating_add(1).saturating_add(height) <= area.bottom() {
        anchor.y.saturating_add(1)
    } else {
        anchor.y.saturating_sub(height)
    }
    .min(area.bottom().saturating_sub(height))
    .max(area.y);

    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    use crate::{
        color::ColorTheme,
        config::{ActionsConfig, CommitListColumn, CoreConfig, UiCommitListConfig, UiConfig},
        keybind::KeyBind,
        protocol::ImageProtocol,
    };

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

    /// The state of a menu whose entries are sub menus, which need no action to be built.
    fn menu(labels: &[&str]) -> ActionMenuState {
        ActionMenuState::new(
            "Actions".into(),
            labels
                .iter()
                .map(|label| ActionMenuEntry::SubMenu {
                    label: (*label).into(),
                    entries: Vec::new(),
                })
                .collect(),
        )
    }

    /// An entry that shows or hides a column of the commit list.
    fn column_toggle(column: CommitListColumn) -> ActionMenuEntry {
        ActionMenuEntry::Toggle {
            label: column.label().into(),
            column,
        }
    }

    /// The entries of a menu that ends with a separator and the column toggles.
    fn menu_with_column_toggles() -> Vec<ActionMenuEntry> {
        let mut entries = vec![
            ActionMenuEntry::SubMenu {
                label: "first".into(),
                entries: Vec::new(),
            },
            ActionMenuEntry::SubMenu {
                label: "second".into(),
                entries: Vec::new(),
            },
            ActionMenuEntry::Separator,
        ];
        entries.extend(
            [
                CommitListColumn::Date,
                CommitListColumn::Hash,
                CommitListColumn::Author,
            ]
            .map(column_toggle),
        );
        entries
    }

    /// The rows of the popup of the last rendered frame, as strings.
    fn rendered_rows(state: &ActionMenuState, buf: &Buffer) -> Vec<String> {
        let popup = state.rendered_area;
        (popup.y..popup.bottom())
            .map(|y| {
                (popup.x..popup.right())
                    .map(|x| buf[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    /// Renders the given entries on a test buffer, and gives the state and the buffer to the
    /// closure.
    fn with_rendered_menu<R>(
        ctx: &Rc<AppContext>,
        entries: Vec<ActionMenuEntry>,
        f: impl FnOnce(&ActionMenuState, &Buffer) -> R,
    ) -> R {
        let area = Rect::new(0, 0, 40, 24);
        let mut state = ActionMenuState::new("Actions".into(), entries);
        let mut buf = Buffer::empty(area);
        ActionMenu::new(ctx.clone())
            .placement(ActionMenuPlacement::Center)
            .render(area, &mut buf, &mut state);
        f(&state, &buf)
    }

    fn popup_of(area: Rect, state: &ActionMenuState, placement: ActionMenuPlacement) -> Rect {
        popup_area(area, &state.title, &state.entries, placement)
    }

    #[test]
    fn test_the_popup_is_centered_when_it_is_not_anchored() {
        let area = Rect::new(0, 0, 80, 24);
        let popup = popup_of(area, &menu(&["a", "b"]), ActionMenuPlacement::Center);
        let left_margin = popup.x - area.x;
        let right_margin = area.right() - popup.right();
        assert!(
            left_margin.abs_diff(right_margin) <= 1,
            "the popup is centered horizontally: {popup:?}"
        );
        assert!(popup.bottom() <= area.bottom());
    }

    #[test]
    fn test_the_anchored_popup_is_placed_on_the_left_of_the_anchor_and_below_it() {
        let area = Rect::new(0, 0, 80, 24);
        let anchor = Position::new(78, 5);
        let popup = popup_of(
            area,
            &menu(&["a", "b"]),
            ActionMenuPlacement::Anchor(anchor),
        );
        assert!(
            popup.right() <= anchor.x,
            "{popup:?} is on the left of {anchor:?}"
        );
        assert!(popup.y > anchor.y, "{popup:?} is below {anchor:?}");
        assert!(popup.right() <= area.right() && popup.bottom() <= area.bottom());
    }

    #[test]
    fn test_the_anchored_popup_is_flipped_to_the_right_when_the_left_side_is_too_narrow() {
        let area = Rect::new(0, 0, 80, 24);
        let anchor = Position::new(3, 5);
        let popup = popup_of(
            area,
            &menu(&["a", "b"]),
            ActionMenuPlacement::Anchor(anchor),
        );
        assert!(
            popup.x > anchor.x,
            "{popup:?} is on the right of {anchor:?}"
        );
        assert!(popup.right() <= area.right());
    }

    #[test]
    fn test_the_anchored_popup_is_flipped_up_when_there_is_no_room_below() {
        let area = Rect::new(0, 0, 80, 24);
        let anchor = Position::new(78, 23);
        let popup = popup_of(
            area,
            &menu(&["a", "b", "c", "d"]),
            ActionMenuPlacement::Anchor(anchor),
        );
        assert!(popup.bottom() <= anchor.y, "{popup:?} is above {anchor:?}");
        assert!(popup.y >= area.y);
    }

    #[test]
    fn test_the_anchored_popup_is_kept_inside_a_small_area() {
        let labels = ["a label that is rather long", "b"];
        for area in [
            Rect::new(0, 0, 1, 1),
            Rect::new(0, 0, 5, 2),
            Rect::new(0, 0, 20, 4),
            Rect::new(7, 3, 30, 10),
        ] {
            let anchor = Position::new(area.right() - 1, area.bottom() - 1);
            let popup = popup_of(area, &menu(&labels), ActionMenuPlacement::Anchor(anchor));
            assert!(
                popup.right() <= area.right() && popup.bottom() <= area.bottom(),
                "area {area:?}: {popup:?} does not stick out"
            );
            assert!(
                popup.x >= area.x && popup.y >= area.y,
                "area {area:?}: {popup:?} does not stick out"
            );
        }
    }

    #[test]
    fn test_the_entry_rects_match_the_entries_that_are_rendered() {
        let area = Rect::new(0, 0, 80, 24);
        let labels = ["first", "second", "third"];
        let mut state = menu(&labels);
        let mut buf = Buffer::empty(area);
        let anchor = Position::new(78, 2);
        ActionMenu::new(test_app_context())
            .placement(ActionMenuPlacement::Anchor(anchor))
            .render(area, &mut buf, &mut state);

        assert!(
            state.rendered_area.right() <= anchor.x + 1,
            "the popup is next to the anchor"
        );
        for (i, label) in labels.iter().enumerate() {
            let rect = state.entry_rects[i];
            assert_eq!(state.entry_at(rect.x, rect.y), Some(i));
            assert_eq!(
                state.entry_at(rect.right() - 1, rect.bottom() - 1),
                Some(i),
                "the whole row of the entry is clickable"
            );
            assert_eq!(
                buf[(rect.x, rect.y)].symbol(),
                label.chars().next().unwrap().to_string(),
                "the entry {i} is rendered in its rectangle"
            );
            assert!(state.contains(rect.x, rect.y));
        }
        assert_eq!(
            state.entry_at(area.right() - 1, area.bottom() - 1),
            None,
            "a cell outside of the popup is on no entry"
        );
    }

    #[test]
    fn test_the_entry_rects_follow_the_rendered_entries_when_the_menu_is_scrolled() {
        // The popup is too short for the three entries, so the list of the menu is scrolled
        let area = Rect::new(0, 0, 40, 4);
        let mut state = menu(&["first", "second", "third"]);
        state.select(2);
        let mut buf = Buffer::empty(area);
        ActionMenu::new(test_app_context())
            .placement(ActionMenuPlacement::Anchor(Position::new(39, 1)))
            .render(area, &mut buf, &mut state);

        assert_eq!(state.entry_rects[0], Rect::default());
        assert_eq!(state.entry_at(0, 0), None);
        assert_eq!(
            state.entry_at(state.entry_rects[2].x, state.entry_rects[2].y),
            Some(2),
            "the selected entry that is visible is clickable"
        );
    }

    #[test]
    fn test_the_click_on_the_popup_is_not_outside_of_it() {
        let area = Rect::new(0, 0, 80, 24);
        let mut state = menu(&["a", "b"]);
        let mut buf = Buffer::empty(area);
        ActionMenu::new(test_app_context())
            .placement(ActionMenuPlacement::Anchor(Position::new(78, 2)))
            .render(area, &mut buf, &mut state);

        let popup = state.rendered_area;
        assert!(state.contains(popup.x, popup.y));
        assert!(!state.contains(popup.right(), popup.y), "the border ends");
        assert!(!state.contains(popup.x, popup.bottom()));
        assert!(!state.contains(0, 0), "a click on the list is outside");
    }

    #[test]
    fn test_the_column_toggles_show_whether_the_column_is_shown() {
        let ctx = test_app_context();
        with_rendered_menu(&ctx, menu_with_column_toggles(), |state, buf| {
            let rows = rendered_rows(state, buf);
            for label in ["Date", "Hash", "Author"] {
                assert!(
                    rows.iter().any(|row| row.contains(&format!("✓ {label}"))),
                    "the entry of {label} is checked while the column is shown: {rows:?}"
                );
            }
        });

        // The date column is hidden, so its entry is not checked anymore
        ctx.commit_list_config.set(UiCommitListConfig {
            show_date: false,
            ..Default::default()
        });
        with_rendered_menu(&ctx, menu_with_column_toggles(), |state, buf| {
            let rows = rendered_rows(state, buf);
            assert!(
                rows.iter().any(|row| row.contains("  Date")),
                "the entry of a hidden column is not checked: {rows:?}"
            );
            assert!(
                rows.iter().any(|row| row.contains("✓ Hash")),
                "the other entries keep their state: {rows:?}"
            );
        });
    }

    #[test]
    fn test_the_separator_is_a_line_across_the_menu_that_is_never_selected() {
        let ctx = test_app_context();
        with_rendered_menu(&ctx, menu_with_column_toggles(), |state, buf| {
            let separator = state.entry_rects[2];
            assert!(
                !separator.is_empty(),
                "the separator is rendered like the other entries"
            );
            for x in separator.x..separator.right() {
                assert_eq!(
                    buf[(x, separator.y)].symbol(),
                    SEPARATOR,
                    "the separator fills its row"
                );
            }
            assert_eq!(
                state.entry_at(separator.x, separator.y),
                None,
                "the mouse cannot select the separator"
            );
            assert!(
                state.contains(separator.x, separator.y),
                "a click on the separator is inside the popup, so it does not close the menu"
            );
            assert_eq!(
                state.entry_at(state.entry_rects[3].x, state.entry_rects[3].y),
                Some(3),
                "the toggle below the separator is selectable"
            );
        });
    }

    #[test]
    fn test_the_selection_skips_the_separator() {
        let mut state = ActionMenuState::new("Actions".into(), menu_with_column_toggles());
        assert_eq!(state.selected_entry().unwrap().label(), "first");

        state.select_prev();
        assert_eq!(
            state.selected_entry().unwrap().label(),
            "first",
            "the selection stays on the first entry"
        );
        state.select_next();
        assert_eq!(state.selected_entry().unwrap().label(), "second");
        state.select_next();
        assert_eq!(
            state.selected_entry().unwrap().label(),
            "Date",
            "the separator is skipped"
        );
        state.select_next();
        assert_eq!(state.selected_entry().unwrap().label(), "Hash");
        state.select_next();
        assert_eq!(state.selected_entry().unwrap().label(), "Author");
        state.select_next();
        assert_eq!(
            state.selected_entry().unwrap().label(),
            "Author",
            "the selection does not move past the last entry"
        );
        state.select_prev();
        assert_eq!(state.selected_entry().unwrap().label(), "Hash");

        state.select(2);
        assert_eq!(
            state.selected_entry().unwrap().label(),
            "Hash",
            "the separator is not selectable, so its index is ignored"
        );
    }
}
