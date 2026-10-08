use std::rc::Rc;

use ratatui::{
    crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent, MouseEvent},
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
        action_menu::{ActionMenu, ActionMenuState},
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
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> ActionMenuView<'a> {
        ActionMenuView {
            before,
            menus: vec![ActionMenuState::new(title, entries)],
            popup: ActionPopup::None,
            refs,
            ctx,
            tx,
        }
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

    pub fn handle_mouse_event(&mut self, _mouse: MouseEvent) {
        // The action menu is keyboard only
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.before.render(f, area);

        if let Some(menu) = self.menus.last_mut() {
            let action_menu = ActionMenu::new(self.ctx.clone());
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
}
