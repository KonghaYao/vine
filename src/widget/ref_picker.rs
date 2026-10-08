use std::rc::Rc;

use ratatui::{
    buffer::Buffer,
    crossterm::event::{Event as CrosstermEvent, KeyEvent},
    layout::{Constraint, Layout, Rect},
    style::Stylize,
    text::Span,
    widgets::{Block, Borders, Clear, Padding, StatefulWidget, Widget},
};
use tui_input::{backend::crossterm::EventHandler, Input};

use crate::{
    app::AppContext,
    git::Ref,
    widget::ref_list::{RefList, RefListState},
};

/// State of the ref picker popup.
#[derive(Debug)]
pub struct RefPickerState {
    refs: Vec<Ref>,
    query: Input,
    list: RefListState,
}

impl RefPickerState {
    pub fn new(refs: Vec<Ref>) -> Self {
        let mut state = Self {
            refs,
            query: Input::default(),
            list: RefListState::new(&[]),
        };
        state.rebuild_list();
        state
    }

    pub fn handle_input(&mut self, key: KeyEvent) {
        self.query.handle_event(&CrosstermEvent::Key(key));
        self.rebuild_list();
    }

    /// The query shown in the status line.
    pub fn query_string(&self) -> String {
        format!("/{}", self.query.value())
    }

    pub fn query_cursor_position(&self) -> u16 {
        self.query.visual_cursor() as u16 + 1 // add 1 for "/"
    }

    pub fn select_next(&mut self) {
        self.list.select_next();
    }

    pub fn select_prev(&mut self) {
        self.list.select_prev();
    }

    pub fn open_node(&mut self) {
        self.list.open_node();
    }

    pub fn close_node(&mut self) {
        self.list.close_node();
    }

    /// The selected ref name, or `None` when a category or an inner node is selected.
    pub fn selected_ref_name(&self) -> Option<String> {
        self.list.selected_leaf_ref_name()
    }

    pub fn list_state(&mut self) -> &mut RefListState {
        &mut self.list
    }

    fn rebuild_list(&mut self) {
        let query = self.query.value().to_lowercase();
        let refs: Vec<&Ref> = self
            .refs
            .iter()
            .filter(|r| query.is_empty() || r.name().to_lowercase().contains(&query))
            .collect();
        self.list = RefListState::new(&refs);
        self.list.open_all();
        self.list.select_first_leaf();
    }
}

pub struct RefPicker {
    ctx: Rc<AppContext>,
}

impl RefPicker {
    pub fn new(ctx: Rc<AppContext>) -> Self {
        Self { ctx }
    }
}

impl StatefulWidget for RefPicker {
    type State = RefPickerState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let [_, center_area, _] = Layout::horizontal([
            Constraint::Percentage(10),
            Constraint::Percentage(80),
            Constraint::Percentage(10),
        ])
        .areas(area);
        let [_, popup_area, _] = Layout::vertical([
            Constraint::Percentage(10),
            Constraint::Percentage(80),
            Constraint::Percentage(10),
        ])
        .areas(center_area);

        Widget::render(Clear, popup_area, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.ctx.color_theme.divider_fg)
            .title(Span::raw(" Select ref ").fg(self.ctx.color_theme.help_block_title_fg))
            .padding(Padding::horizontal(1));
        let inner = block.inner(popup_area);
        Widget::render(block, popup_area, buf);
        if inner.is_empty() {
            return;
        }

        let ref_list = RefList::new(self.ctx.clone());
        StatefulWidget::render(ref_list, inner, buf, state.list_state());
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};

    use super::*;

    fn refs() -> Vec<Ref> {
        vec![
            Ref::Branch {
                name: "main".into(),
                target: Default::default(),
            },
            Ref::Branch {
                name: "feature/one".into(),
                target: Default::default(),
            },
            Ref::RemoteBranch {
                name: "origin/main".into(),
                target: Default::default(),
            },
            Ref::Tag {
                name: "v1.0.0".into(),
                target: Default::default(),
            },
        ]
    }

    fn input(state: &mut RefPickerState, s: &str) {
        for c in s.chars() {
            state.handle_input(KeyEvent::from(KeyCode::Char(c)));
        }
    }

    #[test]
    fn test_selected_ref_name_steps_over_the_tree() {
        let mut state = RefPickerState::new(refs());
        // the first ref is selected when the picker is opened
        assert_eq!(state.selected_ref_name(), Some("feature/one".into()));
        assert_eq!(state.query_string(), "/");

        state.select_next();
        assert_eq!(state.selected_ref_name(), Some("main".into()));

        // category and inner nodes are not refs, so they cannot be selected
        state.select_next(); // remotes category
        assert_eq!(state.selected_ref_name(), None);
        state.select_next(); // origin
        assert_eq!(state.selected_ref_name(), None);
        state.select_next(); // origin/main
        assert_eq!(state.selected_ref_name(), Some("origin/main".into()));

        state.select_next(); // tags category
        assert_eq!(state.selected_ref_name(), None);
        state.select_next(); // v1.0.0
        assert_eq!(state.selected_ref_name(), Some("v1.0.0".into()));
    }

    #[test]
    fn test_query_filters_the_refs() {
        let mut state = RefPickerState::new(refs());
        input(&mut state, "main");

        assert_eq!(state.query_string(), "/main");
        assert_eq!(state.selected_ref_name(), Some("main".into()));

        // step over the other categories and inner nodes
        state.select_next();
        assert_eq!(state.selected_ref_name(), None);
        state.select_next();
        assert_eq!(state.selected_ref_name(), None);
        state.select_next();
        assert_eq!(state.selected_ref_name(), Some("origin/main".into()));
    }

    #[test]
    fn test_query_is_case_insensitive_and_matches_a_part_of_the_name() {
        let mut state = RefPickerState::new(refs());
        input(&mut state, "FEAT");
        assert_eq!(state.selected_ref_name(), Some("feature/one".into()));

        let mut state = RefPickerState::new(refs());
        input(&mut state, "1.0");
        assert_eq!(state.selected_ref_name(), Some("v1.0.0".into()));
    }

    #[test]
    fn test_query_without_matches_selects_no_ref() {
        let mut state = RefPickerState::new(refs());
        input(&mut state, "nothing");
        assert_eq!(state.selected_ref_name(), None);
    }

    #[test]
    fn test_removing_the_query_shows_all_refs_again() {
        let mut state = RefPickerState::new(refs());
        input(&mut state, "feature");
        assert_eq!(state.selected_ref_name(), Some("feature/one".into()));

        for _ in 0.."feature".len() {
            state.handle_input(KeyEvent::from(KeyCode::Backspace));
        }
        assert_eq!(state.query_string(), "/");
        assert_eq!(state.selected_ref_name(), Some("feature/one".into()));

        input(&mut state, "v1");
        assert_eq!(state.selected_ref_name(), Some("v1.0.0".into()));
    }

    #[test]
    fn test_query_cursor_position() {
        let mut state = RefPickerState::new(refs());
        assert_eq!(state.query_cursor_position(), 1); // the "/" prefix
        input(&mut state, "main");
        assert_eq!(state.query_cursor_position(), 5);
    }
}
