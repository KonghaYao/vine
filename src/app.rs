use std::{
    collections::VecDeque,
    io::{self, Write},
    rc::Rc,
    thread,
};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph},
    DefaultTerminal, Frame,
};
use rustc_hash::FxHashMap;

use crate::{
    color::{ColorTheme, GraphColorSet},
    config::{ActionsConfig, CoreConfig, CursorType, UiConfig, UserCommand, UserCommandType},
    event::{AppEvent, EventController, UserEvent, UserEventWithCount},
    external::{
        copy_to_clipboard, exec_user_command, exec_user_command_suspend, ExternalCommandParameters,
    },
    git::{self, BranchTracking, Commit, CommitHash, FileChange, Head, Ref, Repository},
    git_action::{
        self, action_feedback, commit_menu_entries, ref_menu_entries, switch_actions_for_ref,
        write_action_error, ActionFeedback, ActionMenuContext, ActionMenuEntry, GitAction,
        MenuAction,
    },
    graph::GraphImageManager,
    keybind::KeyBind,
    protocol::ImageProtocol,
    search::SearchOptions,
    view::{RefreshViewContext, View},
    watch::AutoRefreshBlockers,
    widget::commit_list::{CommitInfo, CommitListState},
};

impl StatusLine {
    /// Whether the status line is in a state where mouse events are ignored.
    ///
    /// Mouse events are ignored while a text input is active (like a search query) and while a
    /// remote action is running in the background, so that they cannot run a second action.
    fn blocks_mouse_events(&self) -> bool {
        matches!(self, StatusLine::Input { .. } | StatusLine::Loading(_))
    }
}

#[derive(Debug, Default)]
enum StatusLine {
    #[default]
    None,
    Input {
        message: String,
        cursor_position: u16,
        metadata: String,
    },
    Transient(String),
    Loading(String),
    SearchResult {
        message: String,
        options: String,
        matched: bool,
    },
    NotificationInfo(String),
    NotificationSuccess(String),
    NotificationWarn(String),
    NotificationError(String),
}

/// The branch that is checked out, as it is shown in the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HeadName {
    /// The name of the branch, which is also the branch that is pushed to a remote.
    Branch(String),
    /// The short hash of the commit that is checked out.
    Detached(String),
}

impl HeadName {
    /// The name of the branch, or `None` when HEAD is detached.
    fn branch(&self) -> Option<&str> {
        match self {
            HeadName::Branch(name) => Some(name),
            HeadName::Detached(_) => None,
        }
    }

    /// The text of the segment of HEAD.
    fn text(&self) -> String {
        match self {
            HeadName::Branch(name) => name.clone(),
            HeadName::Detached(short_hash) => format!("detached {short_hash}"),
        }
    }
}

/// What a click on the push segment of the status bar does.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PushTarget {
    /// The branch is pushed to this remote.
    Remote(String),
    /// The branch does not track a remote and the repository has several, so the remote to push to
    /// is chosen in an action menu.
    ChooseRemote,
}

impl PushTarget {
    /// The name shown by the push segment.
    fn label(&self) -> &str {
        match self {
            PushTarget::Remote(remote) => remote,
            PushTarget::ChooseRemote => "push",
        }
    }
}

/// A segment of the bottom status bar.
///
/// The segments are rendered from left to right, separated by two spaces, and the segments without
/// data are left out. New operations are added as new segments, so that the bar stays a list of
/// independent parts.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StatusBarSegment {
    /// The branch that is checked out, or the short hash of a detached HEAD.
    Branch(String),
    /// The number of commits the branch is ahead of its upstream.
    Ahead(usize),
    /// The number of commits the branch is behind its upstream.
    Behind(usize),
    /// The push of the current branch, named by the remote it is pushed to.
    Push { label: String },
}

impl StatusBarSegment {
    /// The text of the segment.
    fn text(&self) -> String {
        match self {
            StatusBarSegment::Branch(name) => name.clone(),
            StatusBarSegment::Ahead(n) => format!("↑{n}"),
            StatusBarSegment::Behind(n) => format!("↓{n}"),
            StatusBarSegment::Push { label } => format!("⇡ {label}"),
        }
    }
}

/// The state of the bottom status bar, which is loaded when the application is created.
///
/// The state is loaded again with the application, so that the bar follows the changes that are
/// made by the application and those that are made outside of it.
#[derive(Debug, Default)]
struct StatusBar {
    /// The branch that is checked out, or `None` when the repository has no HEAD.
    head: Option<HeadName>,
    /// How the branch that is checked out tracks its upstream.
    tracking: BranchTracking,
    /// The names of the remotes of the repository.
    remotes: Vec<String>,
    /// Whether the git actions are enabled; the push segment is not shown when they are not.
    actions_enabled: bool,
}

impl StatusBar {
    /// Loads the state of the status bar of the repository.
    fn new(repository: &Repository, actions_enabled: bool) -> Self {
        let head = match repository.head() {
            Head::Branch { name } => Some(HeadName::Branch(name.clone())),
            Head::Detached { target } => Some(HeadName::Detached(target.as_short_hash().into())),
            Head::None => None,
        };
        StatusBar {
            head,
            tracking: git::branch_tracking(repository.path()),
            remotes: git::load_remotes(repository.path()),
            actions_enabled,
        }
    }

    /// The segments to render, in the order they are rendered.
    fn segments(&self) -> Vec<StatusBarSegment> {
        let mut segments = Vec::new();
        if let Some(text) = self.head.as_ref().map(HeadName::text) {
            segments.push(StatusBarSegment::Branch(text));
        }
        if self.tracking.ahead > 0 {
            segments.push(StatusBarSegment::Ahead(self.tracking.ahead));
        }
        if self.tracking.behind > 0 {
            segments.push(StatusBarSegment::Behind(self.tracking.behind));
        }
        if let Some(target) = self.push_target() {
            segments.push(StatusBarSegment::Push {
                label: target.label().into(),
            });
        }
        segments
    }

    /// What a click on the push segment does, or `None` when the push segment is not shown.
    fn push_target(&self) -> Option<PushTarget> {
        if !self.actions_enabled {
            return None;
        }
        // a detached HEAD is not pushed to the remote of a branch
        self.branch()?;
        push_target(&self.tracking, &self.remotes)
    }

    /// The branch that is pushed to a remote when the push segment is clicked.
    fn branch(&self) -> Option<&str> {
        self.head.as_ref()?.branch()
    }
}

/// The remote the status bar pushes the current branch to, or `None` when there is nothing to push
/// to.
///
/// The remote of the upstream is used when the branch tracks one. A branch that does not track one
/// is pushed to the only remote of the repository, and the remote is chosen in an action menu when
/// there are several.
fn push_target(tracking: &BranchTracking, remotes: &[String]) -> Option<PushTarget> {
    if let Some(remote) = &tracking.remote {
        return Some(PushTarget::Remote(remote.clone()));
    }
    match remotes {
        [] => None,
        [only] => Some(PushTarget::Remote(only.clone())),
        _ => Some(PushTarget::ChooseRemote),
    }
}

/// The space on the left of the status bar.
const STATUS_BAR_PADDING: u16 = 1;
/// The number of spaces between two segments of the status bar.
const STATUS_BAR_SEPARATOR: u16 = 2;

/// The layout of the status bar: every segment with the area it is rendered in.
///
/// The line is truncated at the last segment that fits in the area: the segments that follow are
/// dropped, and a segment is never split, so that the area of a segment always matches its text.
fn status_bar_layout(segments: &[StatusBarSegment], area: Rect) -> Vec<(StatusBarSegment, Rect)> {
    let mut layout: Vec<(StatusBarSegment, Rect)> = Vec::new();
    let mut x = area.x.saturating_add(STATUS_BAR_PADDING);
    for segment in segments {
        if !layout.is_empty() {
            x = x.saturating_add(STATUS_BAR_SEPARATOR);
        }
        let Ok(width) = u16::try_from(console::measure_text_width(&segment.text())) else {
            break;
        };
        if width == 0 || x.saturating_add(width) > area.right() {
            break;
        }
        layout.push((segment.clone(), Rect::new(x, area.y, width, 1)));
        x = x.saturating_add(width);
    }
    layout
}

#[derive(Clone, Copy)]
pub enum InitialSelection {
    Latest,
    Head,
}

pub enum Ret {
    Quit,
    Refresh(RefreshRequest),
}

pub struct RefreshRequest {
    pub context: RefreshViewContext,
}

#[derive(Debug)]
pub struct AppContext {
    pub keybind: KeyBind,
    pub core_config: CoreConfig,
    pub ui_config: UiConfig,
    pub actions_config: ActionsConfig,
    pub color_theme: ColorTheme,
    pub image_protocol: ImageProtocol,
}

#[derive(Debug, Default)]
struct AppStatus {
    status_line: StatusLine,
    numeric_prefix: String,
    view_area: Rect,
    /// The area of the push segment of the status bar, recorded while rendering.
    status_bar_push_area: Option<Rect>,
}

impl AppStatus {
    /// Whether the position is on the push segment of the status bar of the last rendered frame.
    fn is_on_status_bar_push(&self, position: Position) -> bool {
        self.status_bar_push_area
            .is_some_and(|area| area.contains(position))
    }
}

#[derive(Debug)]
pub struct App<'a> {
    repository: &'a Repository,
    view: View<'a>,
    status_bar: StatusBar,
    app_status: AppStatus,
    /// Whether a remote action is running in the background.
    remote_action_in_progress: bool,
    /// The actions that are queued to run after the current one succeeded.
    pending_actions: VecDeque<GitAction>,
    /// Whether a change of the repository is waiting to be refreshed.
    ///
    /// The refresh is postponed while the user is in the middle of something, to avoid
    /// interrupting the input and the dialogs, and to keep what is shown on the screen.
    pending_refresh: bool,
    ctx: Rc<AppContext>,
    ec: &'a EventController,
}

impl<'a> App<'a> {
    pub fn new(
        repository: &'a Repository,
        graph_image_manager: GraphImageManager<'a>,
        graph_color_set: &'a GraphColorSet,
        initial_selection: InitialSelection,
        ctx: Rc<AppContext>,
        ec: &'a EventController,
        refresh_view_context: Option<RefreshViewContext>,
    ) -> Self {
        let graph = graph_image_manager.graph();
        let mut ref_name_to_commit_index_map = FxHashMap::default();
        let commits = graph
            .commits
            .iter()
            .enumerate()
            .map(|(i, commit)| {
                let refs = repository.refs(&commit.commit_hash);
                for r in &refs {
                    ref_name_to_commit_index_map.insert(r.name(), i);
                }
                let (pos_x, _) = graph.commit_pos_map[&commit.commit_hash];
                let graph_color = graph_color_set.get(pos_x).to_ratatui_color();
                CommitInfo::new(commit, refs, graph_color)
            })
            .collect();
        let graph_cell_width = graph_image_manager.graph_cell_width();
        let head = repository.head();
        let mut commit_list_state = CommitListState::new(
            commits,
            graph_image_manager,
            graph_cell_width,
            head,
            ref_name_to_commit_index_map,
            SearchOptions {
                target: ctx.core_config.search.target,
                ignore_case: ctx.core_config.search.ignore_case,
                fuzzy: ctx.core_config.search.fuzzy,
            },
            ctx.ui_config.list.scrolloff as usize,
        );
        if let InitialSelection::Head = initial_selection {
            match repository.head() {
                Head::Branch { name } => commit_list_state.select_ref(name),
                Head::Detached { target } => commit_list_state.select_commit_hash(target),
                Head::None => {}
            }
        }
        let view = View::of_list(commit_list_state, ctx.clone(), ec.sender());

        let status_bar = StatusBar::new(repository, ctx.actions_config.enable);

        let mut app = Self {
            repository,
            view,
            status_bar,
            app_status: AppStatus::default(),
            remote_action_in_progress: false,
            pending_actions: VecDeque::new(),
            pending_refresh: false,
            ctx,
            ec,
        };

        if let Some(warning) = &graph.warning {
            app.warn_notification(warning.clone());
        }

        if let Some(context) = refresh_view_context {
            app.init_with_context(context);
        }

        app
    }
}

impl App<'_> {
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<Ret, std::io::Error> {
        // Clearing the screen here, as it should be cleared upon refresh
        self.clear_image(None)?;
        terminal.clear()?;

        loop {
            if self.pending_refresh && !self.is_auto_refresh_blocked() {
                // The application is idle again, so the postponed refresh is performed now.
                self.pending_refresh = false;
                self.view.refresh();
            }

            self.prepare_render(terminal)?;
            self.flush_pending_graph_uploads()?;
            terminal.draw(|f| self.render(f))?;
            match self.ec.recv() {
                AppEvent::Key(key) => {
                    match self.app_status.status_line {
                        StatusLine::None | StatusLine::Input { .. } | StatusLine::Loading(_) => {
                            // do nothing
                        }
                        StatusLine::Transient(_)
                        | StatusLine::SearchResult { .. }
                        | StatusLine::NotificationInfo(_)
                        | StatusLine::NotificationSuccess(_)
                        | StatusLine::NotificationWarn(_) => {
                            // Clear message and pass key input as is
                            self.clear_status_line();
                        }
                        StatusLine::NotificationError(_) => {
                            // Clear message and cancel key input
                            self.clear_status_line();
                            continue;
                        }
                    }

                    let user_event = self.ctx.keybind.get(&key);

                    if let Some(UserEvent::Cancel) = user_event {
                        if !self.app_status.numeric_prefix.is_empty() {
                            // Clear numeric prefix and cancel the event
                            self.app_status.numeric_prefix.clear();
                            continue;
                        }
                    }

                    match user_event {
                        Some(UserEvent::ForceQuit) => {
                            self.ec.send(AppEvent::Quit);
                        }
                        Some(ue) => {
                            let event_with_count =
                                process_numeric_prefix(&self.app_status.numeric_prefix, *ue, key);
                            self.view.handle_event(event_with_count, key);
                            self.app_status.numeric_prefix.clear();
                        }
                        None => {
                            if matches!(self.app_status.status_line, StatusLine::Input { .. })
                                || self.view.wants_raw_key_events()
                            {
                                // In input mode, pass all key events to the view
                                // fixme: currently, the only thing that processes key_event is searching the list,
                                //        so this probably works, but it's not the right process...
                                self.app_status.numeric_prefix.clear();
                                self.view.handle_event(
                                    UserEventWithCount::from_event(UserEvent::Unknown),
                                    key,
                                );
                            } else if let KeyCode::Char(c) = key.code {
                                // Accumulate numeric prefix
                                if c.is_ascii_digit()
                                    && (c != '0' || !self.app_status.numeric_prefix.is_empty())
                                {
                                    self.app_status.numeric_prefix.push(c);
                                }
                            }
                        }
                    }
                }
                AppEvent::Mouse(mouse) => {
                    // Ignore mouse events when the mouse is disabled by the configuration,
                    // while the search query is being entered, or while a remote action is
                    // running in the background (keys are ignored in the same states)
                    if !self.ctx.ui_config.common.mouse
                        || self.app_status.status_line.blocks_mouse_events()
                        || self.view.ignores_mouse_event()
                    {
                        continue;
                    }
                    // A click on the push segment of the status bar pushes the current branch,
                    // and is not passed to the view
                    if self.is_status_bar_push_click(&mouse) {
                        self.push_current_branch();
                        continue;
                    }
                    // Only the commit list uses the movement of the mouse, for the hover of its
                    // rows
                    if matches!(mouse.kind, MouseEventKind::Moved)
                        && !matches!(self.view, View::List(_))
                    {
                        continue;
                    }
                    self.view.handle_mouse_event(mouse);
                }
                AppEvent::Resize(w, h) => {
                    let _ = (w, h);
                }
                AppEvent::Quit => {
                    self.cleanup_graph_images()?;
                    return Ok(Ret::Quit);
                }
                AppEvent::OpenDetail => {
                    self.clear_image(Some(terminal))?;
                    self.open_detail();
                }
                AppEvent::CloseDetail => {
                    terminal.clear()?;
                    self.close_detail();
                }
                AppEvent::OpenUserCommand(n) => {
                    self.clear_image(Some(terminal))?;
                    self.open_user_command(n, Some(terminal));
                }
                AppEvent::CloseUserCommand => {
                    terminal.clear()?;
                    self.close_user_command();
                }
                AppEvent::OpenRefs => {
                    self.open_refs();
                }
                AppEvent::CloseRefs => {
                    self.close_refs();
                }
                AppEvent::OpenActions => {
                    self.open_actions();
                }
                AppEvent::CloseActions => {
                    self.close_actions();
                }
                AppEvent::MoreButtonHoverElapsed(token) => {
                    // The dwell is turned into the menu only while the commit list is shown and
                    // the arming is still the current one; the arming is expired otherwise
                    let armed = match &mut self.view {
                        View::List(view) => view.take_armed_more_button(token),
                        _ => false,
                    };
                    if armed {
                        self.open_actions();
                    }
                }
                AppEvent::ExecuteGitAction(action) => {
                    self.execute_git_action(*action);
                }
                AppEvent::SwitchToRef(r) => {
                    self.switch_to_ref(*r);
                }
                AppEvent::RemoteActionFinished { action, result } => {
                    self.remote_action_in_progress = false;
                    self.clear_status_line();
                    self.handle_action_result(&action, result);
                }
                AppEvent::OpenActionOutput(output) => {
                    self.clear_image(Some(terminal))?;
                    self.open_action_output(output);
                }
                AppEvent::OpenHelp => {
                    self.clear_image(None)?;
                    self.open_help();
                }
                AppEvent::CloseHelp => {
                    terminal.clear()?;
                    self.close_help();
                }
                AppEvent::SelectOlderCommit => {
                    self.select_older_commit();
                }
                AppEvent::SelectNewerCommit => {
                    self.select_newer_commit();
                }
                AppEvent::SelectParentCommit => {
                    self.select_parent_commit();
                }
                AppEvent::CopyToClipboard { name, value } => {
                    self.copy_to_clipboard(name, value);
                }
                AppEvent::Refresh(context) => {
                    self.cleanup_graph_images()?;
                    let request = RefreshRequest { context };
                    return Ok(Ret::Refresh(request));
                }
                AppEvent::RepoChanged => {
                    if self.is_auto_refresh_blocked() {
                        self.pending_refresh = true;
                    } else {
                        // The same refresh as the one of the refresh key, restoring the state of
                        // the current view.
                        self.view.refresh();
                    }
                }
                AppEvent::ClearStatusLine => {
                    self.clear_status_line();
                }
                AppEvent::UpdateStatusInput {
                    message,
                    cursor_position,
                    metadata,
                } => {
                    self.update_status_input(message, cursor_position, metadata);
                }
                AppEvent::UpdateStatusTransient(msg) => {
                    self.update_status_transient(msg);
                }
                AppEvent::UpdateSearchResult {
                    message,
                    options,
                    matched,
                } => {
                    self.update_search_result(message, options, matched);
                }
                AppEvent::NotifyInfo(msg) => {
                    self.info_notification(msg);
                }
                AppEvent::NotifySuccess(msg) => {
                    self.success_notification(msg);
                }
                AppEvent::NotifyWarn(msg) => {
                    self.warn_notification(msg);
                }
                AppEvent::NotifyError(msg) => {
                    self.error_notification(msg);
                }
            }
        }
    }

    /// Whether a change of the repository must be postponed before it is refreshed.
    ///
    /// Refreshing rebuilds the whole application, so it is postponed while the user is entering
    /// input or a dialog is open, and while a refresh would repeat or break what is running.
    fn is_auto_refresh_blocked(&self) -> bool {
        AutoRefreshBlockers {
            input_in_progress: matches!(self.app_status.status_line, StatusLine::Input { .. })
                || self.view.is_searching(),
            overlay_open: matches!(self.view, View::ActionMenu(_) | View::Help(_)),
            // Refreshing the user command view runs its command again.
            user_command_open: matches!(self.view, View::UserCommand(_)),
            // The queued actions of the running action would be lost.
            remote_action_in_progress: self.remote_action_in_progress,
        }
        .blocks()
    }

    fn prepare_render(&mut self, terminal: &mut DefaultTerminal) -> Result<(), std::io::Error> {
        let area: Rect = terminal.size()?.into();
        let [view_area, _, _] = split_app_areas(area);
        self.update_state(view_area);
        self.view.update_layout(view_area);
        self.view.prepare_graph_uploads();
        Ok(())
    }

    fn flush_pending_graph_uploads(&mut self) -> Result<(), std::io::Error> {
        let uploads = self.view.drain_pending_graph_uploads();
        if uploads.is_empty() {
            return Ok(());
        }

        let mut stdout = io::stdout().lock();
        for upload in uploads {
            stdout.write_all(upload.as_bytes())?;
        }
        stdout.flush()
    }

    fn cleanup_graph_images(&self) -> Result<(), std::io::Error> {
        let image_ids = self.view.graph_image_ids_sorted();
        self.ctx.image_protocol.delete_images(&image_ids)
    }

    fn render(&mut self, f: &mut Frame) {
        let base = Block::default()
            .fg(self.ctx.color_theme.fg)
            .bg(self.ctx.color_theme.bg);
        f.render_widget(base, f.area());

        let [view_area, status_line_area, status_bar_area] = split_app_areas(f.area());

        self.update_state(view_area);

        self.view.render(f, view_area);
        self.render_status_line(f, status_line_area);
        self.render_status_bar(f, status_bar_area);
    }

    /// Renders the status bar, which is the last line of the screen.
    ///
    /// The area of the push segment is recorded while rendering, so that a click can be matched to
    /// the segment under the cursor.
    fn render_status_bar(&mut self, f: &mut Frame, area: Rect) {
        let base_style = Style::default()
            .fg(self.ctx.color_theme.status_bar_fg)
            .bg(self.ctx.color_theme.status_bar_bg);
        f.buffer_mut().set_style(area, base_style);

        self.app_status.status_bar_push_area = None;
        let layout = status_bar_layout(&self.status_bar.segments(), area);
        for (segment, rect) in layout {
            let mut style = base_style;
            if matches!(segment, StatusBarSegment::Push { .. }) {
                self.app_status.status_bar_push_area = Some(rect);
                if self.ctx.ui_config.common.mouse {
                    // the push segment is clickable, which is hinted by its bold style
                    style = style.add_modifier(Modifier::BOLD);
                }
            }
            f.buffer_mut()
                .set_string(rect.x, rect.y, segment.text(), style);
        }
    }
}

impl App<'_> {
    fn render_status_line(&self, f: &mut Frame, area: Rect) {
        let text: Line = match &self.app_status.status_line {
            StatusLine::None => {
                if self.app_status.numeric_prefix.is_empty() {
                    Line::raw("")
                } else {
                    Line::raw(self.app_status.numeric_prefix.as_str())
                        .fg(self.ctx.color_theme.status_input_transient_fg)
                }
            }
            StatusLine::Input {
                message, metadata, ..
            } => status_line_with_metadata(
                message,
                metadata,
                Style::default().fg(self.ctx.color_theme.status_input_fg),
                Style::default().fg(self.ctx.color_theme.status_input_transient_fg),
                area.width,
            ),
            StatusLine::Loading(msg) => Line::raw(msg)
                .add_modifier(Modifier::BOLD)
                .fg(self.ctx.color_theme.status_info_fg),
            StatusLine::Transient(msg) => {
                let msg_w = console::measure_text_width(msg.as_str());
                let pad_w = (area.width as usize).saturating_sub(msg_w + 2 /* pad */);
                Line::from(vec![
                    " ".repeat(pad_w).into(),
                    msg.as_str()
                        .fg(self.ctx.color_theme.status_input_transient_fg),
                ])
            }
            StatusLine::SearchResult {
                message,
                options,
                matched,
            } => {
                let message_style = if *matched {
                    Style::default().fg(self.ctx.color_theme.status_info_fg)
                } else {
                    Style::default()
                        .fg(self.ctx.color_theme.status_warn_fg)
                        .add_modifier(Modifier::BOLD)
                };
                status_line_with_metadata(
                    message,
                    options,
                    message_style,
                    Style::default().fg(self.ctx.color_theme.status_input_transient_fg),
                    area.width,
                )
            }
            StatusLine::NotificationInfo(msg) => {
                Line::raw(msg).fg(self.ctx.color_theme.status_info_fg)
            }
            StatusLine::NotificationSuccess(msg) => Line::raw(msg)
                .add_modifier(Modifier::BOLD)
                .fg(self.ctx.color_theme.status_success_fg),
            StatusLine::NotificationWarn(msg) => Line::raw(msg)
                .add_modifier(Modifier::BOLD)
                .fg(self.ctx.color_theme.status_warn_fg),
            StatusLine::NotificationError(msg) => Line::raw(format!("ERROR: {msg}"))
                .add_modifier(Modifier::BOLD)
                .fg(self.ctx.color_theme.status_error_fg),
        };
        let paragraph = Paragraph::new(text).block(
            Block::default()
                .borders(Borders::TOP)
                .style(Style::default().fg(self.ctx.color_theme.divider_fg))
                .padding(Padding::horizontal(1)),
        );
        f.render_widget(paragraph, area);

        if let StatusLine::Input {
            cursor_position, ..
        } = &self.app_status.status_line
        {
            let (x, y) = (area.x + cursor_position + 1, area.y + 1);
            match &self.ctx.ui_config.common.cursor_type {
                CursorType::Native => {
                    f.set_cursor_position((x, y));
                }
                CursorType::Virtual(cursor) => {
                    let style = Style::default().fg(self.ctx.color_theme.virtual_cursor_fg);
                    f.buffer_mut().set_string(x, y, cursor, style);
                }
            }
        }
    }
}

/// The area of the view, of the message line, and of the status bar, from top to bottom.
///
/// The status bar is the last line of the screen, under the divider and the message line.
fn split_app_areas(area: Rect) -> [Rect; 3] {
    Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(area)
}

impl App<'_> {
    fn update_state(&mut self, view_area: Rect) {
        self.app_status.view_area = view_area;
    }

    fn clear_image(&self, terminal: Option<&mut DefaultTerminal>) -> Result<(), std::io::Error> {
        // Sometimes the first image fails to render after a full screen clear
        // As a workaround, the first area is preserved when a full clear is not required
        if let Some(t) = terminal {
            for y in 1..t.size()?.height {
                self.ctx.image_protocol.clear_line(y);
            }
        } else {
            self.ctx.image_protocol.clear();
        }
        Ok(())
    }

    fn open_detail(&mut self) {
        let commit_list_state = match self.view {
            View::List(ref mut view) => view.take_list_state(),
            View::UserCommand(ref mut view) => view.take_list_state(),
            _ => return,
        };
        let (commit, changes, refs) = selected_commit_details(self.repository, &commit_list_state);
        self.view = View::of_detail(
            commit_list_state,
            commit,
            changes,
            refs,
            self.ctx.clone(),
            self.ec.sender(),
        );
    }

    fn close_detail(&mut self) {
        if let View::Detail(ref mut view) = self.view {
            let commit_list_state = view.take_list_state();
            self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
        }
    }

    fn open_user_command(
        &mut self,
        user_command_number: usize,
        terminal: Option<&mut DefaultTerminal>,
    ) {
        let clear = match extract_user_command_by_number(user_command_number, &self.ctx)
            .map(|c| &c.r#type)
        {
            Ok(UserCommandType::Inline) => {
                self.open_user_command_inline(user_command_number);
                false
            }
            Ok(UserCommandType::Silent) => {
                self.open_user_command_silent(user_command_number);
                true
            }
            Ok(UserCommandType::Suspend) => {
                self.open_user_command_suspend(user_command_number);
                true
            }
            Err(err) => {
                self.ec.send(AppEvent::NotifyError(err));
                false
            }
        };
        if clear {
            if let Some(t) = terminal {
                if let Err(err) = t.clear() {
                    let msg = format!("Failed to clear terminal: {err:?}");
                    self.ec.send(AppEvent::NotifyError(msg));
                }
            }
        }
    }

    fn open_user_command_inline(&mut self, user_command_number: usize) {
        let commit_list_state = match self.view {
            View::List(ref mut view) => view.as_list_state(),
            View::Detail(ref mut view) => view.as_list_state(),
            View::UserCommand(ref mut view) => view.as_list_state(),
            _ => return,
        };
        let (commit, _, refs) = selected_commit_details(self.repository, commit_list_state);
        let result = build_external_command_parameters_and_exec_command(
            &commit,
            &refs,
            user_command_number,
            self.app_status.view_area,
            &self.ctx,
        );
        match result {
            Ok(output) => {
                // take list state only when the command execution is successful, to avoid losing the state when the command fails
                let commit_list_state = match self.view {
                    View::List(ref mut view) => view.take_list_state(),
                    View::Detail(ref mut view) => view.take_list_state(),
                    View::UserCommand(ref mut view) => view.take_list_state(),
                    _ => return,
                };
                self.view = View::of_user_command(
                    commit_list_state,
                    output,
                    Some(user_command_number),
                    self.ctx.clone(),
                    self.ec.sender(),
                );
            }
            Err(err) => {
                self.ec.send(AppEvent::NotifyError(err));
            }
        };
    }

    fn open_user_command_silent(&mut self, user_command_number: usize) {
        let commit_list_state = match self.view {
            View::List(ref mut view) => view.as_list_state(),
            View::Detail(ref mut view) => view.as_list_state(),
            View::UserCommand(ref mut view) => view.as_list_state(),
            _ => return,
        };
        let (commit, _, refs) = selected_commit_details(self.repository, commit_list_state);
        let result = build_external_command_parameters_and_exec_command(
            &commit,
            &refs,
            user_command_number,
            self.app_status.view_area,
            &self.ctx,
        );
        match result {
            Ok(_) => {
                if extract_user_command_refresh_by_number(user_command_number, &self.ctx) {
                    self.view.refresh();
                }
            }
            Err(err) => {
                self.ec.send(AppEvent::NotifyError(err));
            }
        }
    }

    fn open_user_command_suspend(&mut self, user_command_number: usize) {
        let commit_list_state = match self.view {
            View::List(ref mut view) => view.as_list_state(),
            View::Detail(ref mut view) => view.as_list_state(),
            View::UserCommand(ref mut view) => view.as_list_state(),
            _ => return,
        };
        let (commit, _, refs) = selected_commit_details(self.repository, commit_list_state);
        match build_external_command_parameters(
            &commit,
            &refs,
            user_command_number,
            self.app_status.view_area,
            &self.ctx,
        ) {
            Ok(params) => {
                self.ec.suspend();
                let exec_result = exec_user_command_suspend(params);
                self.ec.resume();

                if extract_user_command_refresh_by_number(user_command_number, &self.ctx) {
                    self.view.refresh();
                }

                // notify after resuming and refreshing
                if let Err(err) = exec_result {
                    self.ec.send(AppEvent::NotifyError(err));
                }
            }
            Err(err) => {
                self.ec.send(AppEvent::NotifyError(err));
            }
        }
    }

    fn close_user_command(&mut self) {
        if let View::UserCommand(ref mut view) = self.view {
            let commit_list_state = view.take_list_state();
            self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
        }
    }

    fn open_refs(&mut self) {
        if let View::List(ref mut view) = self.view {
            let commit_list_state = view.take_list_state();
            let refs = self.repository.all_refs();
            self.view = View::of_refs(commit_list_state, &refs, self.ctx.clone(), self.ec.sender());
        }
    }

    fn close_refs(&mut self) {
        if let View::Refs(ref mut view) = self.view {
            let commit_list_state = view.take_list_state();
            self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
        }
    }

    fn open_help(&mut self) {
        let before_view = std::mem::take(&mut self.view);
        self.view = View::of_help(before_view, self.ctx.clone(), self.ec.sender());
    }

    fn close_help(&mut self) {
        if let View::Help(ref mut view) = self.view {
            self.view = view.take_before_view();
        }
    }

    fn open_actions(&mut self) {
        if !self.ctx.actions_config.enable || matches!(self.view, View::ActionMenu(_)) {
            return;
        }
        let Some((name, entries)) = self.action_menu_contents() else {
            return;
        };
        if entries.is_empty() {
            self.ec.send(AppEvent::NotifyInfo(format!(
                "No actions available for {name}"
            )));
            return;
        }
        let refs = self.repository.all_refs().into_iter().cloned().collect();
        // The more button of the list is not shown while the menu is open, and it is not shown
        // again until the mouse moves, so that the menu cannot be reopened by a stale hover
        if let View::List(view) = &mut self.view {
            view.clear_hover();
        }
        let before = std::mem::take(&mut self.view);
        self.view = View::of_action_menu(
            before,
            action_menu_title(&name),
            entries,
            refs,
            self.ctx.clone(),
            self.ec.sender(),
        );
    }

    /// The name of the target of the action menu for the current view, and the menu entries.
    fn action_menu_contents(&self) -> Option<(String, Vec<ActionMenuEntry>)> {
        let remotes = git::load_remotes(self.repository.path());
        let current_branch = match self.repository.head() {
            Head::Branch { name } => Some(name.as_str()),
            Head::Detached { .. } | Head::None => None,
        };
        let menu_context = ActionMenuContext {
            writable: write_action_error(self.ctx.actions_config.enable, self.repository.is_bare())
                .is_none(),
            remotes: &remotes,
            current_branch,
        };
        match &self.view {
            View::List(view) => {
                self.commit_action_menu_contents(view.as_list_state(), &menu_context)
            }
            View::Detail(view) => {
                self.commit_action_menu_contents(view.as_list_state(), &menu_context)
            }
            View::UserCommand(view) => {
                self.commit_action_menu_contents(view.as_list_state(), &menu_context)
            }
            View::Refs(view) => {
                let name = view.selected_ref_name()?;
                let r = self
                    .repository
                    .all_refs()
                    .into_iter()
                    .find(|r| r.name() == name)?
                    .clone();
                let entries = ref_menu_entries(&r, &menu_context);
                Some((r.name().into(), entries))
            }
            _ => None,
        }
    }

    fn commit_action_menu_contents(
        &self,
        list_state: &CommitListState,
        menu_context: &ActionMenuContext,
    ) -> Option<(String, Vec<ActionMenuEntry>)> {
        let commit_hash = list_state.selected_commit_hash().clone();
        let refs: Vec<Ref> = self
            .repository
            .refs(&commit_hash)
            .into_iter()
            .cloned()
            .collect();
        let entries = commit_menu_entries(
            &commit_hash,
            &refs,
            &self.ctx.core_config.user_command.commands,
            menu_context,
        );
        Some((commit_action_menu_name(&commit_hash), entries))
    }

    fn close_actions(&mut self) {
        if let View::ActionMenu(ref mut view) = self.view {
            self.view = view.take_before_view();
        }
    }

    /// Whether the mouse event is a left click on the push segment of the status bar.
    fn is_status_bar_push_click(&self, mouse: &MouseEvent) -> bool {
        matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self
                .app_status
                .is_on_status_bar_push(Position::new(mouse.column, mouse.row))
    }

    /// Pushes the branch that is checked out, like a click on the push segment of the status bar.
    ///
    /// The push runs in the background like any other remote action. The remote to push to is
    /// chosen in an action menu when the branch does not track one and the repository has several.
    fn push_current_branch(&mut self) {
        let Some(branch) = self.status_bar.branch().map(String::from) else {
            return;
        };
        match self.status_bar.push_target() {
            Some(PushTarget::Remote(remote)) => {
                self.ec
                    .send(AppEvent::ExecuteGitAction(Box::new(GitAction::Push {
                        remote,
                        branch,
                    })));
            }
            Some(PushTarget::ChooseRemote) => self.open_push_menu(&branch),
            None => {}
        }
    }

    /// Opens the menu that chooses the remote to push the given branch to.
    fn open_push_menu(&mut self, branch: &str) {
        if matches!(self.view, View::ActionMenu(_)) {
            return;
        }
        let entries = self
            .status_bar
            .remotes
            .iter()
            .map(|remote| ActionMenuEntry::Action {
                label: format!("Push to '{remote}'"),
                action: MenuAction::Action(GitAction::Push {
                    remote: remote.clone(),
                    branch: branch.into(),
                }),
            })
            .collect();
        let refs = self.repository.all_refs().into_iter().cloned().collect();
        let before = std::mem::take(&mut self.view);
        self.view = View::of_action_menu(
            before,
            action_menu_title(branch),
            entries,
            refs,
            self.ctx.clone(),
            self.ec.sender(),
        );
    }

    /// Switches the repository to the ref that was double clicked in the commit list.
    ///
    /// Switching is not destructive, so it is executed without confirmation.
    fn switch_to_ref(&mut self, r: Ref) {
        // Write actions are not available when they are disabled, or in a bare repository
        if let Some(reason) =
            write_action_error(self.ctx.actions_config.enable, self.repository.is_bare())
        {
            self.ec.send(AppEvent::NotifyError(reason.into()));
            return;
        }
        let path = self.repository.path().to_path_buf();
        match switch_actions_for_ref(&r, |name| git::local_branch_exists(&path, name)) {
            Ok(actions) => self.execute_git_actions(actions),
            Err(message) => self.ec.send(AppEvent::NotifyError(message)),
        }
    }

    /// Runs the actions in order, starting each one after the previous one succeeded.
    fn execute_git_actions(&mut self, actions: Vec<GitAction>) {
        let mut actions = actions.into_iter();
        let Some(first) = actions.next() else {
            return;
        };
        self.pending_actions = actions.collect();
        self.execute_git_action(first);
    }

    fn execute_git_action(&mut self, action: GitAction) {
        if action.is_remote() {
            self.execute_remote_action(action);
            return;
        }
        let result = git_action::execute_git_action(&action, self.repository.path());
        self.handle_action_result(&action, result);
    }

    /// Runs a remote action in a background thread, so that the application keeps responding.
    ///
    /// The result is sent back to the application as an [`AppEvent::RemoteActionFinished`] event.
    fn execute_remote_action(&mut self, action: GitAction) {
        if self.remote_action_in_progress {
            self.ec.send(AppEvent::NotifyInfo(
                "A remote operation is already running".into(),
            ));
            return;
        }
        self.remote_action_in_progress = true;
        self.app_status.status_line = StatusLine::Loading(action.progress_message());

        let tx = self.ec.sender();
        let path = self.repository.path().to_path_buf();
        thread::spawn(move || {
            let result = git_action::execute_git_action(&action, &path);
            tx.send_if_running(AppEvent::RemoteActionFinished {
                action: Box::new(action),
                result,
            });
        });
    }

    fn handle_action_result(&mut self, action: &GitAction, result: Result<String, String>) {
        match action_feedback(action, result) {
            ActionFeedback::Success { summary, output } => {
                // The repository is reloaded after the action, so the events are sent in the
                // order they should be handled by the reloaded application.
                self.view.refresh();
                if let Some(output) = output {
                    self.ec.send(AppEvent::OpenActionOutput(output));
                }
                self.ec.send(AppEvent::NotifySuccess(summary));
                // The next action of a chain is started from the event loop, so that its progress
                // is shown after the summary of the action that just finished.
                if let Some(next) = self.pending_actions.pop_front() {
                    self.ec.send(AppEvent::ExecuteGitAction(Box::new(next)));
                }
            }
            ActionFeedback::Error { message } => {
                // The rest of a chain is dropped, because it depends on the action that failed
                self.pending_actions.clear();
                self.ec.send(AppEvent::NotifyError(message));
            }
        }
    }

    fn open_action_output(&mut self, output: String) {
        let commit_list_state = match self.view {
            View::List(ref mut view) => view.take_list_state(),
            View::Detail(ref mut view) => view.take_list_state(),
            View::UserCommand(ref mut view) => view.take_list_state(),
            View::Refs(ref mut view) => view.take_list_state(),
            _ => return,
        };
        self.view = View::of_action_output(
            commit_list_state,
            output,
            self.ctx.clone(),
            self.ec.sender(),
        );
    }

    fn select_older_commit(&mut self) {
        if let View::Detail(ref mut view) = self.view {
            view.select_older_commit(self.repository);
        } else if let View::UserCommand(ref mut view) = self.view {
            view.select_older_commit(
                self.repository,
                self.app_status.view_area,
                build_external_command_parameters_and_exec_command,
            );
        }
    }

    fn select_newer_commit(&mut self) {
        if let View::Detail(ref mut view) = self.view {
            view.select_newer_commit(self.repository);
        } else if let View::UserCommand(ref mut view) = self.view {
            view.select_newer_commit(
                self.repository,
                self.app_status.view_area,
                build_external_command_parameters_and_exec_command,
            );
        }
    }

    fn select_parent_commit(&mut self) {
        if let View::Detail(ref mut view) = self.view {
            view.select_parent_commit(self.repository);
        } else if let View::UserCommand(ref mut view) = self.view {
            view.select_parent_commit(
                self.repository,
                self.app_status.view_area,
                build_external_command_parameters_and_exec_command,
            );
        }
    }

    fn init_with_context(&mut self, context: RefreshViewContext) {
        if let View::List(ref mut view) = self.view {
            view.reset_commit_list_with(context.list_context());
        }
        match context {
            RefreshViewContext::List { .. } => {}
            RefreshViewContext::Detail { .. } => {
                self.open_detail();
            }
            RefreshViewContext::UserCommand {
                user_command_context,
                ..
            } => {
                self.open_user_command(user_command_context.n, None);
            }
            RefreshViewContext::Refs { refs_context, .. } => {
                self.open_refs();
                if let View::Refs(ref mut view) = self.view {
                    view.reset_refs_with(refs_context);
                }
            }
        }
    }

    fn clear_status_line(&mut self) {
        self.app_status.status_line = StatusLine::None;
    }

    fn update_status_input(&mut self, message: String, cursor_position: u16, metadata: String) {
        self.app_status.status_line = StatusLine::Input {
            message,
            cursor_position,
            metadata,
        };
    }

    fn update_status_transient(&mut self, msg: String) {
        self.app_status.status_line = StatusLine::Transient(msg);
    }

    fn update_search_result(&mut self, message: String, options: String, matched: bool) {
        self.app_status.status_line = StatusLine::SearchResult {
            message,
            options,
            matched,
        };
    }

    fn info_notification(&mut self, msg: String) {
        self.app_status.status_line = StatusLine::NotificationInfo(msg);
    }

    fn success_notification(&mut self, msg: String) {
        self.app_status.status_line = StatusLine::NotificationSuccess(msg);
    }

    fn warn_notification(&mut self, msg: String) {
        self.app_status.status_line = StatusLine::NotificationWarn(msg);
    }

    fn error_notification(&mut self, msg: String) {
        self.app_status.status_line = StatusLine::NotificationError(msg);
    }

    fn copy_to_clipboard(&self, name: String, value: String) {
        match copy_to_clipboard(value, &self.ctx.core_config.external.clipboard) {
            Ok(_) => {
                let msg = format!("Copied {name} to clipboard successfully");
                self.ec.send(AppEvent::NotifySuccess(msg));
            }
            Err(msg) => {
                self.ec.send(AppEvent::NotifyError(msg));
            }
        }
    }
}

fn status_line_with_metadata(
    message: &str,
    metadata: &str,
    message_style: Style,
    metadata_style: Style,
    area_width: u16,
) -> Line<'static> {
    let content_width = area_width.saturating_sub(2) as usize;
    let message_width = console::measure_text_width(message);
    let metadata_width = console::measure_text_width(metadata);
    let min_gap_width = 2;

    if message_width + min_gap_width + metadata_width > content_width {
        return Line::from(Span::styled(message.to_owned(), message_style));
    }

    let pad_width = content_width - message_width - metadata_width;
    Line::from(vec![
        Span::styled(message.to_owned(), message_style),
        Span::raw(" ".repeat(pad_width)),
        Span::styled(metadata.to_owned(), metadata_style),
    ])
}

/// The title of the action menu of a target with the given name.
fn action_menu_title(name: &str) -> String {
    format!("Actions - {name}")
}

/// The name of the target of the action menu of a commit.
///
/// The name is not prefixed with the title of the menu, so that the prefix is added only when the
/// menu is created.
fn commit_action_menu_name(commit_hash: &CommitHash) -> String {
    commit_hash.as_short_hash().to_string()
}

fn selected_commit_details(
    repository: &Repository,
    commit_list_state: &CommitListState,
) -> (Commit, Vec<FileChange>, Vec<Ref>) {
    let selected = commit_list_state.selected_commit_hash().clone();
    let (commit, changes) = repository.commit_detail(&selected);
    let refs: Vec<Ref> = repository.refs(&selected).into_iter().cloned().collect();
    (commit, changes, refs)
}

fn process_numeric_prefix(
    numeric_prefix: &str,
    user_event: UserEvent,
    _key_event: KeyEvent,
) -> UserEventWithCount {
    if user_event.is_countable() {
        let count = if numeric_prefix.is_empty() {
            1
        } else {
            numeric_prefix.parse::<usize>().unwrap_or(1)
        };
        UserEventWithCount::new(user_event, count)
    } else {
        UserEventWithCount::from_event(user_event)
    }
}

fn extract_user_command_by_number(
    user_command_number: usize,
    ctx: &AppContext,
) -> Result<&UserCommand, String> {
    ctx.core_config
        .user_command
        .commands
        .get(&user_command_number.to_string())
        .ok_or_else(|| format!("No user command configured for number {user_command_number}",))
}

fn extract_user_command_refresh_by_number(user_command_number: usize, ctx: &AppContext) -> bool {
    extract_user_command_by_number(user_command_number, ctx)
        .map(|c| c.refresh)
        .unwrap_or_default()
}

fn build_external_command_parameters_and_exec_command(
    commit: &Commit,
    refs: &[Ref],
    user_command_number: usize,
    view_area: Rect,
    ctx: &AppContext,
) -> Result<String, String> {
    build_external_command_parameters(commit, refs, user_command_number, view_area, ctx)
        .and_then(exec_user_command)
}

fn build_external_command_parameters<'a>(
    commit: &'a Commit,
    refs: &'a [Ref],
    user_command_number: usize,
    view_area: Rect,
    ctx: &'a AppContext,
) -> Result<ExternalCommandParameters<'a>, String> {
    let command = &extract_user_command_by_number(user_command_number, ctx)?.commands;
    let target_hash = commit.commit_hash.as_str();
    let parent_hashes = commit
        .parent_commit_hashes
        .iter()
        .map(|c| c.as_str())
        .collect();

    let mut all_refs = vec![];
    let mut branches = vec![];
    let mut remote_branches = vec![];
    let mut tags = vec![];
    let mut stash = None;
    for r in refs {
        match r {
            Ref::Tag { .. } => tags.push(r.name()),
            Ref::Branch { .. } => branches.push(r.name()),
            Ref::RemoteBranch { .. } => remote_branches.push(r.name()),
            Ref::Stash { .. } => {
                stash = Some(r.name());
                continue; // skip stashes from {{refs}}
            }
        }
        all_refs.push(r.name());
    }

    let area_width = view_area.width.saturating_sub(4); // minus the left and right padding
    let area_height = (view_area.height.saturating_sub(1))
        .min(ctx.ui_config.user_command.height)
        .saturating_sub(1); // minus the top border
    Ok(ExternalCommandParameters {
        command,
        target_hash,
        parent_hashes,
        all_refs,
        branches,
        remote_branches,
        tags,
        stash,
        area_width,
        area_height,
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn test_mouse_events_are_ignored_for_input_and_loading() {
        assert!(StatusLine::Input {
            message: String::new(),
            cursor_position: 0,
            metadata: String::new(),
        }
        .blocks_mouse_events());
        assert!(StatusLine::Loading("Pushing 'main' to 'origin'...".into()).blocks_mouse_events());

        for status_line in [
            StatusLine::None,
            StatusLine::Transient("message".into()),
            StatusLine::SearchResult {
                message: String::new(),
                options: String::new(),
                matched: true,
            },
            StatusLine::NotificationInfo("message".into()),
            StatusLine::NotificationSuccess("message".into()),
            StatusLine::NotificationWarn("message".into()),
            StatusLine::NotificationError("message".into()),
        ] {
            assert!(!status_line.blocks_mouse_events(), "{status_line:?}");
        }
    }

    #[test]
    fn test_action_menu_title_has_a_single_prefix() {
        // the name of a commit is its short hash, and the refs are named by their ref name
        let commit_hash = CommitHash::from("abcdef0123456789abcdef0123456789abcdef01");
        assert_eq!(
            action_menu_title(&commit_action_menu_name(&commit_hash)),
            "Actions - abcdef0"
        );
        assert_eq!(action_menu_title("v1.0.0"), "Actions - v1.0.0");

        // the name never contains the prefix of the title
        assert!(!commit_action_menu_name(&commit_hash).contains("Actions"));
    }

    #[rustfmt::skip]
    #[rstest]
    #[case("",    UserEvent::NavigateDown, UserEventWithCount::new(UserEvent::NavigateDown, 1))] // no prefix
    #[case("5",   UserEvent::NavigateUp,   UserEventWithCount::new(UserEvent::NavigateUp, 5))] // with prefix
    #[case("0",   UserEvent::PageDown,     UserEventWithCount::new(UserEvent::PageDown, 1))] // zero should be converted to 1
    #[case("42",  UserEvent::ScrollDown,   UserEventWithCount::new(UserEvent::ScrollDown, 42))] // multi-digit number
    #[case("999", UserEvent::PageDown,     UserEventWithCount::new(UserEvent::PageDown, 999))] // large number
    #[case("abc", UserEvent::ScrollUp,     UserEventWithCount::new(UserEvent::ScrollUp, 1))] // should fallback to 1
    #[case("5",   UserEvent::Quit,         UserEventWithCount::new(UserEvent::Quit, 1))] // non-countable event with prefix
    #[case("",    UserEvent::Confirm,      UserEventWithCount::new(UserEvent::Confirm, 1))] // non-countable event without prefix
    fn test_process_numeric_prefix(
        #[case] numeric_prefix: &str,
        #[case] user_event: UserEvent,
        #[case] expected: UserEventWithCount,
    ) {
        let dummy_key_event = KeyEvent::from(KeyCode::Enter); // KeyEvent is not used in the logic
        let actual = process_numeric_prefix(numeric_prefix, user_event, dummy_key_event);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_status_line_with_metadata_right_aligns_metadata() {
        let line =
            status_line_with_metadata("left", "[meta]", Style::default(), Style::default(), 14);

        assert_eq!(line.spans.len(), 3);
        assert_eq!(line.spans[0].content.as_ref(), "left");
        assert_eq!(line.spans[1].content.as_ref(), "  ");
        assert_eq!(line.spans[2].content.as_ref(), "[meta]");
    }

    #[test]
    fn test_status_line_with_metadata_hides_metadata_when_area_is_too_narrow() {
        let line =
            status_line_with_metadata("left", "[meta]", Style::default(), Style::default(), 13);

        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].content.as_ref(), "left");
    }

    /// The segments of a branch that is ahead of and behind its upstream, and that is pushed to
    /// `origin`.
    fn status_bar_segments() -> Vec<StatusBarSegment> {
        vec![
            StatusBarSegment::Branch("main".into()),
            StatusBarSegment::Ahead(1),
            StatusBarSegment::Behind(1),
            StatusBarSegment::Push {
                label: "origin".into(),
            },
        ]
    }

    /// The text of the segments and the area each of them is rendered in.
    fn status_bar_texts(layout: &[(StatusBarSegment, Rect)]) -> Vec<(String, Rect)> {
        layout
            .iter()
            .map(|(segment, rect)| (segment.text(), *rect))
            .collect()
    }

    #[test]
    fn test_status_bar_layout_renders_the_segments_from_left_to_right() {
        let area = Rect::new(0, 23, 40, 1);

        let layout = status_bar_layout(&status_bar_segments(), area);

        // one space of padding, then the segments separated by two spaces
        assert_eq!(
            status_bar_texts(&layout),
            vec![
                ("main".into(), Rect::new(1, 23, 4, 1)),
                ("↑1".into(), Rect::new(7, 23, 2, 1)),
                ("↓1".into(), Rect::new(11, 23, 2, 1)),
                ("⇡ origin".into(), Rect::new(15, 23, 8, 1)),
            ]
        );
    }

    #[test]
    fn test_status_bar_layout_is_truncated_at_the_last_segment_that_fits() {
        let segments = status_bar_segments();

        // "↓1" would end on the 13th column, which is outside of the area
        let layout = status_bar_layout(&segments, Rect::new(0, 23, 12, 1));
        assert_eq!(
            status_bar_texts(&layout),
            vec![
                ("main".into(), Rect::new(1, 23, 4, 1)),
                ("↑1".into(), Rect::new(7, 23, 2, 1)),
            ]
        );

        // only the branch fits
        let layout = status_bar_layout(&segments, Rect::new(0, 23, 6, 1));
        assert_eq!(status_bar_texts(&layout).len(), 1);

        // nothing fits, and nothing panics
        assert!(status_bar_layout(&segments, Rect::new(0, 23, 4, 1)).is_empty());
        assert!(status_bar_layout(&segments, Rect::new(0, 23, 0, 1)).is_empty());
        assert!(status_bar_layout(&segments, Rect::new(0, 23, u16::MAX, 1)).len() == 4);
    }

    #[test]
    fn test_the_push_segment_is_hit_by_a_click_on_its_area() {
        let area = Rect::new(0, 23, 40, 1);
        let push_area = status_bar_layout(&status_bar_segments(), area)
            .into_iter()
            .find(|(segment, _)| matches!(segment, StatusBarSegment::Push { .. }))
            .map(|(_, rect)| rect);
        let app_status = AppStatus {
            status_bar_push_area: push_area,
            ..Default::default()
        };

        assert_eq!(
            app_status.status_bar_push_area,
            Some(Rect::new(15, 23, 8, 1))
        );
        assert!(app_status.is_on_status_bar_push(Position::new(15, 23)));
        assert!(app_status.is_on_status_bar_push(Position::new(22, 23)));
        // the separator, the segment before, and the line above are not part of the segment
        assert!(!app_status.is_on_status_bar_push(Position::new(14, 23)));
        assert!(!app_status.is_on_status_bar_push(Position::new(23, 23)));
        assert!(!app_status.is_on_status_bar_push(Position::new(15, 22)));

        // the push segment is not clickable when it is not rendered
        assert!(!AppStatus::default().is_on_status_bar_push(Position::new(15, 23)));
    }

    #[test]
    fn test_status_bar_segments_of_a_branch_that_tracks_a_remote() {
        let status_bar = StatusBar {
            head: Some(HeadName::Branch("main".into())),
            tracking: BranchTracking {
                upstream: Some("origin/main".into()),
                remote: Some("origin".into()),
                ahead: 1,
                behind: 2,
            },
            remotes: vec!["origin".into(), "fork".into()],
            actions_enabled: true,
        };

        // the numbers are shown only when they are not zero, and the push uses the remote of the
        // upstream even though the repository has several remotes
        assert_eq!(
            status_bar.segments(),
            vec![
                StatusBarSegment::Branch("main".into()),
                StatusBarSegment::Ahead(1),
                StatusBarSegment::Behind(2),
                StatusBarSegment::Push {
                    label: "origin".into(),
                },
            ]
        );
        assert_eq!(
            status_bar.push_target(),
            Some(PushTarget::Remote("origin".into()))
        );
        assert_eq!(status_bar.branch(), Some("main"));
    }

    #[test]
    fn test_status_bar_segments_without_upstream_or_remote() {
        let mut status_bar = StatusBar {
            head: Some(HeadName::Branch("main".into())),
            actions_enabled: true,
            ..Default::default()
        };
        // no upstream and no remote: only the branch, which cannot be pushed
        assert_eq!(
            status_bar.segments(),
            vec![StatusBarSegment::Branch("main".into())]
        );
        assert_eq!(status_bar.push_target(), None);

        // a single remote is used when the branch does not track one
        status_bar.remotes = vec!["origin".into()];
        assert_eq!(
            status_bar.push_target(),
            Some(PushTarget::Remote("origin".into()))
        );

        // several remotes: the remote is chosen in the menu
        status_bar.remotes = vec!["origin".into(), "fork".into()];
        assert_eq!(status_bar.push_target(), Some(PushTarget::ChooseRemote));
        assert_eq!(
            status_bar.segments(),
            vec![
                StatusBarSegment::Branch("main".into()),
                StatusBarSegment::Push {
                    label: "push".into(),
                },
            ]
        );
    }

    #[test]
    fn test_status_bar_segments_of_a_detached_head() {
        let status_bar = StatusBar {
            head: Some(HeadName::Detached("abc1234".into())),
            tracking: BranchTracking {
                upstream: Some("origin/main".into()),
                remote: Some("origin".into()),
                ahead: 1,
                behind: 1,
            },
            remotes: vec!["origin".into()],
            actions_enabled: true,
        };

        // a detached HEAD shows its short hash, and has no branch to push
        assert_eq!(
            status_bar.segments(),
            vec![
                StatusBarSegment::Branch("detached abc1234".into()),
                StatusBarSegment::Ahead(1),
                StatusBarSegment::Behind(1),
            ]
        );
        assert_eq!(status_bar.branch(), None);
        assert_eq!(status_bar.push_target(), None);
    }

    #[test]
    fn test_the_push_segment_is_not_shown_when_the_actions_are_disabled() {
        let status_bar = StatusBar {
            head: Some(HeadName::Branch("main".into())),
            tracking: BranchTracking {
                upstream: Some("origin/main".into()),
                remote: Some("origin".into()),
                ..Default::default()
            },
            remotes: vec!["origin".into()],
            actions_enabled: false,
        };

        assert_eq!(
            status_bar.segments(),
            vec![StatusBarSegment::Branch("main".into())]
        );
        assert_eq!(status_bar.push_target(), None);
    }

    #[test]
    fn test_push_target_of_a_branch_that_does_not_track_a_remote() {
        // the only remote, whichever it is
        assert_eq!(
            push_target(&BranchTracking::default(), &["fork".into()]),
            Some(PushTarget::Remote("fork".into()))
        );

        // no remote at all: there is nothing to push to
        assert_eq!(push_target(&BranchTracking::default(), &[]), None);

        // several remotes, and nothing to choose one with
        assert_eq!(
            push_target(
                &BranchTracking::default(),
                &["origin".into(), "fork".into()]
            ),
            Some(PushTarget::ChooseRemote)
        );

        // the remote of the upstream is preferred over the only remote
        let tracking = BranchTracking {
            upstream: Some("fork/main".into()),
            remote: Some("fork".into()),
            ..Default::default()
        };
        assert_eq!(
            push_target(&tracking, &["origin".into(), "fork".into()]),
            Some(PushTarget::Remote("fork".into()))
        );
    }
}
