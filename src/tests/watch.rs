use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use notify::RecursiveMode;

use crate::{
    test_git::GitRepository,
    watch::{RepoWatcher, WatchPaths},
};

/// The time to wait for the watch to report a change.
///
/// The watch reports a change after the file system was quiet, so a couple of seconds is enough
/// when nothing is wrong, but the backends are given some slack.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The time the watch is given to report a change that should not be reported.
const QUIET: Duration = Duration::from_secs(2);

/// Watches the repository at the given path, returning the reported changes.
fn watch_repository(path: &Path) -> (RepoWatcher, mpsc::Receiver<()>) {
    let paths = WatchPaths::resolve(path).unwrap();
    let (tx, rx) = mpsc::channel();
    let watcher = RepoWatcher::start_with(&paths, move || {
        let _ = tx.send(());
    })
    .unwrap();
    (watcher, rx)
}

#[test]
fn test_watch_reports_branch_and_commit_changes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = GitRepository::new(dir.path());
    repo.init();
    repo.commit("first", "2023-01-01");

    let (_watcher, rx) = watch_repository(dir.path());

    // A branch is created and checked out.
    repo.checkout_b("feature");
    assert!(
        rx.recv_timeout(TIMEOUT).is_ok(),
        "creating a branch was not reported"
    );

    // A commit is added to it.
    repo.commit("second", "2023-01-02");
    assert!(
        rx.recv_timeout(TIMEOUT).is_ok(),
        "a commit was not reported"
    );

    // Another branch is checked out.
    repo.checkout("master");
    assert!(
        rx.recv_timeout(TIMEOUT).is_ok(),
        "switching a branch was not reported"
    );
}

#[test]
fn test_watch_does_not_report_reads_of_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    let repo = GitRepository::new(dir.path());
    repo.init();
    repo.commit("first", "2023-01-01");

    let (_watcher, rx) = watch_repository(dir.path());

    // Reading the repository, in the same way as the application does when it loads it, must not
    // be reported as a change, as that would refresh the application over and over.
    for _ in 0..10 {
        repo.rev_parse_head();
        repo.run(&["log", "--oneline", "-1"], "");
        repo.run(&["for-each-ref"], "");
    }

    assert!(
        rx.recv_timeout(QUIET).is_err(),
        "reading the repository was reported as a change"
    );
}

#[test]
fn test_watch_stops_when_it_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let repo = GitRepository::new(dir.path());
    repo.init();
    repo.commit("first", "2023-01-01");

    let (watcher, rx) = watch_repository(dir.path());

    drop(watcher);

    // The thread of the watch is joined when it is dropped, so it is gone by the time the
    // application exits (also when it exits by panicking).
    assert!(matches!(
        rx.recv_timeout(QUIET),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));

    // and no change is reported anymore.
    repo.commit("second", "2023-01-02");
    assert!(matches!(
        rx.recv_timeout(QUIET),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
}

#[test]
fn test_watch_paths_of_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    let repo = GitRepository::new(dir.path());
    repo.init();
    repo.commit("first", "2023-01-01");

    let paths = WatchPaths::resolve(dir.path()).unwrap();

    let git_dir = dir.path().canonicalize().unwrap().join(".git");
    assert_eq!(
        watch_paths(&paths),
        vec![
            // HEAD, packed-refs, ... are all directly in the metadata directory, which is not
            // watched recursively so that the objects are left out.
            (git_dir.clone(), RecursiveMode::NonRecursive),
            (git_dir.join("refs"), RecursiveMode::Recursive),
        ]
    );
}

#[test]
fn test_watch_paths_of_a_bare_repository() {
    let dir = tempfile::tempdir().unwrap();
    let repo = GitRepository::new(dir.path());
    repo.run(&["init", "--bare", "-b", "master"], "");

    let paths = WatchPaths::resolve(dir.path()).unwrap();

    let git_dir = dir.path().canonicalize().unwrap();
    assert_eq!(
        watch_paths(&paths),
        vec![
            (git_dir.clone(), RecursiveMode::NonRecursive),
            (git_dir.join("refs"), RecursiveMode::Recursive),
        ]
    );
}

#[test]
fn test_watch_paths_of_a_linked_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let repo = GitRepository::new(dir.path());
    repo.init();
    repo.commit("first", "2023-01-01");

    let worktree_dir = tempfile::tempdir().unwrap();
    let worktree = worktree_dir.path().to_str().unwrap();
    repo.run(&["worktree", "add", worktree, "-b", "worktree-branch"], "");

    let paths = WatchPaths::resolve(worktree_dir.path()).unwrap();
    let paths = watch_paths(&paths);

    let git_dir = dir.path().canonicalize().unwrap().join(".git");
    let worktree_git_dir = paths
        .iter()
        .find(|(path, mode)| {
            *mode == RecursiveMode::NonRecursive && path.starts_with(git_dir.join("worktrees"))
        })
        .expect("the metadata directory of the worktree should be watched");
    assert!(!worktree_git_dir.0.ends_with("worktrees"));

    // HEAD is in the worktree metadata directory, but the refs are shared with the repository
    // the worktree was created from.
    assert!(paths.contains(&(git_dir.clone(), RecursiveMode::NonRecursive)));
    assert!(paths.contains(&(git_dir.join("refs"), RecursiveMode::Recursive)));
}

fn watch_paths(paths: &WatchPaths) -> Vec<(PathBuf, RecursiveMode)> {
    paths
        .paths()
        .map(|(path, mode)| (path.to_path_buf(), mode))
        .collect()
}
