use std::rc::Rc;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, StatefulWidget, Widget},
};

use crate::{app::AppContext, git_action::ActionMenuEntry};

/// State of the action menu popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionMenuState {
    title: String,
    entries: Vec<ActionMenuEntry>,
    selected: usize,
    offset: usize,
}

impl ActionMenuState {
    pub fn new(title: String, entries: Vec<ActionMenuEntry>) -> Self {
        Self {
            title,
            entries,
            selected: 0,
            offset: 0,
        }
    }

    pub fn selected_entry(&self) -> Option<&ActionMenuEntry> {
        self.entries.get(self.selected)
    }

    pub fn select_next(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
        }
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }
}

pub struct ActionMenu {
    ctx: Rc<AppContext>,
}

impl ActionMenu {
    pub fn new(ctx: Rc<AppContext>) -> Self {
        Self { ctx }
    }

    /// The area of the popup in the given area.
    pub fn popup_area(&self, area: Rect, state: &ActionMenuState) -> Rect {
        popup_area(area, &state.title, &state.entries)
    }
}

impl StatefulWidget for ActionMenu {
    type State = ActionMenuState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let area = self.popup_area(area, state);
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
        if inner.is_empty() {
            return;
        }

        let height = inner.height as usize;
        if state.selected < state.offset {
            state.offset = state.selected;
        } else if state.selected >= state.offset + height {
            state.offset = state.selected + 1 - height;
        }

        let lines = state
            .entries
            .iter()
            .skip(state.offset)
            .take(height)
            .map(|entry| {
                let label = if entry.has_submenu() {
                    format!("{} {}", entry.label(), "▸")
                } else {
                    entry.label().to_string()
                };
                Line::raw(label)
            })
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

fn popup_area(area: Rect, title: &str, entries: &[ActionMenuEntry]) -> Rect {
    let max_label_width = entries
        .iter()
        .map(|e| console::measure_text_width(e.label()) + if e.has_submenu() { 2 } else { 0 })
        .max()
        .unwrap_or_default();
    let width = (max_label_width + 2 /* padding */ + 2/* borders */)
        .max(console::measure_text_width(title) + 4)
        .min(area.width as usize) as u16;
    let height = (entries.len() + 2/* borders */)
        .max(3)
        .min(area.height as usize) as u16;

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
