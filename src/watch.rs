//! Automatic refresh of the repository by watching the git directory.
//!
//! Git does not provide a way to be notified of changes, so the files that the commit graph is
//! built from are watched with `notify` instead: the directory of the repository metadata
//! (`HEAD`, `packed-refs`, ...) and the `refs` directory.
//!
//! A single git operation writes several files in a row, and the refresh is expensive (the whole
//! graph is rebuilt and the images are uploaded again), so the events are merged by a quiet
//! period ([`DEBOUNCE`]) and the refreshes are throttled ([`MIN_REFRESH_INTERVAL`]).

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::event::{AppEvent, Sender};

/// How long the watched paths must not change before the changes are refreshed.
///
/// The events of a single git operation are merged until the file system is quiet for this
/// duration.
const DEBOUNCE: Duration = Duration::from_millis(400);

/// The minimum interval between two automatic refreshes.
const MIN_REFRESH_INTERVAL: Duration = Duration::from_millis(1_000);

/// The interval at which the watcher thread wakes up when it has nothing to do.
const IDLE_INTERVAL: Duration = Duration::from_millis(100);

/// Decides when the file system events are turned into a single refresh.
///
/// This is a pure state machine over the current time, so that it can be tested without touching
/// the file system.
#[derive(Debug, Clone)]
pub struct RefreshDebouncer {
    debounce: Duration,
    min_interval: Duration,
    /// When the latest event of the pending changes was seen.
    last_event: Option<Instant>,
    /// When the last refresh was triggered.
    last_refresh: Option<Instant>,
}

impl RefreshDebouncer {
    pub fn new(debounce: Duration, min_interval: Duration) -> Self {
        Self {
            debounce,
            min_interval,
            last_event: None,
            last_refresh: None,
        }
    }

    /// Records that the watched paths were changed.
    pub fn on_event(&mut self, now: Instant) {
        self.last_event = Some(now);
    }

    /// Forgets the recorded changes, so that they do not trigger a refresh.
    pub fn clear(&mut self) {
        self.last_event = None;
    }

    /// Whether the recorded changes should trigger a refresh now.
    pub fn should_trigger(&self, now: Instant) -> bool {
        let Some(last_event) = self.last_event else {
            return false;
        };
        if now.saturating_duration_since(last_event) < self.debounce {
            return false;
        }
        self.last_refresh
            .is_none_or(|last| now.saturating_duration_since(last) >= self.min_interval)
    }

    /// Triggers a refresh for the recorded changes.
    pub fn trigger(&mut self, now: Instant) {
        self.last_event = None;
        self.last_refresh = Some(now);
    }

    /// Triggers a refresh if one is due, returning whether it was triggered.
    pub fn poll(&mut self, now: Instant) -> bool {
        if !self.should_trigger(now) {
            return false;
        }
        self.trigger(now);
        true
    }

    /// How long the caller may wait before checking again.
    pub fn sleep_time(&self, now: Instant) -> Duration {
        let Some(last_event) = self.last_event else {
            return IDLE_INTERVAL;
        };
        let quiet = self
            .debounce
            .saturating_sub(now.saturating_duration_since(last_event));
        let interval = self.last_refresh.map_or(Duration::ZERO, |last| {
            self.min_interval
                .saturating_sub(now.saturating_duration_since(last))
        });
        quiet.max(interval).max(Duration::from_millis(1))
    }
}

/// The application states in which an automatic refresh must be postponed.
///
/// A refresh rebuilds the whole repository and replaces the state of the application, which
/// interrupts the user and, in some states, repeats what the application is already doing, so it
/// is deferred until the application is idle again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutoRefreshBlockers {
    /// The user is entering input, such as a search query, in the status line.
    pub input_in_progress: bool,
    /// A menu, a dialog, or the help screen is open.
    pub overlay_open: bool,
    /// The output of a user command is shown, which a refresh would run again.
    pub user_command_open: bool,
    /// A git action is running in the background, which owns the state to be restored.
    pub remote_action_in_progress: bool,
}

impl AutoRefreshBlockers {
    /// Whether an automatic refresh must not be performed in the current state.
    pub fn blocks(self) -> bool {
        self.input_in_progress
            || self.overlay_open
            || self.user_command_open
            || self.remote_action_in_progress
    }
}

/// The paths that are watched for changes of the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchPaths {
    paths: Vec<(PathBuf, RecursiveMode)>,
}

impl WatchPaths {
    /// Resolves the paths to watch for the repository at the given path.
    ///
    /// The paths are resolved with `git rev-parse`, so that linked worktrees and bare
    /// repositories work as well, where the refs and `HEAD` are not in the same directory.
    pub fn resolve(repo_path: &Path) -> Result<Self, String> {
        let git_dir = canonicalize(rev_parse_path(repo_path, "--absolute-git-dir")?);
        let common_dir = rev_parse_path(repo_path, "--git-common-dir")?;
        let common_dir = canonicalize(if common_dir.is_absolute() {
            common_dir
        } else {
            repo_path.join(common_dir)
        });

        // The directories are watched instead of the files themselves, as `HEAD` and
        // `packed-refs` are replaced by a rename when they are updated, which drops the watch on
        // the file itself on some platforms.
        let mut paths: Vec<(PathBuf, RecursiveMode)> = Vec::new();
        for dir in [git_dir, common_dir.clone()] {
            if !paths.iter().any(|(path, _)| *path == dir) {
                paths.push((dir, RecursiveMode::NonRecursive));
            }
        }

        // The whole repository metadata directory is not watched recursively, as the objects
        // directory changes on every fetch, which would only cause pointless refreshes.
        let refs_dir = common_dir.join("refs");
        if refs_dir.is_dir() {
            paths.push((canonicalize(refs_dir), RecursiveMode::Recursive));
        }

        Ok(Self { paths })
    }

    /// The watched paths and how they are watched.
    pub fn paths(&self) -> impl Iterator<Item = (&Path, RecursiveMode)> {
        self.paths
            .iter()
            .map(|(path, mode)| (path.as_path(), *mode))
    }
}

/// Watches the repository and notifies the application of the changes.
///
/// The watch runs on its own thread, which is stopped when this is dropped.
#[derive(Debug)]
pub struct RepoWatcher {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    reloaded: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl RepoWatcher {
    /// Watches the given paths, sending `AppEvent::RepoChanged` when they are changed.
    pub fn start(watch_paths: &WatchPaths, tx: Sender) -> Result<Self, String> {
        Self::start_with(watch_paths, move || {
            tx.send_if_running(AppEvent::RepoChanged);
        })
    }

    /// Watches the given paths, calling the given function when they are changed.
    ///
    /// The function is called instead of sending an event, so that the watch can be tested
    /// without an application running.
    pub(crate) fn start_with<F>(watch_paths: &WatchPaths, on_change: F) -> Result<Self, String>
    where
        F: FnMut() + Send + 'static,
    {
        let (raw_tx, raw_rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(raw_tx)
            .map_err(|e| format!("failed to create a file watcher: {e}"))?;
        for (path, mode) in watch_paths.paths() {
            watcher
                .watch(path, mode)
                .map_err(|e| format!("failed to watch {}: {e}", path.display()))?;
        }

        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let reloaded = Arc::new(AtomicBool::new(false));

        let handle = thread::spawn({
            let stop = Arc::clone(&stop);
            let paused = Arc::clone(&paused);
            let reloaded = Arc::clone(&reloaded);
            move || watch_loop(watcher, raw_rx, on_change, stop, paused, reloaded)
        });

        Ok(Self {
            stop,
            paused,
            reloaded,
            handle: Some(handle),
        })
    }

    /// Stops notifying changes, e.g. while the terminal is used by another program.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }

    /// Notifies changes again after [`RepoWatcher::pause`].
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }

    /// Forgets the changes that were already applied, e.g. by the refresh of the application.
    pub fn mark_reloaded(&self) {
        self.reloaded.store(true, Ordering::Relaxed);
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for RepoWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

fn watch_loop<F>(
    // Kept alive so that the watch stays registered.
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<notify::Result<Event>>,
    mut on_change: F,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    reloaded: Arc<AtomicBool>,
) where
    F: FnMut(),
{
    let mut debouncer = RefreshDebouncer::new(DEBOUNCE, MIN_REFRESH_INTERVAL);
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();

        if paused.load(Ordering::Relaxed) {
            // Discard the changes of the suspended period, together with the ones that were not
            // applied yet, as a refresh would only interrupt the user when returning.
            debouncer.clear();
            discard_events(&rx);
            thread::sleep(IDLE_INTERVAL);
            continue;
        }
        if reloaded.swap(false, Ordering::Relaxed) {
            // The changes seen so far are part of the repository the application has just
            // loaded, so they are discarded. A change that is still in progress keeps sending
            // events, and is therefore refreshed as usual.
            debouncer.clear();
            discard_events(&rx);
        }

        if debouncer.poll(now) {
            on_change();
        }

        match rx.recv_timeout(debouncer.sleep_time(now)) {
            Ok(Ok(event)) if !is_relevant_event(&event) => {}
            Ok(_) => {
                // Both a change and a watch error are a reason to refresh the application.
                debouncer.on_event(Instant::now());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn discard_events(rx: &mpsc::Receiver<notify::Result<Event>>) {
    while rx.try_recv().is_ok() {}
}

/// Whether the event is a change of the watched paths.
///
/// Opening a watched file is an access, not a change, and every read of the repository by the
/// application itself is reported as one on some platforms.
fn is_relevant_event(event: &Event) -> bool {
    !matches!(event.kind, EventKind::Access(_))
}

/// Resolves a path with `git rev-parse`, in the same way as the repository is loaded.
fn rev_parse_path(repo_path: &Path, option: &str) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg(option)
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("failed to run `git rev-parse {option}`: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "`git rev-parse {option}` failed: {}",
            stderr.trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let path = stdout.trim();
    if path.is_empty() {
        return Err(format!("`git rev-parse {option}` returned no path"));
    }
    Ok(PathBuf::from(path))
}

/// The canonicalized path, or the path itself when it cannot be canonicalized.
fn canonicalize(path: PathBuf) -> PathBuf {
    match path.canonicalize() {
        Ok(path) => path,
        Err(_) => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn debounce() -> Duration {
        Duration::from_millis(400)
    }

    fn interval() -> Duration {
        Duration::from_millis(1_000)
    }

    #[test]
    fn test_debouncer_triggers_after_the_quiet_period() {
        let mut debouncer = RefreshDebouncer::new(debounce(), interval());
        let now = Instant::now();

        debouncer.on_event(now);

        assert!(!debouncer.poll(now + Duration::from_millis(399)));
        assert!(debouncer.poll(now + debounce()));
    }

    #[test]
    fn test_debouncer_does_not_trigger_without_events() {
        let mut debouncer = RefreshDebouncer::new(debounce(), interval());
        let now = Instant::now();

        assert!(!debouncer.poll(now));
        assert!(!debouncer.poll(now + Duration::from_secs(60)));
    }

    #[test]
    fn test_debouncer_merges_a_burst_into_a_single_refresh() {
        let mut debouncer = RefreshDebouncer::new(debounce(), interval());
        let now = Instant::now();

        debouncer.on_event(now);
        debouncer.on_event(now + Duration::from_millis(100));
        debouncer.on_event(now + Duration::from_millis(200));

        // The quiet period starts at the latest event of the burst.
        assert!(!debouncer.poll(now + Duration::from_millis(500)));
        assert!(debouncer.poll(now + Duration::from_millis(600)));
    }

    #[test]
    fn test_debouncer_waits_for_the_minimum_interval() {
        let mut debouncer = RefreshDebouncer::new(debounce(), interval());
        let now = Instant::now();

        debouncer.on_event(now);
        assert!(debouncer.poll(now + debounce()));

        debouncer.on_event(now + Duration::from_millis(500));

        // The quiet period is over, but the interval since the last refresh is not.
        assert!(!debouncer.poll(now + Duration::from_millis(900)));
        assert!(!debouncer.poll(now + Duration::from_millis(1_399)));
        assert!(debouncer.poll(now + Duration::from_millis(1_400)));
    }

    #[test]
    fn test_debouncer_does_not_lose_the_events_of_the_waiting_period() {
        let mut debouncer = RefreshDebouncer::new(debounce(), interval());
        let now = Instant::now();

        debouncer.on_event(now);
        assert!(debouncer.poll(now + debounce()));

        debouncer.on_event(now + Duration::from_millis(500));
        assert!(!debouncer.poll(now + Duration::from_millis(900)));
        assert!(debouncer.poll(now + Duration::from_millis(1_400)));

        // The waiting period is over once the refresh was triggered.
        assert!(!debouncer.poll(now + Duration::from_millis(1_500)));
    }

    #[test]
    fn test_debouncer_cleared_events_do_not_trigger() {
        let mut debouncer = RefreshDebouncer::new(debounce(), interval());
        let now = Instant::now();

        debouncer.on_event(now);
        debouncer.clear();

        assert!(!debouncer.poll(now + Duration::from_secs(60)));
    }

    #[test]
    fn test_debouncer_sleep_time_waits_for_the_quiet_period() {
        let mut debouncer = RefreshDebouncer::new(debounce(), interval());
        let now = Instant::now();

        assert_eq!(debouncer.sleep_time(now), IDLE_INTERVAL);

        debouncer.on_event(now);
        assert_eq!(debouncer.sleep_time(now), debounce());
        assert_eq!(
            debouncer.sleep_time(now + Duration::from_millis(100)),
            Duration::from_millis(300)
        );

        // Changes are waited for again once they were refreshed.
        debouncer.trigger(now);
        assert_eq!(debouncer.sleep_time(now), IDLE_INTERVAL);
    }

    #[test]
    fn test_debouncer_sleep_time_waits_for_the_minimum_interval() {
        let mut debouncer = RefreshDebouncer::new(debounce(), interval());
        let now = Instant::now();

        debouncer.on_event(now);
        debouncer.trigger(now);
        debouncer.on_event(now + Duration::from_millis(100));

        assert_eq!(
            debouncer.sleep_time(now + Duration::from_millis(500)),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn test_blockers_are_not_blocking_by_default() {
        assert!(!AutoRefreshBlockers::default().blocks());
    }

    #[test]
    fn test_blockers_block_in_the_states_to_postpone() {
        let blocked = [
            AutoRefreshBlockers {
                input_in_progress: true,
                ..Default::default()
            },
            AutoRefreshBlockers {
                overlay_open: true,
                ..Default::default()
            },
            AutoRefreshBlockers {
                user_command_open: true,
                ..Default::default()
            },
            AutoRefreshBlockers {
                remote_action_in_progress: true,
                ..Default::default()
            },
        ];

        for blockers in blocked {
            assert!(blockers.blocks(), "{blockers:?}");
        }
    }

    #[test]
    fn test_relevant_events_are_changes_only() {
        let change = Event::new(EventKind::Modify(notify::event::ModifyKind::Any));
        let create = Event::new(EventKind::Create(notify::event::CreateKind::Any));
        let remove = Event::new(EventKind::Remove(notify::event::RemoveKind::Any));
        let access = Event::new(EventKind::Access(notify::event::AccessKind::Open(
            notify::event::AccessMode::Any,
        )));

        assert!(is_relevant_event(&change));
        assert!(is_relevant_event(&create));
        assert!(is_relevant_event(&remove));
        assert!(!is_relevant_event(&access));
    }
}
