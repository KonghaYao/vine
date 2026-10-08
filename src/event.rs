use std::{
    fmt::{self, Debug, Formatter},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
};

use ratatui::crossterm::event::{KeyEvent, MouseEvent, MouseEventKind};
use serde::{
    de::{self, Deserializer, Visitor},
    Deserialize,
};

use crate::{
    git::Ref,
    git_action::GitAction,
    view::RefreshViewContext,
    watch::{RepoWatcher, WatchPaths},
};

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(usize, usize),
    Quit,
    OpenDetail,
    CloseDetail,
    OpenUserCommand(usize),
    CloseUserCommand,
    OpenRefs,
    CloseRefs,
    OpenActions,
    CloseActions,
    ExecuteGitAction(Box<GitAction>),
    /// Switches the repository to the given ref, which was double clicked in the commit list.
    SwitchToRef(Box<Ref>),
    RemoteActionFinished {
        action: Box<GitAction>,
        result: Result<String, String>,
    },
    OpenActionOutput(String),
    OpenHelp,
    CloseHelp,
    SelectNewerCommit,
    SelectOlderCommit,
    SelectParentCommit,
    CopyToClipboard {
        name: String,
        value: String,
    },
    Refresh(RefreshViewContext),
    /// The repository was changed by another process, and should be refreshed.
    RepoChanged,
    ClearStatusLine,
    UpdateStatusInput {
        message: String,
        cursor_position: u16,
        metadata: String,
    },
    UpdateStatusTransient(String),
    UpdateSearchResult {
        message: String,
        options: String,
        matched: bool,
    },
    NotifyInfo(String),
    NotifySuccess(String),
    NotifyWarn(String),
    NotifyError(String),
}

#[derive(Clone)]
pub struct Sender {
    tx: mpsc::Sender<AppEvent>,
}

impl Sender {
    pub fn send(&self, event: AppEvent) {
        self.tx.send(event).unwrap();
    }

    /// Sends the event, ignoring the failure when the application is already shutting down.
    ///
    /// This is used by background tasks, which may finish after the application stopped.
    pub fn send_if_running(&self, event: AppEvent) {
        let _ = self.tx.send(event);
    }
}

impl Debug for Sender {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Sender")
    }
}

pub struct Receiver {
    rx: mpsc::Receiver<AppEvent>,
}

impl Receiver {
    fn recv(&self) -> AppEvent {
        self.rx.recv().unwrap()
    }
}

impl Debug for Receiver {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Receiver")
    }
}

#[derive(Debug)]
pub struct EventController {
    tx: Sender,
    rx: Receiver,
    stop: Arc<AtomicBool>,
    handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    mouse_capture: bool,
    watcher: Mutex<Option<RepoWatcher>>,
}

impl EventController {
    pub fn init(mouse_capture: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        let tx = Sender { tx };
        let rx = Receiver { rx };

        let controller = EventController {
            tx: tx.clone(),
            rx,
            stop: Arc::new(AtomicBool::new(false)),
            handle: Arc::new(Mutex::new(None)),
            mouse_capture,
            watcher: Mutex::new(None),
        };
        controller.start();

        controller
    }

    /// Watches the repository at the given path, refreshing the application when it is changed
    /// by another process.
    ///
    /// Nothing is watched when the watch cannot be started, in which case the application is
    /// notified and can still be refreshed manually. The watcher is stopped with this controller.
    pub fn watch_repository(&self, repo_path: &Path) {
        let watcher = WatchPaths::resolve(repo_path)
            .and_then(|paths| RepoWatcher::start(&paths, self.tx.clone()));
        match watcher {
            Ok(watcher) => *self.watcher.lock().unwrap() = Some(watcher),
            Err(err) => {
                self.tx.send(AppEvent::NotifyWarn(format!(
                    "Automatic refresh is unavailable: {err}"
                )));
            }
        }
    }

    pub fn start(&self) {
        self.stop.store(false, Ordering::Relaxed);
        let stop = self.stop.clone();
        let tx = self.tx.clone();
        let handle = thread::spawn(move || loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match ratatui::crossterm::event::poll(std::time::Duration::from_millis(100)) {
                Ok(true) => match ratatui::crossterm::event::read() {
                    Ok(e) => match e {
                        ratatui::crossterm::event::Event::Key(key) => {
                            tx.send(AppEvent::Key(key));
                        }
                        ratatui::crossterm::event::Event::Resize(w, h) => {
                            tx.send(AppEvent::Resize(w as usize, h as usize));
                        }
                        // Idle mouse movement has no effect, so it is not forwarded to avoid
                        // redrawing the terminal on every movement.
                        // Allow it again when hover support is implemented (see PLAN §6-A-4).
                        ratatui::crossterm::event::Event::Mouse(mouse)
                            if is_forwarded_mouse_event(&mouse) =>
                        {
                            tx.send(AppEvent::Mouse(mouse));
                        }
                        _ => {}
                    },
                    Err(e) => {
                        panic!("Failed to read event: {e}");
                    }
                },
                Ok(false) => {
                    continue;
                }
                Err(e) => {
                    panic!("Failed to poll event: {e}");
                }
            }
        });
        *self.handle.lock().unwrap() = Some(handle);
    }

    pub fn resume(&self) {
        ratatui::crossterm::execute!(
            std::io::stdout(),
            ratatui::crossterm::terminal::EnterAlternateScreen
        )
        .unwrap();
        ratatui::crossterm::terminal::enable_raw_mode().unwrap();

        self.drain_crossterm_event();
        self.start();
        self.enable_mouse_capture();

        if let Some(watcher) = &*self.watcher.lock().unwrap() {
            watcher.resume();
        }
    }

    pub fn suspend(&self) {
        if let Some(watcher) = &*self.watcher.lock().unwrap() {
            watcher.pause();
        }

        self.stop();
        self.disable_mouse_capture();

        ratatui::crossterm::terminal::disable_raw_mode().unwrap();
        ratatui::crossterm::execute!(
            std::io::stdout(),
            ratatui::crossterm::terminal::LeaveAlternateScreen
        )
        .unwrap();
    }

    /// Tells the repository watcher that the repository was loaded, so that the changes it
    /// already contains are not refreshed again.
    pub fn mark_repository_loaded(&self) {
        if let Some(watcher) = &*self.watcher.lock().unwrap() {
            watcher.mark_reloaded();
        }
    }

    /// Enable the mouse capture, so that mouse events are reported to the application.
    ///
    /// Does nothing when the mouse is disabled by the configuration.
    pub fn enable_mouse_capture(&self) {
        if !self.mouse_capture {
            return;
        }
        ratatui::crossterm::execute!(
            std::io::stdout(),
            ratatui::crossterm::event::EnableMouseCapture
        )
        .unwrap();
    }

    /// Disable the mouse capture, so that the mouse is handled by the outer program again.
    ///
    /// Does nothing when the mouse is disabled by the configuration.
    pub fn disable_mouse_capture(&self) {
        if !self.mouse_capture {
            return;
        }
        ratatui::crossterm::execute!(
            std::io::stdout(),
            ratatui::crossterm::event::DisableMouseCapture
        )
        .unwrap();
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.lock().unwrap().take() {
            handle.join().unwrap();
        }
    }

    fn drain_crossterm_event(&self) {
        while let Ok(true) = ratatui::crossterm::event::poll(std::time::Duration::from_millis(0)) {
            let _ = ratatui::crossterm::event::read();
        }
    }

    pub fn sender(&self) -> Sender {
        self.tx.clone()
    }

    pub fn send(&self, event: AppEvent) {
        self.tx.send(event);
    }

    pub fn recv(&self) -> AppEvent {
        self.rx.recv()
    }
}

/// Whether the mouse event should be forwarded to the application.
///
/// Idle mouse movement has no effect, so it is not forwarded to avoid redrawing the terminal on
/// every movement. Idle movement should be forwarded again when hover support is implemented
/// (see PLAN §6-A-4).
fn is_forwarded_mouse_event(mouse: &MouseEvent) -> bool {
    !matches!(mouse.kind, MouseEventKind::Moved)
}

/// Install a panic hook that disables the mouse capture before the previously installed hook runs.
///
/// The hook installed by `ratatui::init()` restores the raw mode and the alternate screen, but it
/// does not disable the mouse capture. Without this hook, the terminal is left in mouse mode when
/// the application panics, so this must be installed *after* `ratatui::init()`.
pub fn install_mouse_capture_panic_hook() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = ratatui::crossterm::execute!(
            std::io::stdout(),
            ratatui::crossterm::event::DisableMouseCapture
        );
        hook(panic_info);
    }));
}

// The event triggered by user's key input
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UserEvent {
    ForceQuit,
    Quit,
    HelpToggle,
    Cancel,
    Close,
    NavigateUp,
    NavigateDown,
    NavigateRight,
    NavigateLeft,
    SelectUp,
    SelectDown,
    GoToTop,
    GoToBottom,
    GoToParent,
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    HalfPageUp,
    HalfPageDown,
    SelectTop,
    SelectMiddle,
    SelectBottom,
    GoToNext,
    GoToPrevious,
    Confirm,
    RefList,
    Actions,
    Search,
    UserCommand(usize),
    SearchTargetToggle,
    IgnoreCaseToggle,
    FuzzyToggle,
    Refresh,
    ShortCopy,
    FullCopy,
    Unknown,
}

impl<'de> Deserialize<'de> for UserEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UserEventVisitor;

        impl<'de> Visitor<'de> for UserEventVisitor {
            type Value = UserEvent;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string representing a user event")
            }

            fn visit_str<E>(self, value: &str) -> Result<UserEvent, E>
            where
                E: de::Error,
            {
                if value.starts_with("user_command_") {
                    if let Some(num) = parse_user_command_number(value) {
                        Ok(UserEvent::UserCommand(num))
                    } else {
                        let msg = format!("Invalid user_command_n format: {value}",);
                        Err(de::Error::custom(msg))
                    }
                } else {
                    match value {
                        "force_quit" => Ok(UserEvent::ForceQuit),
                        "quit" => Ok(UserEvent::Quit),
                        "help_toggle" => Ok(UserEvent::HelpToggle),
                        "cancel" => Ok(UserEvent::Cancel),
                        "close" => Ok(UserEvent::Close),
                        "navigate_up" => Ok(UserEvent::NavigateUp),
                        "navigate_down" => Ok(UserEvent::NavigateDown),
                        "navigate_right" => Ok(UserEvent::NavigateRight),
                        "navigate_left" => Ok(UserEvent::NavigateLeft),
                        "select_up" => Ok(UserEvent::SelectUp),
                        "select_down" => Ok(UserEvent::SelectDown),
                        "go_to_top" => Ok(UserEvent::GoToTop),
                        "go_to_bottom" => Ok(UserEvent::GoToBottom),
                        "go_to_parent" => Ok(UserEvent::GoToParent),
                        "scroll_up" => Ok(UserEvent::ScrollUp),
                        "scroll_down" => Ok(UserEvent::ScrollDown),
                        "page_up" => Ok(UserEvent::PageUp),
                        "page_down" => Ok(UserEvent::PageDown),
                        "half_page_up" => Ok(UserEvent::HalfPageUp),
                        "half_page_down" => Ok(UserEvent::HalfPageDown),
                        "select_top" => Ok(UserEvent::SelectTop),
                        "select_middle" => Ok(UserEvent::SelectMiddle),
                        "select_bottom" => Ok(UserEvent::SelectBottom),
                        "go_to_next" => Ok(UserEvent::GoToNext),
                        "go_to_previous" => Ok(UserEvent::GoToPrevious),
                        "confirm" => Ok(UserEvent::Confirm),
                        "ref_list" | "ref_list_toggle" => Ok(UserEvent::RefList),
                        "open_actions" => Ok(UserEvent::Actions),
                        "search" => Ok(UserEvent::Search),
                        "search_target_toggle" => Ok(UserEvent::SearchTargetToggle),
                        "ignore_case_toggle" => Ok(UserEvent::IgnoreCaseToggle),
                        "fuzzy_toggle" => Ok(UserEvent::FuzzyToggle),
                        "refresh" => Ok(UserEvent::Refresh),
                        "short_copy" => Ok(UserEvent::ShortCopy),
                        "full_copy" => Ok(UserEvent::FullCopy),
                        _ => {
                            let msg = format!("Unknown user event: {value}");
                            Err(de::Error::custom(msg))
                        }
                    }
                }
            }
        }

        deserializer.deserialize_str(UserEventVisitor)
    }
}

fn parse_user_command_number(s: &str) -> Option<usize> {
    if let Some(num_str) = s.strip_prefix("user_command_") {
        if num_str.parse::<usize>().is_ok() {
            return num_str.parse::<usize>().ok();
        }
        if let Some(num_str) = s.strip_prefix("user_command_view_toggle_") {
            if num_str.parse::<usize>().is_ok() {
                return num_str.parse::<usize>().ok();
            }
        }
    }
    None
}

impl UserEvent {
    pub fn is_countable(&self) -> bool {
        matches!(
            self,
            UserEvent::NavigateUp
                | UserEvent::NavigateDown
                | UserEvent::ScrollUp
                | UserEvent::ScrollDown
                | UserEvent::GoToParent
                | UserEvent::PageUp
                | UserEvent::PageDown
                | UserEvent::HalfPageUp
                | UserEvent::HalfPageDown
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserEventWithCount {
    pub event: UserEvent,
    pub count: usize,
}

impl UserEventWithCount {
    pub fn new(event: UserEvent, count: usize) -> Self {
        Self {
            event,
            count: if count == 0 { 1 } else { count },
        }
    }

    pub fn from_event(event: UserEvent) -> Self {
        Self::new(event, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mouse_event(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        }
    }

    #[test]
    fn test_idle_mouse_movement_is_not_forwarded() {
        assert!(!is_forwarded_mouse_event(&mouse_event(
            MouseEventKind::Moved
        )));
    }

    #[test]
    fn test_mouse_operations_are_forwarded() {
        for kind in [
            MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
            MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Right),
            MouseEventKind::Up(ratatui::crossterm::event::MouseButton::Left),
            MouseEventKind::Drag(ratatui::crossterm::event::MouseButton::Left),
            MouseEventKind::ScrollUp,
            MouseEventKind::ScrollDown,
        ] {
            assert!(is_forwarded_mouse_event(&mouse_event(kind)), "{kind:?}");
        }
    }

    #[test]
    fn test_user_event_with_count_new() {
        let event = UserEventWithCount::new(UserEvent::NavigateUp, 5);
        assert_eq!(event.event, UserEvent::NavigateUp);
        assert_eq!(event.count, 5);
    }

    #[test]
    fn test_user_event_with_count_new_zero_count() {
        let event = UserEventWithCount::new(UserEvent::NavigateDown, 0);
        assert_eq!(event.event, UserEvent::NavigateDown);
        assert_eq!(event.count, 1); // zero should be converted to 1
    }

    #[test]
    fn test_user_event_with_count_from_event() {
        let event = UserEventWithCount::from_event(UserEvent::NavigateLeft);
        assert_eq!(event.event, UserEvent::NavigateLeft);
        assert_eq!(event.count, 1);
    }

    #[test]
    fn test_user_event_with_count_equality() {
        let event1 = UserEventWithCount::new(UserEvent::ScrollUp, 3);
        let event2 = UserEventWithCount::new(UserEvent::ScrollUp, 3);
        let event3 = UserEventWithCount::new(UserEvent::ScrollDown, 3);

        assert_eq!(event1, event2);
        assert_ne!(event1, event3);
    }
}
