use std::rc::Rc;

use ratatui::{
    crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    layout::Rect,
    Frame,
};

use crate::{
    app::AppContext,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    view::{ListRefreshViewContext, RefreshViewContext},
    widget::commit_list::{CommitList, CommitListState, SearchState},
};

/// A scrollbar drag in progress.
#[derive(Debug, Clone, Copy)]
struct ScrollbarDrag {
    grab_offset: u16,
}

#[derive(Debug)]
pub struct ListView<'a> {
    commit_list_state: Option<CommitListState<'a>>,

    list_area: Rect,
    dragging: Option<ScrollbarDrag>,

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
        let commit_list = CommitList::new(self.ctx.clone()).scrollbar(true);
        f.render_stateful_widget(commit_list, area, self.as_mut_list_state());
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
        // Clicking a row selects that commit; the viewport does not move
        if let Some(index) = self
            .as_list_state()
            .commit_index_at_row(self.list_area, row)
        {
            self.as_mut_list_state().select_index_without_scroll(index);
        }
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
