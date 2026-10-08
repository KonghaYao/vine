use std::rc::Rc;

use ratatui::{
    crossterm::event::{KeyEvent, MouseEvent},
    layout::Rect,
    Frame,
};

use crate::{
    app::AppContext,
    event::{Sender, UserEventWithCount},
    git::{Commit, CommitHash, FileChange, Ref},
    git_action::ActionMenuEntry,
    search::SearchOptions,
    view::{
        action_menu::ActionMenuView, detail::DetailView, help::HelpView, list::ListView,
        refs::RefsView, user_command::UserCommandView,
    },
    widget::commit_list::{CommitListState, SearchRefreshContext, SearchState},
};

#[derive(Debug, Default)]
pub enum View<'a> {
    #[default]
    Default, // dummy variant to make #[default] work
    List(Box<ListView<'a>>),
    ActionMenu(Box<ActionMenuView<'a>>),
    Detail(Box<DetailView<'a>>),
    UserCommand(Box<UserCommandView<'a>>),
    Refs(Box<RefsView<'a>>),
    Help(Box<HelpView<'a>>),
}

impl<'a> View<'a> {
    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key_event: KeyEvent) {
        match self {
            View::Default => {}
            View::List(view) => view.handle_event(event_with_count, key_event),
            View::ActionMenu(view) => view.handle_event(event_with_count, key_event),
            View::Detail(view) => view.handle_event(event_with_count, key_event),
            View::UserCommand(view) => view.handle_event(event_with_count, key_event),
            View::Refs(view) => view.handle_event(event_with_count, key_event),
            View::Help(view) => view.handle_event(event_with_count, key_event),
        }
    }

    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        match self {
            View::Default => {}
            View::List(view) => view.handle_mouse_event(mouse),
            View::ActionMenu(view) => view.handle_mouse_event(mouse),
            View::Detail(view) => view.handle_mouse_event(mouse),
            View::UserCommand(view) => view.handle_mouse_event(mouse),
            View::Refs(view) => view.handle_mouse_event(mouse),
            View::Help(view) => view.handle_mouse_event(mouse),
        }
    }

    /// Whether the view should receive all key events as they are.
    ///
    /// For example, the action menu handles keys that are not bound to any user event (like the
    /// y/n of the confirmation dialog).
    pub fn wants_raw_key_events(&self) -> bool {
        matches!(self, View::ActionMenu(_))
    }

    /// Whether the view currently ignores mouse events.
    ///
    /// For example, mouse events are ignored while the search query is being entered.
    pub fn ignores_mouse_event(&self) -> bool {
        match self {
            View::List(view) => matches!(
                view.as_list_state().search_state(),
                SearchState::Searching { .. }
            ),
            _ => false,
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        match self {
            View::Default => {}
            View::List(view) => view.render(f, area),
            View::ActionMenu(view) => view.render(f, area),
            View::Detail(view) => view.render(f, area),
            View::UserCommand(view) => view.render(f, area),
            View::Refs(view) => view.render(f, area),
            View::Help(view) => view.render(f, area),
        }
    }

    pub fn update_layout(&mut self, area: Rect) {
        match self {
            View::Default => {}
            View::List(view) => view.update_layout(area),
            View::ActionMenu(view) => view.update_layout(area),
            View::Detail(view) => view.update_layout(area),
            View::UserCommand(view) => view.update_layout(area),
            View::Refs(view) => view.update_layout(area),
            View::Help(_) => {}
        }
    }

    pub fn prepare_graph_uploads(&mut self) {
        match self {
            View::Default => {}
            View::List(view) => view.prepare_graph_uploads(),
            View::ActionMenu(view) => view.prepare_graph_uploads(),
            View::Detail(view) => view.prepare_graph_uploads(),
            View::UserCommand(view) => view.prepare_graph_uploads(),
            View::Refs(view) => view.prepare_graph_uploads(),
            View::Help(_) => {}
        }
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        match self {
            View::Default => Vec::new(),
            View::List(view) => view.drain_pending_graph_uploads(),
            View::ActionMenu(view) => view.drain_pending_graph_uploads(),
            View::Detail(view) => view.drain_pending_graph_uploads(),
            View::UserCommand(view) => view.drain_pending_graph_uploads(),
            View::Refs(view) => view.drain_pending_graph_uploads(),
            View::Help(_) => Vec::new(),
        }
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        match self {
            View::Default => Vec::new(),
            View::List(view) => view.graph_image_ids_sorted(),
            View::ActionMenu(view) => view.graph_image_ids_sorted(),
            View::Detail(view) => view.graph_image_ids_sorted(),
            View::UserCommand(view) => view.graph_image_ids_sorted(),
            View::Refs(view) => view.graph_image_ids_sorted(),
            View::Help(view) => view.graph_image_ids_sorted(),
        }
    }

    pub fn of_list(
        commit_list_state: CommitListState<'a>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        View::List(Box::new(ListView::new(commit_list_state, ctx, tx)))
    }

    pub fn of_detail(
        commit_list_state: CommitListState<'a>,
        commit: Commit,
        changes: Vec<FileChange>,
        refs: Vec<Ref>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        View::Detail(Box::new(DetailView::new(
            commit_list_state,
            commit,
            changes,
            refs,
            ctx,
            tx,
        )))
    }

    pub fn of_user_command(
        commit_list_state: CommitListState<'a>,
        command_output: String,
        user_command_number: Option<usize>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        View::UserCommand(Box::new(UserCommandView::new(
            commit_list_state,
            command_output,
            user_command_number,
            ctx,
            tx,
        )))
    }

    pub fn of_refs(
        commit_list_state: CommitListState<'a>,
        refs: &[&Ref],
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        View::Refs(Box::new(RefsView::new(commit_list_state, refs, ctx, tx)))
    }

    pub fn of_action_menu(
        before: View<'a>,
        title: String,
        entries: Vec<ActionMenuEntry>,
        refs: Vec<Ref>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        View::ActionMenu(Box::new(ActionMenuView::new(
            before, title, entries, refs, ctx, tx,
        )))
    }

    /// The view that renders the output of a git action.
    ///
    /// The output is not bound to a user command, so it is rendered with no user command number.
    pub fn of_action_output(
        commit_list_state: CommitListState<'a>,
        output: String,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        View::of_user_command(commit_list_state, output, None, ctx, tx)
    }

    pub fn of_help(before: View<'a>, ctx: Rc<AppContext>, tx: Sender) -> Self {
        View::Help(Box::new(HelpView::new(before, ctx, tx)))
    }

    pub fn refresh(&mut self) {
        match self {
            View::Default => {}
            View::List(view) => view.refresh(),
            View::ActionMenu(_) => {}
            View::Detail(view) => view.refresh(),
            View::UserCommand(view) => view.refresh(),
            View::Refs(view) => view.refresh(),
            View::Help(_) => {}
        }
    }
}

#[derive(Debug, Clone)]
pub enum RefreshViewContext {
    List {
        list_context: ListRefreshViewContext,
    },
    Detail {
        list_context: ListRefreshViewContext,
    },
    UserCommand {
        list_context: ListRefreshViewContext,
        user_command_context: UserCommandRefreshViewContext,
    },
    Refs {
        list_context: ListRefreshViewContext,
        refs_context: RefsRefreshViewContext,
    },
}

impl RefreshViewContext {
    pub fn list_context(&self) -> &ListRefreshViewContext {
        match self {
            RefreshViewContext::List { list_context }
            | RefreshViewContext::Detail { list_context }
            | RefreshViewContext::UserCommand { list_context, .. }
            | RefreshViewContext::Refs { list_context, .. } => list_context,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ListRefreshViewContext {
    pub commit_hash: String,
    pub selected: usize,
    pub offset: usize,
    pub height: usize,
    pub scroll_to_top: bool,
    pub search_options: SearchOptions,
    pub search_context: Option<SearchRefreshContext>,
}

impl From<&CommitListState<'_>> for ListRefreshViewContext {
    fn from(list_state: &CommitListState) -> Self {
        let commit_hash = list_state.selected_commit_hash().as_str().into();
        let (selected, offset, height) = list_state.current_list_status();
        // If the selected commit is the top one and there is no offset, it means the list is already scrolled to the top.
        // In this case, we set scroll_to_top to true to indicate that the view should be scrolled to the top after refresh.
        let scroll_to_top = selected == 0 && offset == 0;
        let search_options = list_state.search_options();
        let search_context = list_state.search_refresh_context();
        ListRefreshViewContext {
            commit_hash,
            selected,
            offset,
            height,
            scroll_to_top,
            search_options,
            search_context,
        }
    }
}

impl ListRefreshViewContext {
    /// Restore the commit list state recorded before a refresh.
    pub fn restore(&self, list_state: &mut CommitListState) {
        list_state.restore_search_options(self.search_options);
        list_state.reset_height(self.height);
        if self.scroll_to_top {
            list_state.select_first();
        } else {
            // Keep the same commit selected when it is still in the list after the refresh,
            // and fall back to the recorded absolute index when it is gone.
            let commit_hash = CommitHash::from(self.commit_hash.as_str());
            let index = list_state
                .commit_index_of_hash(&commit_hash)
                .unwrap_or(self.selected);
            list_state.restore_selected_index(index, self.offset);
        }
        if let Some(search_context) = &self.search_context {
            list_state.restore_search(search_context);
        }
    }
}

#[derive(Debug, Clone)]
pub struct UserCommandRefreshViewContext {
    pub n: usize,
}

#[derive(Debug, Clone)]
pub struct RefsRefreshViewContext {
    pub selected: Vec<String>,
    pub opened: Vec<Vec<String>>,
}
