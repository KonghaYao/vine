use std::rc::Rc;

use ratatui::{
    crossterm::event::{
        Event as CrosstermEvent, KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind,
    },
    layout::Rect,
    Frame,
};
use tui_input::{backend::crossterm::EventHandler, Input};

use crate::{
    app::AppContext,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    git::Ref,
    git_action::{needs_confirmation, ActionMenuEntry, GitAction, MenuAction},
    view::View,
    widget::{
        action_menu::{ActionMenu, ActionMenuPlacement, ActionMenuState},
        confirm_dialog::{ConfirmDialog, ConfirmDialogState},
        ref_picker::{RefPicker, RefPickerState},
    },
};

/// The answer of the confirmation dialog for a key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmationAnswer {
    /// The action is confirmed (`y`).
    Confirm,
    /// The action is cancelled (`n`, `Esc` or `Enter`).
    Cancel,
    /// The dialog is kept open.
    Pending,
}

/// The answer of the confirmation dialog for the given key event.
///
/// The dialog is answered with `y` or `n`; `Esc` (and any other key that is not `y`) cancels it,
/// so that the menu is shown again.
fn confirmation_answer(event: UserEvent, key: KeyEvent) -> ConfirmationAnswer {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => ConfirmationAnswer::Confirm,
        KeyCode::Char('n') | KeyCode::Char('N') => ConfirmationAnswer::Cancel,
        _ => match event {
            UserEvent::Confirm | UserEvent::Cancel | UserEvent::Close => ConfirmationAnswer::Cancel,
            _ => ConfirmationAnswer::Pending,
        },
    }
}

/// The popup rendered with the action menu.
#[derive(Debug)]
enum ActionPopup {
    /// The action menu itself.
    None,
    /// The name of the action is being entered in the status line.
    Input { action: MenuAction, input: Input },
    /// The action is confirmed by the user.
    Confirm { action: GitAction },
    /// The ref of the action is being selected.
    RefPicker {
        action: MenuAction,
        picker: RefPickerState,
    },
}

/// The action menu overlay.
///
/// The menu is rendered over the view it was opened from, and the actions are executed by the
/// application (see [`AppEvent::ExecuteGitAction`]).
#[derive(Debug)]
pub struct ActionMenuView<'a> {
    before: View<'a>,

    /// The menu stack; the last menu is the current one.
    menus: Vec<ActionMenuState>,
    popup: ActionPopup,

    /// Whether the menu floats next to the more button it was opened from, or is centered.
    placement: ActionMenuPlacement,

    /// All refs of the repository, used by the ref picker.
    refs: Vec<Ref>,

    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> ActionMenuView<'a> {
    pub fn new(
        before: View<'a>,
        title: String,
        entries: Vec<ActionMenuEntry>,
        refs: Vec<Ref>,
        placement: ActionMenuPlacement,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> ActionMenuView<'a> {
        ActionMenuView {
            before,
            menus: vec![ActionMenuState::new(title, entries)],
            popup: ActionPopup::None,
            placement,
            refs,
            ctx,
            tx,
        }
    }

    /// Whether the menu floats next to the cell it was anchored to, like a tooltip.
    fn is_floating(&self) -> bool {
        matches!(self.placement, ActionMenuPlacement::Anchor(_))
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        let event = event_with_count.event;
        match std::mem::replace(&mut self.popup, ActionPopup::None) {
            ActionPopup::None => self.handle_menu_event(event),
            ActionPopup::Input { action, input } => {
                self.popup = self.handle_input_event(action, input, event, key);
            }
            ActionPopup::Confirm { action } => {
                self.popup = self.handle_confirm_event(action, event, key);
            }
            ActionPopup::RefPicker { action, picker } => {
                self.popup = self.handle_ref_picker_event(action, picker, event, key);
            }
        }
    }

    /// Handle the mouse, which drives the menu that was opened from the more button of a row.
    ///
    /// The centered menu keeps the keyboard only behaviour it always had. The tooltip menu selects
    /// the entry under the cursor, executes the entry that is clicked, and closes when the click
    /// lands outside the popup. A click is never passed to the view under the menu: the list, the
    /// more button and the scrollbar of the list do not see the clicks while the menu is open.
    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        // The sub menus, the confirmation dialog and the ref picker are keyboard only
        if !self.is_floating() || !matches!(self.popup, ActionPopup::None) {
            return;
        }
        let Some(menu) = self.menus.last() else {
            return;
        };
        let entry = menu.entry_at(mouse.column, mouse.row);
        let outside = !menu.contains(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Moved => {
                // Hovering an entry highlights it, like the cursor of the keyboard
                if let Some(index) = entry {
                    self.select(index);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = entry {
                    // A click executes the entry, like pressing enter on it
                    self.select(index);
                    self.select_entry();
                } else if outside {
                    self.tx.send(AppEvent::CloseActions);
                }
            }
            MouseEventKind::Down(MouseButton::Right) if outside => {
                self.tx.send(AppEvent::CloseActions);
            }
            _ => {}
        }
    }

    /// Select the entry with the given index of the current menu.
    fn select(&mut self, index: usize) {
        if let Some(menu) = self.menus.last_mut() {
            menu.select(index);
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.before.render(f, area);

        if let Some(menu) = self.menus.last_mut() {
            let action_menu = ActionMenu::new(self.ctx.clone()).placement(self.placement);
            f.render_stateful_widget(action_menu, area, menu);
        }

        match &mut self.popup {
            ActionPopup::None | ActionPopup::Input { .. } => {}
            ActionPopup::Confirm { action } => {
                let message = format!("{}?", action.success_summary());
                let mut state = ConfirmDialogState::new("Confirm", message, "y: yes  n: no");
                let dialog = ConfirmDialog::new(self.ctx.clone());
                f.render_stateful_widget(dialog, area, &mut state);
            }
            ActionPopup::RefPicker { picker, .. } => {
                let ref_picker = RefPicker::new(self.ctx.clone());
                f.render_stateful_widget(ref_picker, area, picker);
            }
        }
    }

    pub fn update_layout(&mut self, area: Rect) {
        self.before.update_layout(area);
    }

    pub fn prepare_graph_uploads(&mut self) {
        self.before.prepare_graph_uploads();
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        self.before.drain_pending_graph_uploads()
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        self.before.graph_image_ids_sorted()
    }

    pub fn take_before_view(&mut self) -> View<'a> {
        std::mem::take(&mut self.before)
    }
}

impl ActionMenuView<'_> {
    fn handle_menu_event(&mut self, event: UserEvent) {
        match event {
            UserEvent::Quit => {
                self.tx.send(AppEvent::Quit);
            }
            UserEvent::Cancel | UserEvent::Close => {
                if self.menus.len() > 1 {
                    self.menus.pop();
                } else {
                    self.tx.send(AppEvent::CloseActions);
                }
            }
            UserEvent::NavigateDown | UserEvent::SelectDown => {
                if let Some(menu) = self.menus.last_mut() {
                    menu.select_next();
                }
            }
            UserEvent::NavigateUp | UserEvent::SelectUp => {
                if let Some(menu) = self.menus.last_mut() {
                    menu.select_prev();
                }
            }
            UserEvent::Confirm => {
                self.select_entry();
            }
            _ => {}
        }
    }

    fn select_entry(&mut self) {
        let Some(entry) = self
            .menus
            .last()
            .and_then(|menu| menu.selected_entry().cloned())
        else {
            return;
        };
        match entry {
            ActionMenuEntry::SubMenu { label, entries } => {
                self.menus.push(ActionMenuState::new(label, entries));
            }
            ActionMenuEntry::Action { action, .. } => self.start_action(action),
            // The menu is kept open, so that several columns can be toggled in a row
            ActionMenuEntry::Toggle { column, .. } => {
                self.tx.send(AppEvent::ToggleCommitListColumn(column));
            }
            // A separator is not selectable, so it is never reached; nothing is run just in case
            ActionMenuEntry::Separator => {}
        }
    }

    fn start_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::Action(action) => self.run_action(action),
            MenuAction::CreateBranch { .. }
            | MenuAction::CreateTag { .. }
            | MenuAction::RenameBranch { .. } => self.start_name_input(action),
            MenuAction::MergeRef | MenuAction::RebaseRef => self.start_ref_picker(action),
        }
    }

    /// Runs the action, asking for a confirmation first when the action is destructive.
    fn run_action(&mut self, action: GitAction) {
        if self.needs_confirmation(&action) {
            self.popup = ActionPopup::Confirm { action };
        } else {
            self.execute_action(action);
        }
    }

    fn needs_confirmation(&self, action: &GitAction) -> bool {
        needs_confirmation(action, self.ctx.actions_config.confirm_destructive)
    }

    fn execute_action(&mut self, action: GitAction) {
        self.tx.send(AppEvent::CloseActions);
        match action {
            GitAction::CopyCommitHash { commit_hash } => {
                self.tx.send(AppEvent::CopyToClipboard {
                    name: "Commit SHA".into(),
                    value: commit_hash.as_str().into(),
                });
            }
            GitAction::ShowCommitDetail => {
                self.tx.send(AppEvent::OpenDetail);
            }
            GitAction::RunUserCommand { number } => {
                self.tx.send(AppEvent::OpenUserCommand(number));
            }
            action => {
                self.tx.send(AppEvent::ExecuteGitAction(Box::new(action)));
            }
        }
    }

    fn start_name_input(&mut self, action: MenuAction) {
        let input = Input::default();
        self.update_input_status(&action, &input);
        self.popup = ActionPopup::Input { action, input };
    }

    fn handle_input_event(
        &mut self,
        action: MenuAction,
        mut input: Input,
        event: UserEvent,
        key: KeyEvent,
    ) -> ActionPopup {
        match event {
            UserEvent::Confirm => {
                let name = input.value().trim();
                if name.is_empty() {
                    return ActionPopup::Input { action, input };
                }
                self.tx.send(AppEvent::ClearStatusLine);
                let Some(git_action) = action.into_action_with_name(name) else {
                    return ActionPopup::None;
                };
                self.run_action(git_action);
                ActionPopup::None
            }
            UserEvent::Cancel => {
                self.tx.send(AppEvent::ClearStatusLine);
                ActionPopup::None
            }
            _ => {
                input.handle_event(&CrosstermEvent::Key(key));
                self.update_input_status(&action, &input);
                ActionPopup::Input { action, input }
            }
        }
    }

    fn update_input_status(&self, action: &MenuAction, input: &Input) {
        let prompt = match action {
            MenuAction::CreateBranch { .. } => "New branch name: ".to_string(),
            MenuAction::CreateTag { .. } => "New tag name: ".to_string(),
            MenuAction::RenameBranch { old_name } => format!("New name for '{old_name}': "),
            _ => String::new(),
        };
        let cursor_position = prompt.len() as u16 + input.visual_cursor() as u16;
        self.tx.send(AppEvent::UpdateStatusInput {
            message: format!("{prompt}{}", input.value()),
            cursor_position,
            metadata: "Enter: confirm  Esc: cancel".into(),
        });
    }

    fn start_ref_picker(&mut self, action: MenuAction) {
        let picker = RefPickerState::new(self.refs.clone());
        self.update_picker_status(&picker);
        self.popup = ActionPopup::RefPicker { action, picker };
    }

    fn handle_ref_picker_event(
        &mut self,
        action: MenuAction,
        mut picker: RefPickerState,
        event: UserEvent,
        key: KeyEvent,
    ) -> ActionPopup {
        match event {
            UserEvent::Confirm => {
                let Some(name) = picker.selected_ref_name() else {
                    // a category node is selected, so there is nothing to run yet
                    return ActionPopup::RefPicker { action, picker };
                };
                self.tx.send(AppEvent::ClearStatusLine);
                let Some(git_action) = action.into_action_with_ref(&name) else {
                    return ActionPopup::None;
                };
                self.run_action(git_action);
                return ActionPopup::None;
            }
            UserEvent::Cancel => {
                self.tx.send(AppEvent::ClearStatusLine);
                return ActionPopup::None;
            }
            UserEvent::NavigateDown | UserEvent::SelectDown => {
                picker.select_next();
            }
            UserEvent::NavigateUp | UserEvent::SelectUp => {
                picker.select_prev();
            }
            UserEvent::NavigateRight => {
                picker.open_node();
            }
            UserEvent::NavigateLeft => {
                picker.close_node();
            }
            _ => {
                picker.handle_input(key);
                self.update_picker_status(&picker);
            }
        }
        ActionPopup::RefPicker { action, picker }
    }

    fn update_picker_status(&self, picker: &RefPickerState) {
        self.tx.send(AppEvent::UpdateStatusInput {
            message: picker.query_string(),
            cursor_position: picker.query_cursor_position(),
            metadata: "Enter: select  Esc: cancel".into(),
        });
    }

    fn handle_confirm_event(
        &mut self,
        action: GitAction,
        event: UserEvent,
        key: KeyEvent,
    ) -> ActionPopup {
        match confirmation_answer(event, key) {
            ConfirmationAnswer::Confirm => {
                self.execute_action(action);
                ActionPopup::None
            }
            // The action is not confirmed, so the menu is shown again
            ConfirmationAnswer::Cancel => ActionPopup::None,
            ConfirmationAnswer::Pending => ActionPopup::Confirm { action },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn press(c: char) -> KeyEvent {
        KeyEvent::from(KeyCode::Char(c))
    }

    #[test]
    fn test_confirmation_dialog_is_answered_with_y_and_n() {
        assert_eq!(
            confirmation_answer(UserEvent::Unknown, press('y')),
            ConfirmationAnswer::Confirm
        );
        assert_eq!(
            confirmation_answer(UserEvent::Unknown, press('Y')),
            ConfirmationAnswer::Confirm
        );
        assert_eq!(
            confirmation_answer(UserEvent::Unknown, press('n')),
            ConfirmationAnswer::Cancel
        );
        assert_eq!(
            confirmation_answer(UserEvent::Unknown, press('N')),
            ConfirmationAnswer::Cancel
        );
    }

    #[test]
    fn test_confirmation_dialog_is_cancelled_by_escape_and_enter() {
        for event in [UserEvent::Cancel, UserEvent::Close, UserEvent::Confirm] {
            assert_eq!(
                confirmation_answer(event, KeyEvent::from(KeyCode::Esc)),
                ConfirmationAnswer::Cancel,
                "{event:?}"
            );
        }
    }

    #[test]
    fn test_confirmation_dialog_ignores_other_keys() {
        // `n` is bound to `go_to_next` by default, so it must not be treated as navigation
        assert_eq!(
            confirmation_answer(UserEvent::GoToNext, press('n')),
            ConfirmationAnswer::Cancel
        );
        for (event, key) in [
            (UserEvent::NavigateDown, KeyEvent::from(KeyCode::Down)),
            (UserEvent::Quit, press('q')),
            (UserEvent::Unknown, KeyEvent::from(KeyCode::Enter)),
        ] {
            assert_eq!(
                confirmation_answer(event, key),
                ConfirmationAnswer::Pending,
                "{event:?} {key:?}"
            );
        }
    }

    use crate::{
        color::ColorTheme,
        config::{ActionsConfig, CommitListColumn, CoreConfig, UiCommitListConfig, UiConfig},
        event::{Receiver, Sender},
        git::CommitHash,
        keybind::KeyBind,
        protocol::ImageProtocol,
    };
    use ratatui::{buffer::Buffer, layout::Position, widgets::StatefulWidget};

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

    /// An entry that runs the given action, without any submenu.
    fn entry(label: &str, action: GitAction) -> ActionMenuEntry {
        ActionMenuEntry::Action {
            label: label.into(),
            action: MenuAction::Action(action),
        }
    }

    fn copy_hash_entry() -> ActionMenuEntry {
        entry(
            "Copy commit hash",
            GitAction::CopyCommitHash {
                commit_hash: CommitHash::default(),
            },
        )
    }

    fn detail_entry() -> ActionMenuEntry {
        entry("Show commit details", GitAction::ShowCommitDetail)
    }

    /// An entry that shows or hides a column of the commit list.
    fn column_toggle(column: CommitListColumn) -> ActionMenuEntry {
        ActionMenuEntry::Toggle {
            label: column.label().into(),
            column,
        }
    }

    /// The entries of a menu that ends with a separator and the column toggles.
    fn entries_with_column_toggles() -> Vec<ActionMenuEntry> {
        vec![
            copy_hash_entry(),
            ActionMenuEntry::Separator,
            column_toggle(CommitListColumn::Date),
            column_toggle(CommitListColumn::Author),
        ]
    }

    /// The view of a menu floating next to the more button of a row near the right edge.
    fn floating_view(entries: Vec<ActionMenuEntry>) -> (ActionMenuView<'static>, Receiver) {
        let (tx, rx) = Sender::channel();
        let view = ActionMenuView::new(
            View::Default,
            "Actions".into(),
            entries,
            Vec::new(),
            ActionMenuPlacement::Anchor(Position::new(78, 2)),
            test_app_context(),
            tx,
        );
        (view, rx)
    }

    /// The view of a menu that is opened with the keyboard or a right click.
    fn centered_view(entries: Vec<ActionMenuEntry>) -> (ActionMenuView<'static>, Receiver) {
        let (tx, rx) = Sender::channel();
        let view = ActionMenuView::new(
            View::Default,
            "Actions".into(),
            entries,
            Vec::new(),
            ActionMenuPlacement::Center,
            test_app_context(),
            tx,
        );
        (view, rx)
    }

    /// Renders the menu, which records the popup and the entries on the screen.
    fn render(view: &mut ActionMenuView<'static>, area: Rect) {
        let mut buf = Buffer::empty(area);
        let placement = view.placement;
        let menu = view.menus.last_mut().unwrap();
        let action_menu = ActionMenu::new(test_app_context()).placement(placement);
        StatefulWidget::render(action_menu, area, &mut buf, menu);
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        }
    }

    fn area() -> Rect {
        Rect::new(0, 0, 80, 24)
    }

    /// The position of the entry with the given index of the menu, found by scanning the area.
    ///
    /// Only the hit test of the menu is used, so that the tests click on exactly the cells the menu
    /// considers to be part of the entry, whatever the popup was rendered in.
    fn entry_position(view: &ActionMenuView<'static>, area: Rect, index: usize) -> Position {
        scan(view, area, |menu, position| {
            menu.entry_at(position.x, position.y) == Some(index)
        })
        .unwrap_or_else(|| panic!("the entry {index} is not rendered in {area:?}"))
    }

    /// A position of the area that is inside the popup but outside of its entries.
    fn popup_position_without_entry(view: &ActionMenuView<'static>, area: Rect) -> Position {
        scan(view, area, |menu, position| {
            menu.contains(position.x, position.y) && menu.entry_at(position.x, position.y).is_none()
        })
        .expect("the popup has a border and a title")
    }

    /// A position of the area that is outside of the popup.
    fn outside_position(view: &ActionMenuView<'static>, area: Rect) -> Position {
        scan(view, area, |menu, position| {
            !menu.contains(position.x, position.y)
        })
        .expect("the area is larger than the popup")
    }

    fn scan(
        view: &ActionMenuView<'static>,
        area: Rect,
        predicate: impl Fn(&ActionMenuState, Position) -> bool,
    ) -> Option<Position> {
        let menu = view.menus.last().unwrap();
        (area.top()..area.bottom())
            .flat_map(|y| (area.left()..area.right()).map(move |x| Position::new(x, y)))
            .find(|position| predicate(menu, *position))
    }

    #[test]
    fn test_the_mouse_highlights_the_entry_under_the_cursor() {
        let (mut view, _rx) = floating_view(vec![copy_hash_entry(), detail_entry()]);
        render(&mut view, area());
        let second = entry_position(&view, area(), 1);
        view.handle_mouse_event(mouse(MouseEventKind::Moved, second.x, second.y));
        assert_eq!(
            view.menus[0].selected_entry().unwrap().label(),
            "Show commit details"
        );
    }

    #[test]
    fn test_the_click_on_an_entry_runs_the_action_of_the_entry() {
        let (mut view, rx) = floating_view(vec![copy_hash_entry(), detail_entry()]);
        render(&mut view, area());
        let second = entry_position(&view, area(), 1);
        view.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            second.x,
            second.y,
        ));
        assert!(
            matches!(rx.try_recv(), Some(AppEvent::CloseActions)),
            "the menu is closed when the action runs"
        );
        assert!(
            matches!(rx.try_recv(), Some(AppEvent::OpenDetail)),
            "the clicked entry is the one that runs, like pressing enter on it"
        );
        assert!(rx.try_recv().is_none());
    }

    #[test]
    fn test_the_click_on_an_entry_runs_the_action_at_the_flipped_position() {
        let (mut view, rx) = floating_view(vec![copy_hash_entry(), detail_entry()]);
        render(&mut view, area());
        let second = entry_position(&view, area(), 1);
        assert!(
            second.x + 1 < 78,
            "the popup is flipped to the left of the anchor: {second:?}"
        );
        view.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            second.x,
            second.y,
        ));
        assert!(matches!(rx.try_recv(), Some(AppEvent::CloseActions)));
        assert!(matches!(rx.try_recv(), Some(AppEvent::OpenDetail)));
    }

    #[test]
    fn test_the_click_outside_of_the_popup_closes_the_menu() {
        let (mut view, rx) = floating_view(vec![copy_hash_entry(), detail_entry()]);
        render(&mut view, area());
        let outside = outside_position(&view, area());
        view.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            outside.x,
            outside.y,
        ));
        assert!(matches!(rx.try_recv(), Some(AppEvent::CloseActions)));
        assert!(
            rx.try_recv().is_none(),
            "a click on the list does not run an action"
        );
        assert!(
            view.menus[0].selected_entry().is_some(),
            "the menu is not changed, it is only closed"
        );
    }

    #[test]
    fn test_the_click_on_the_popup_but_not_on_an_entry_is_ignored() {
        let (mut view, rx) = floating_view(vec![copy_hash_entry(), detail_entry()]);
        render(&mut view, area());
        // The title row of the popup is not one of its entries
        let title = popup_position_without_entry(&view, area());
        view.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            title.x,
            title.y,
        ));
        assert!(
            rx.try_recv().is_none(),
            "a click on the popup neither runs an action nor closes the menu"
        );
    }

    #[test]
    fn test_the_mouse_does_not_drive_the_centered_menu() {
        let (mut view, rx) = centered_view(vec![copy_hash_entry(), detail_entry()]);
        render(&mut view, area());
        let first = entry_position(&view, area(), 0);
        view.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            first.x,
            first.y,
        ));
        view.handle_mouse_event(mouse(MouseEventKind::Moved, first.x, first.y));
        assert!(
            rx.try_recv().is_none(),
            "the centered menu is keyboard only, like it always was"
        );
        assert_eq!(
            view.menus[0].selected_entry().unwrap().label(),
            "Copy commit hash",
            "the entry under the cursor is not selected"
        );
    }

    #[test]
    fn test_the_submenu_is_driven_by_the_mouse_like_the_first_menu() {
        let (mut view, rx) = floating_view(vec![ActionMenuEntry::SubMenu {
            label: "Branch".into(),
            entries: vec![detail_entry()],
        }]);
        let confirm = UserEventWithCount::from_event(UserEvent::Confirm);
        view.handle_event(confirm, KeyEvent::from(KeyCode::Enter));
        assert_eq!(view.menus.len(), 2, "the submenu is open");
        render(&mut view, area());
        // The last menu is the submenu, so it is the one that is scanned
        let entry = entry_position(&view, area(), 0);
        view.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            entry.x,
            entry.y,
        ));
        assert!(
            matches!(rx.try_recv(), Some(AppEvent::CloseActions)),
            "the entry of the submenu runs when it is clicked"
        );
        assert!(matches!(rx.try_recv(), Some(AppEvent::OpenDetail)));
    }

    #[test]
    fn test_the_click_on_an_entry_runs_it_after_the_window_is_resized() {
        let (mut view, rx) = floating_view(vec![copy_hash_entry(), detail_entry()]);
        render(&mut view, area());
        // The window is resized, so the popup moves and the entries are rendered elsewhere
        let small = Rect::new(0, 0, 40, 10);
        render(&mut view, small);
        let second = entry_position(&view, small, 1);
        view.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            second.x,
            second.y,
        ));
        assert!(matches!(rx.try_recv(), Some(AppEvent::CloseActions)));
        assert!(matches!(rx.try_recv(), Some(AppEvent::OpenDetail)));
    }

    #[test]
    fn test_the_click_on_an_entry_runs_it_when_the_popup_is_flipped_up() {
        let (mut view, rx) = floating_view(vec![copy_hash_entry(), detail_entry()]);
        // The anchor is in the last row of a short area, so the popup is flipped above it
        let area = Rect::new(0, 0, 80, 5);
        render(&mut view, area);
        let second = entry_position(&view, area, 1);
        assert!(
            second.y <= 2,
            "the popup is not below the anchor at {second:?}"
        );
        view.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            second.x,
            second.y,
        ));
        assert!(matches!(rx.try_recv(), Some(AppEvent::CloseActions)));
        assert!(matches!(rx.try_recv(), Some(AppEvent::OpenDetail)));
    }

    #[test]
    fn test_the_confirmation_dialog_is_not_driven_by_the_mouse() {
        let (mut view, rx) = floating_view(vec![copy_hash_entry()]);
        view.popup = ActionPopup::Confirm {
            action: GitAction::ShowCommitDetail,
        };
        view.handle_mouse_event(mouse(MouseEventKind::Moved, 10, 10));
        view.handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), 10, 10));
        assert!(
            rx.try_recv().is_none(),
            "the dialog is keyboard only, and a click does not run the action"
        );
        assert!(matches!(view.popup, ActionPopup::Confirm { .. }));
    }

    #[test]
    fn test_the_click_on_a_column_toggle_sends_it_and_keeps_the_menu_open() {
        let (mut view, rx) = floating_view(entries_with_column_toggles());
        render(&mut view, area());
        // The toggle of the date is the third entry; the separator before it is not an entry for
        // the mouse, so its index is never returned by the hit test
        let date = entry_position(&view, area(), 2);
        assert_eq!(
            view.menus[0].selected_entry().unwrap().label(),
            "Copy commit hash"
        );
        view.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            date.x,
            date.y,
        ));
        assert!(
            matches!(
                rx.try_recv(),
                Some(AppEvent::ToggleCommitListColumn(CommitListColumn::Date))
            ),
            "the clicked toggle is the one that is sent"
        );
        assert!(
            rx.try_recv().is_none(),
            "the menu is not closed, so several columns can be toggled in a row"
        );
        assert_eq!(
            view.menus[0].selected_entry().unwrap().label(),
            "Date",
            "the toggle that is clicked is selected"
        );
    }

    #[test]
    fn test_the_keyboard_toggles_the_column_below_the_separator() {
        let (mut view, rx) = floating_view(entries_with_column_toggles());
        let down = UserEventWithCount::from_event(UserEvent::NavigateDown);
        let enter = UserEventWithCount::from_event(UserEvent::Confirm);
        // The separator is skipped, so the first step down is the date toggle
        view.handle_event(down, KeyEvent::from(KeyCode::Down));
        assert_eq!(view.menus[0].selected_entry().unwrap().label(), "Date");
        view.handle_event(enter, KeyEvent::from(KeyCode::Enter));
        assert!(matches!(
            rx.try_recv(),
            Some(AppEvent::ToggleCommitListColumn(CommitListColumn::Date))
        ));
        assert!(rx.try_recv().is_none());

        let up = UserEventWithCount::from_event(UserEvent::NavigateUp);
        view.handle_event(up, KeyEvent::from(KeyCode::Up));
        assert_eq!(
            view.menus[0].selected_entry().unwrap().label(),
            "Copy commit hash",
            "the separator is skipped upwards too"
        );
    }

    #[test]
    fn test_the_click_on_the_separator_does_nothing() {
        let (mut view, rx) = floating_view(entries_with_column_toggles());
        render(&mut view, area());
        // The row of the separator is inside the popup, so it is not one of its entries
        let separator = separator_position(&view, area());
        view.handle_mouse_event(mouse(MouseEventKind::Moved, separator.x, separator.y));
        assert_eq!(
            view.menus[0].selected_entry().unwrap().label(),
            "Copy commit hash",
            "hovering the separator does not change the selection"
        );
        view.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            separator.x,
            separator.y,
        ));
        assert!(
            rx.try_recv().is_none(),
            "a click on the separator is neither an action nor a click outside the popup"
        );
    }

    /// A position of the row of the separator, which is the row above the date toggle.
    fn separator_position(view: &ActionMenuView<'static>, area: Rect) -> Position {
        let date = entry_position(view, area, 2);
        let position = Position::new(date.x, date.y - 1);
        assert!(
            view.menus
                .last()
                .unwrap()
                .entry_at(position.x, position.y)
                .is_none(),
            "the separator is not one of the entries of the menu"
        );
        position
    }
}
