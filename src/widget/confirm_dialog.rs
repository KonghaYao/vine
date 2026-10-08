use std::rc::Rc;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, StatefulWidget, Widget},
};

use crate::app::AppContext;

/// State of the confirmation dialog popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmDialogState {
    title: String,
    message: String,
    hint: String,
}

impl ConfirmDialogState {
    pub fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            hint: hint.into(),
        }
    }
}

pub struct ConfirmDialog {
    ctx: Rc<AppContext>,
}

impl ConfirmDialog {
    pub fn new(ctx: Rc<AppContext>) -> Self {
        Self { ctx }
    }
}

impl StatefulWidget for ConfirmDialog {
    type State = ConfirmDialogState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let width = (console::measure_text_width(&state.title)
            .max(console::measure_text_width(&state.message))
            .max(console::measure_text_width(&state.hint))
            + 2 /* padding */
            + 2/* borders */)
            .min(area.width as usize) as u16;
        let height = 4 /* message and hint lines */ + 2 /* borders */;
        let height = height.min(area.height);

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

        Widget::render(Clear, popup_area, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.ctx.color_theme.status_warn_fg)
            .title(Span::raw(format!(" {} ", state.title)).fg(self.ctx.color_theme.status_warn_fg))
            .padding(Padding::horizontal(1));
        let inner = block.inner(popup_area);
        Widget::render(block, popup_area, buf);
        if inner.is_empty() {
            return;
        }

        let lines = vec![
            Line::raw(""),
            Line::raw(state.message.clone()).add_modifier(Modifier::BOLD),
            Line::raw(""),
            Line::raw(state.hint.clone()).fg(self.ctx.color_theme.status_input_transient_fg),
        ];
        Widget::render(Paragraph::new(lines), inner, buf);
    }
}
