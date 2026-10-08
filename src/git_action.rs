use std::{
    path::Path,
    process::{Command, Stdio},
};

use rustc_hash::FxHashMap;

use crate::{
    config::{CommitListColumn, UserCommand},
    git::{CommitHash, Ref},
};

/// A git action that can be executed by the application.
///
/// Actions that are not git commands (copying to the clipboard, opening a view, ...) are also
/// represented here, and they are executed by the application instead of git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitAction {
    CopyCommitHash {
        commit_hash: CommitHash,
    },
    ShowCommitDetail,
    RunUserCommand {
        number: usize,
    },
    /// `git switch -c <name> <target>`
    CreateBranch {
        name: String,
        target: CommitHash,
    },
    /// `git switch <name>`
    SwitchBranch {
        name: String,
    },
    /// `git switch -c <name> --track <start_point>`
    SwitchTrackingBranch {
        name: String,
        start_point: String,
    },
    /// `git merge <name>`
    MergeBranch {
        name: String,
    },
    /// `git rebase <name>`
    RebaseBranch {
        name: String,
    },
    /// `git branch -m <old_name> <new_name>`
    RenameBranch {
        old_name: String,
        new_name: String,
    },
    /// `git branch -d <name>`, or `git branch -D <name>` when forced
    DeleteBranch {
        name: String,
        force: bool,
    },
    /// `git switch --detach <commit_hash>`
    CheckoutDetached {
        commit_hash: CommitHash,
    },
    /// `git cherry-pick <commit_hash>`
    CherryPick {
        commit_hash: CommitHash,
    },
    /// `git revert --no-edit <commit_hash>`
    Revert {
        commit_hash: CommitHash,
    },
    /// `git reset <mode> <commit_hash>`
    ResetCommit {
        commit_hash: CommitHash,
        mode: ResetMode,
    },
    /// `git tag <name> <target>`
    CreateTag {
        name: String,
        target: CommitHash,
    },
    /// `git tag -d <name>`
    DeleteTag {
        name: String,
    },
    /// `git stash apply <name>`
    StashApply {
        name: String,
    },
    /// `git stash pop <name>`
    StashPop {
        name: String,
    },
    /// `git stash drop <name>`
    StashDrop {
        name: String,
    },
    /// `git fetch <remote>`
    Fetch {
        remote: String,
    },
    /// `git pull <remote> <branch>`
    Pull {
        remote: String,
        branch: String,
    },
    /// `git push <remote> <branch>`
    Push {
        remote: String,
        branch: String,
    },
    /// `git push <remote> <name>`
    PushTag {
        remote: String,
        name: String,
    },
}

/// The mode of `git reset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

impl ResetMode {
    fn as_arg(&self) -> &'static str {
        match self {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            ResetMode::Soft => "soft",
            ResetMode::Mixed => "mixed",
            ResetMode::Hard => "hard",
        }
    }
}

impl GitAction {
    /// The arguments of the git command for this action, or `None` when the action is not a git
    /// command.
    ///
    /// The arguments are passed to git as they are, one by one, so names are never interpreted by
    /// a shell.
    pub fn git_args(&self) -> Option<Vec<String>> {
        let args: Vec<&str> = match self {
            GitAction::CopyCommitHash { .. }
            | GitAction::ShowCommitDetail
            | GitAction::RunUserCommand { .. } => return None,
            GitAction::CreateBranch { name, target } => vec!["switch", "-c", name, target.as_str()],
            GitAction::SwitchBranch { name } => vec!["switch", name],
            GitAction::SwitchTrackingBranch { name, start_point } => {
                vec!["switch", "-c", name, "--track", start_point]
            }
            GitAction::MergeBranch { name } => vec!["merge", name],
            GitAction::RebaseBranch { name } => vec!["rebase", name],
            GitAction::RenameBranch { old_name, new_name } => {
                vec!["branch", "-m", old_name, new_name]
            }
            GitAction::DeleteBranch { name, force } => {
                vec!["branch", if *force { "-D" } else { "-d" }, name]
            }
            GitAction::CheckoutDetached { commit_hash } => {
                vec!["switch", "--detach", commit_hash.as_str()]
            }
            GitAction::CherryPick { commit_hash } => vec!["cherry-pick", commit_hash.as_str()],
            GitAction::Revert { commit_hash } => {
                vec!["revert", "--no-edit", commit_hash.as_str()]
            }
            GitAction::ResetCommit { commit_hash, mode } => {
                vec!["reset", mode.as_arg(), commit_hash.as_str()]
            }
            GitAction::CreateTag { name, target } => vec!["tag", name, target.as_str()],
            GitAction::DeleteTag { name } => vec!["tag", "-d", name],
            GitAction::StashApply { name } => vec!["stash", "apply", name],
            GitAction::StashPop { name } => vec!["stash", "pop", name],
            GitAction::StashDrop { name } => vec!["stash", "drop", name],
            GitAction::Fetch { remote } => vec!["fetch", remote],
            GitAction::Pull { remote, branch } => vec!["pull", remote, branch],
            GitAction::Push { remote, branch } => vec!["push", remote, branch],
            GitAction::PushTag { remote, name } => vec!["push", remote, name],
        };
        Some(args.into_iter().map(String::from).collect())
    }

    /// Whether the action is destructive and should be confirmed by the user.
    ///
    /// Destructive actions rewrite history or delete something that cannot be restored easily:
    /// deleting branches and tags, dropping stashes, rebasing and hard resets. Merges, reverts and
    /// cherry-picks create new commits (they can be undone), so they are not confirmed.
    pub fn is_destructive(&self) -> bool {
        match self {
            GitAction::DeleteBranch { .. }
            | GitAction::DeleteTag { .. }
            | GitAction::StashDrop { .. }
            | GitAction::RebaseBranch { .. } => true,
            GitAction::ResetCommit { mode, .. } => matches!(mode, ResetMode::Hard),
            _ => false,
        }
    }

    /// Whether the action runs a command that talks to a remote repository, and should therefore
    /// be executed in the background.
    pub fn is_remote(&self) -> bool {
        matches!(
            self,
            GitAction::Fetch { .. }
                | GitAction::Pull { .. }
                | GitAction::Push { .. }
                | GitAction::PushTag { .. }
        )
    }

    /// The message shown while the action is running in the background.
    pub fn progress_message(&self) -> String {
        match self {
            GitAction::Fetch { remote } => format!("Fetching from '{remote}'..."),
            GitAction::Pull { remote, branch } => {
                format!("Pulling '{branch}' from '{remote}'...")
            }
            GitAction::Push { remote, branch } => {
                format!("Pushing '{branch}' to '{remote}'...")
            }
            GitAction::PushTag { remote, name } => format!("Pushing tag '{name}' to '{remote}'..."),
            _ => format!("Running {}...", self.success_summary()),
        }
    }

    /// A one line summary of the action, shown when it succeeds.
    pub fn success_summary(&self) -> String {
        match self {
            GitAction::CopyCommitHash { .. } => "Copied commit hash to clipboard".into(),
            GitAction::ShowCommitDetail => "Opened commit details".into(),
            GitAction::RunUserCommand { number } => format!("Executed user command {number}"),
            GitAction::CreateBranch { name, .. } => format!("Created branch '{name}'"),
            GitAction::SwitchBranch { name } => format!("Switched to branch '{name}'"),
            GitAction::SwitchTrackingBranch { name, start_point } => {
                format!("Switched to a new branch '{name}' tracking '{start_point}'")
            }
            GitAction::MergeBranch { name } => format!("Merged '{name}' into the current branch"),
            GitAction::RebaseBranch { name } => format!("Rebased onto '{name}'"),
            GitAction::RenameBranch { old_name, new_name } => {
                format!("Renamed branch '{old_name}' to '{new_name}'")
            }
            GitAction::DeleteBranch { name, .. } => format!("Deleted branch '{name}'"),
            GitAction::CheckoutDetached { commit_hash } => {
                format!(
                    "Checked out '{}' (detached HEAD)",
                    commit_hash.as_short_hash()
                )
            }
            GitAction::CherryPick { commit_hash } => {
                format!("Cherry-picked '{}'", commit_hash.as_short_hash())
            }
            GitAction::Revert { commit_hash } => {
                format!("Reverted '{}'", commit_hash.as_short_hash())
            }
            GitAction::ResetCommit { mode, .. } => {
                format!("Reset the current branch ({})", mode.as_str())
            }
            GitAction::CreateTag { name, .. } => format!("Created tag '{name}'"),
            GitAction::DeleteTag { name } => format!("Deleted tag '{name}'"),
            GitAction::StashApply { name } => format!("Applied '{name}'"),
            GitAction::StashPop { name } => format!("Popped '{name}'"),
            GitAction::StashDrop { name } => format!("Dropped '{name}'"),
            GitAction::Fetch { remote } => format!("Fetched from '{remote}'"),
            GitAction::Pull { branch, remote } => format!("Pulled '{branch}' from '{remote}'"),
            GitAction::Push { branch, remote } => format!("Pushed '{branch}' to '{remote}'"),
            GitAction::PushTag { name, remote } => format!("Pushed tag '{name}' to '{remote}'"),
        }
    }
}

/// The feedback shown after an action finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionFeedback {
    /// The action succeeded; the repository should be reloaded and the summary shown.
    Success {
        summary: String,
        /// The output to show in the output view, when there is something to show.
        output: Option<String>,
    },
    /// The action failed; the message is shown in the status line.
    Error { message: String },
}

/// Builds the feedback of a finished action.
///
/// The result of an action is handled the same way whether the action ran in the foreground or in
/// the background.
pub fn action_feedback(action: &GitAction, result: Result<String, String>) -> ActionFeedback {
    match result {
        Ok(output) => ActionFeedback::Success {
            summary: action.success_summary(),
            output: has_multiple_lines(&output).then_some(output),
        },
        Err(message) => ActionFeedback::Error { message },
    }
}

/// Whether the output has more than one non-empty line, in which case it is shown in the output
/// view instead of the status line.
fn has_multiple_lines(output: &str) -> bool {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
        > 1
}

/// An entry of the action menu.
///
/// Some entries need additional input (a name, a ref, a confirmation) before they become a
/// [`GitAction`], which is why they are not [`GitAction`]s themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    /// An action that can be executed as it is.
    Action(GitAction),
    /// Create a branch at the given commit (`git switch -c <name> <target>`).
    CreateBranch { target: CommitHash },
    /// Create a tag at the given commit (`git tag <name> <target>`).
    CreateTag { target: CommitHash },
    /// Rename the given branch (`git branch -m <old_name> <new_name>`).
    RenameBranch { old_name: String },
    /// Merge another ref into the current branch.
    MergeRef,
    /// Rebase the current branch onto another ref.
    RebaseRef,
}

impl MenuAction {
    /// The action to execute for the given ref, or `None` when the action does not need a ref.
    pub fn into_action_with_ref(self, name: &str) -> Option<GitAction> {
        match self {
            MenuAction::MergeRef => Some(GitAction::MergeBranch { name: name.into() }),
            MenuAction::RebaseRef => Some(GitAction::RebaseBranch { name: name.into() }),
            _ => None,
        }
    }

    /// The action to execute for the given name, or `None` when the action does not need a name.
    pub fn into_action_with_name(self, name: &str) -> Option<GitAction> {
        match self {
            MenuAction::CreateBranch { target } => Some(GitAction::CreateBranch {
                name: name.into(),
                target,
            }),
            MenuAction::CreateTag { target } => Some(GitAction::CreateTag {
                name: name.into(),
                target,
            }),
            MenuAction::RenameBranch { old_name } => Some(GitAction::RenameBranch {
                old_name,
                new_name: name.into(),
            }),
            _ => None,
        }
    }
}

/// Whether the action should be confirmed by the user before it is executed.
pub fn needs_confirmation(action: &GitAction, confirm_destructive: bool) -> bool {
    action.is_destructive() && confirm_destructive
}

/// An entry of the action menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionMenuEntry {
    Action {
        label: String,
        action: MenuAction,
    },
    /// A switch that shows or hides a column of the commit list.
    Toggle {
        label: String,
        column: CommitListColumn,
    },
    SubMenu {
        label: String,
        entries: Vec<ActionMenuEntry>,
    },
    /// A horizontal line that separates the entries around it.
    ///
    /// A separator is not an action: it is never selected and it does not run anything.
    Separator,
}

impl ActionMenuEntry {
    pub fn label(&self) -> &str {
        match self {
            ActionMenuEntry::Action { label, .. }
            | ActionMenuEntry::Toggle { label, .. }
            | ActionMenuEntry::SubMenu { label, .. } => label,
            ActionMenuEntry::Separator => "",
        }
    }

    pub fn has_submenu(&self) -> bool {
        matches!(self, ActionMenuEntry::SubMenu { .. })
    }

    /// The column that the entry shows or hides, or `None` when the entry is not a toggle.
    pub fn column_toggle(&self) -> Option<CommitListColumn> {
        match self {
            ActionMenuEntry::Toggle { column, .. } => Some(*column),
            ActionMenuEntry::Action { .. }
            | ActionMenuEntry::SubMenu { .. }
            | ActionMenuEntry::Separator => None,
        }
    }

    /// Whether the entry can be selected and run by the user.
    ///
    /// The separators are the only entries that cannot.
    pub fn is_selectable(&self) -> bool {
        !matches!(self, ActionMenuEntry::Separator)
    }
}

fn action_entry(label: impl Into<String>, action: MenuAction) -> ActionMenuEntry {
    ActionMenuEntry::Action {
        label: label.into(),
        action,
    }
}

/// The entries that show or hide the optional columns of the commit list.
///
/// They are not git actions, so they are listed whatever the state of the repository is.
fn column_toggle_entries() -> Vec<ActionMenuEntry> {
    [
        CommitListColumn::Date,
        CommitListColumn::Hash,
        CommitListColumn::Author,
    ]
    .into_iter()
    .map(|column| ActionMenuEntry::Toggle {
        label: column.label().into(),
        column,
    })
    .collect()
}

/// Builds the action menu for the given commit.
///
/// The submenu of each ref attached to the commit is built by [`ref_menu_entries`].
/// The context of the action menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionMenuContext<'a> {
    /// Whether write actions are available (they are not in a bare repository).
    pub writable: bool,
    /// The remotes of the repository, used by the remote actions.
    pub remotes: &'a [String],
    /// The name of the current branch, used by the actions that only apply to it.
    pub current_branch: Option<&'a str>,
}

impl ActionMenuContext<'_> {
    fn is_current_branch(&self, name: &str) -> bool {
        self.current_branch == Some(name)
    }
}

/// The reason why write actions cannot be performed, or `None` when they can.
///
/// The actions are disabled by the configuration, and a bare repository has no working tree to
/// write to.
pub fn write_action_error(actions_enabled: bool, is_bare: bool) -> Option<&'static str> {
    if !actions_enabled {
        Some("Actions are disabled in the configuration")
    } else if is_bare {
        Some("Actions are not available in a bare repository")
    } else {
        None
    }
}

/// The actions that switch the repository to the given ref, in the order they must be run.
///
/// Switching to a local branch or a tag is a single action. A remote branch is switched to the
/// local branch that tracks it: the local branch is created and set to track the remote branch
/// when it does not exist yet, and checked out and then pulled when it already exists, so that
/// the branch is not left behind the remote.
pub fn switch_actions_for_ref(
    r: &Ref,
    is_local_branch: impl Fn(&str) -> bool,
) -> Result<Vec<GitAction>, String> {
    match r {
        Ref::Branch { name, .. } => Ok(vec![GitAction::SwitchBranch { name: name.clone() }]),
        Ref::RemoteBranch { name, .. } => {
            let (remote, branch) = split_remote_branch(name)
                .ok_or_else(|| format!("Cannot determine the local branch that tracks '{name}'"))?;
            if is_local_branch(branch) {
                Ok(vec![
                    GitAction::SwitchBranch {
                        name: branch.into(),
                    },
                    GitAction::Pull {
                        remote: remote.into(),
                        branch: branch.into(),
                    },
                ])
            } else {
                Ok(vec![GitAction::SwitchTrackingBranch {
                    name: branch.into(),
                    start_point: name.clone(),
                }])
            }
        }
        Ref::Tag { target, .. } => Ok(vec![GitAction::CheckoutDetached {
            commit_hash: target.clone(),
        }]),
        Ref::Stash { name, .. } => Err(format!("Cannot switch to the stash '{name}'")),
    }
}

/// Splits the name of a remote branch into the name of its remote and the name of its branch.
///
/// The remote is the first component of the name, so the branch `main` of the remote `origin` is
/// `origin/main`, and the branch `feature/x` of the same remote is `origin/feature/x`.
fn split_remote_branch(remote_branch: &str) -> Option<(&str, &str)> {
    let (remote, branch) = remote_branch.split_once('/')?;
    (!branch.is_empty()).then_some((remote, branch))
}

/// The refs attached to the commit that can be merged into the current branch.
///
/// Only branches are merged this way: a tag or a stash is not a branch. The current branch is not
/// a candidate either, since merging it into itself does nothing.
fn mergeable_branch_refs<'a>(refs: &'a [Ref], ctx: &ActionMenuContext) -> Vec<&'a Ref> {
    refs.iter()
        .filter(|r| match r {
            Ref::Branch { name, .. } => !ctx.is_current_branch(name),
            Ref::RemoteBranch { .. } => true,
            Ref::Tag { .. } | Ref::Stash { .. } => false,
        })
        .collect()
}

/// The entries that merge the branch of the commit into the current branch.
///
/// The branch of the commit is merged directly, without going through the ref picker of
/// "Merge branch...": a single candidate becomes one entry that merges it, and several candidates
/// become a submenu that lists them.
fn merge_branch_menu_entries(refs: &[Ref], ctx: &ActionMenuContext) -> Vec<ActionMenuEntry> {
    let merge_entry = |r: &&Ref| {
        action_entry(
            r.name(),
            MenuAction::Action(GitAction::MergeBranch {
                name: r.name().into(),
            }),
        )
    };
    match mergeable_branch_refs(refs, ctx).as_slice() {
        [] => Vec::new(),
        [r] => vec![action_entry(
            "Merge into current branch",
            MenuAction::Action(GitAction::MergeBranch {
                name: r.name().into(),
            }),
        )],
        branches => vec![ActionMenuEntry::SubMenu {
            label: "Merge into current branch".into(),
            entries: branches.iter().map(merge_entry).collect(),
        }],
    }
}

/// Builds the action menu for the given commit.
///
/// The submenu of each ref attached to the commit is built by [`ref_menu_entries`].
pub fn commit_menu_entries(
    commit_hash: &CommitHash,
    refs: &[Ref],
    user_commands: &FxHashMap<String, UserCommand>,
    ctx: &ActionMenuContext,
) -> Vec<ActionMenuEntry> {
    let mut entries = vec![
        action_entry(
            "Copy commit hash",
            MenuAction::Action(GitAction::CopyCommitHash {
                commit_hash: commit_hash.clone(),
            }),
        ),
        action_entry(
            "Show commit details",
            MenuAction::Action(GitAction::ShowCommitDetail),
        ),
    ];

    if ctx.writable {
        entries.push(action_entry(
            "Checkout (detached)",
            MenuAction::Action(GitAction::CheckoutDetached {
                commit_hash: commit_hash.clone(),
            }),
        ));
        entries.push(action_entry(
            "Create branch...",
            MenuAction::CreateBranch {
                target: commit_hash.clone(),
            },
        ));
        entries.push(action_entry(
            "Create tag...",
            MenuAction::CreateTag {
                target: commit_hash.clone(),
            },
        ));
        entries.push(action_entry(
            "Cherry-pick",
            MenuAction::Action(GitAction::CherryPick {
                commit_hash: commit_hash.clone(),
            }),
        ));
        entries.push(action_entry(
            "Revert",
            MenuAction::Action(GitAction::Revert {
                commit_hash: commit_hash.clone(),
            }),
        ));
        entries.push(ActionMenuEntry::SubMenu {
            label: "Reset".into(),
            entries: reset_menu_entries(commit_hash),
        });
        entries.extend(merge_branch_menu_entries(refs, ctx));
        entries.push(action_entry("Merge branch...", MenuAction::MergeRef));
        entries.push(action_entry("Rebase onto...", MenuAction::RebaseRef));
    }

    for number in sorted_user_command_numbers(user_commands) {
        let name = &user_commands[&number.to_string()].name;
        entries.push(action_entry(
            format!("Run user command {number}: {name}"),
            MenuAction::Action(GitAction::RunUserCommand { number }),
        ));
    }

    for r in refs {
        let sub_entries = ref_menu_entries(r, ctx);
        if !sub_entries.is_empty() {
            entries.push(ActionMenuEntry::SubMenu {
                label: r.name().into(),
                entries: sub_entries,
            });
        }
    }

    // The column toggles are not git actions, so they are not gated by `ctx.writable`
    entries.push(ActionMenuEntry::Separator);
    entries.extend(column_toggle_entries());

    entries
}

fn reset_menu_entries(commit_hash: &CommitHash) -> Vec<ActionMenuEntry> {
    [ResetMode::Soft, ResetMode::Mixed, ResetMode::Hard]
        .into_iter()
        .map(|mode| {
            action_entry(
                format!("Reset ({})", mode.as_str()),
                MenuAction::Action(GitAction::ResetCommit {
                    commit_hash: commit_hash.clone(),
                    mode,
                }),
            )
        })
        .collect()
}

/// Builds the action menu for the given ref.
pub fn ref_menu_entries(r: &Ref, ctx: &ActionMenuContext) -> Vec<ActionMenuEntry> {
    if !ctx.writable {
        return Vec::new();
    }
    match r {
        Ref::Branch { name, .. } => {
            let mut entries = vec![
                action_entry(
                    "Checkout / Switch",
                    MenuAction::Action(GitAction::SwitchBranch { name: name.clone() }),
                ),
                action_entry(
                    "Merge into current branch",
                    MenuAction::Action(GitAction::MergeBranch { name: name.clone() }),
                ),
                action_entry(
                    "Rebase current onto this branch",
                    MenuAction::Action(GitAction::RebaseBranch { name: name.clone() }),
                ),
                action_entry(
                    "Rename...",
                    MenuAction::RenameBranch {
                        old_name: name.clone(),
                    },
                ),
                action_entry(
                    "Delete",
                    MenuAction::Action(GitAction::DeleteBranch {
                        name: name.clone(),
                        force: false,
                    }),
                ),
                action_entry(
                    "Delete (force)",
                    MenuAction::Action(GitAction::DeleteBranch {
                        name: name.clone(),
                        force: true,
                    }),
                ),
            ];
            entries.extend(remote_branch_menu_entries(name, ctx));
            entries
        }
        // Remote branches can be used as the source of merge and rebase
        Ref::RemoteBranch { name, .. } => vec![
            action_entry(
                "Merge into current branch",
                MenuAction::Action(GitAction::MergeBranch { name: name.clone() }),
            ),
            action_entry(
                "Rebase current onto this branch",
                MenuAction::Action(GitAction::RebaseBranch { name: name.clone() }),
            ),
        ],
        Ref::Tag { name, target } => {
            let mut entries = vec![
                action_entry(
                    "Checkout (detached)",
                    MenuAction::Action(GitAction::CheckoutDetached {
                        // `git switch --detach <tag>` checks out the commit the tag points to
                        commit_hash: target.clone(),
                    }),
                ),
                action_entry(
                    "Delete",
                    MenuAction::Action(GitAction::DeleteTag { name: name.clone() }),
                ),
            ];
            entries.extend(ctx.remotes.iter().map(|remote| {
                action_entry(
                    format!("Push to '{remote}'"),
                    MenuAction::Action(GitAction::PushTag {
                        remote: remote.clone(),
                        name: name.clone(),
                    }),
                )
            }));
            entries
        }
        Ref::Stash { name, .. } => vec![
            action_entry(
                "Apply",
                MenuAction::Action(GitAction::StashApply { name: name.clone() }),
            ),
            action_entry(
                "Pop",
                MenuAction::Action(GitAction::StashPop { name: name.clone() }),
            ),
            action_entry(
                "Drop",
                MenuAction::Action(GitAction::StashDrop { name: name.clone() }),
            ),
        ],
    }
}

/// The remote actions of a local branch: fetch, pull and push for each remote.
///
/// Pulling only makes sense for the branch that is checked out, so it is only offered for the
/// current branch (pulling another branch would merge it into the current branch).
fn remote_branch_menu_entries(name: &str, ctx: &ActionMenuContext) -> Vec<ActionMenuEntry> {
    let mut entries = Vec::new();
    for remote in ctx.remotes {
        entries.push(action_entry(
            format!("Fetch '{remote}'"),
            MenuAction::Action(GitAction::Fetch {
                remote: remote.clone(),
            }),
        ));
        if ctx.is_current_branch(name) {
            entries.push(action_entry(
                format!("Pull from '{remote}'"),
                MenuAction::Action(GitAction::Pull {
                    remote: remote.clone(),
                    branch: name.into(),
                }),
            ));
        }
        entries.push(action_entry(
            format!("Push to '{remote}'"),
            MenuAction::Action(GitAction::Push {
                remote: remote.clone(),
                branch: name.into(),
            }),
        ));
    }
    entries
}

fn sorted_user_command_numbers(user_commands: &FxHashMap<String, UserCommand>) -> Vec<usize> {
    let mut numbers: Vec<usize> = user_commands
        .keys()
        .filter_map(|n| n.parse::<usize>().ok())
        .collect();
    numbers.sort_unstable();
    numbers
}

/// Executes the git command of the action in the given repository.
///
/// Returns the standard output of the command.
pub fn execute_git_action(action: &GitAction, path: &Path) -> Result<String, String> {
    let Some(args) = action.git_args() else {
        return Err("the action is not a git command".into());
    };

    let mut command = Command::new("git");
    command.args(&args).current_dir(path);
    if action.is_remote() {
        // A remote command that asks for credentials would block the background task, and the
        // prompt cannot be answered while the application owns the terminal, so it is disabled.
        command.stdin(Stdio::null()).env("GIT_TERMINAL_PROMPT", "0");
    }

    let output = command
        .output()
        .map_err(|e| format!("Failed to execute 'git {}': {e}", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(error_message(&stderr, &stdout, &args));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into())
}

/// The line of the output of a failed command that describes the failure.
///
/// Git reports information like the remote it talked to before the actual failure, so the first
/// line of the output is usually not the interesting one.
fn error_message(stderr: &str, stdout: &str, args: &[String]) -> String {
    const MARKERS: [&str; 8] = [
        "error",
        "fatal",
        "rejected",
        "conflict",
        "failed",
        "cannot",
        "not fully merged",
        "would be overwritten",
    ];

    for output in [stderr, stdout] {
        let lines: Vec<&str> = output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        let line = lines.iter().find(|line| {
            let lower = line.to_lowercase();
            MARKERS.iter().any(|marker| lower.contains(marker))
        });
        if let Some(line) = line.or_else(|| lines.first()) {
            return (*line).to_string();
        }
    }

    format!("git {} failed", args.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_commands(commands: &[(usize, &str)]) -> FxHashMap<String, UserCommand> {
        commands
            .iter()
            .map(|(n, name)| {
                (
                    n.to_string(),
                    UserCommand {
                        name: (*name).into(),
                        r#type: Default::default(),
                        commands: vec!["echo".into()],
                        refresh: false,
                    },
                )
            })
            .collect()
    }

    fn commit_hash(n: u8) -> CommitHash {
        CommitHash::from(format!("{:040x}", n).as_str())
    }

    fn labels(entries: &[ActionMenuEntry]) -> Vec<String> {
        entries.iter().map(|e| e.label().to_string()).collect()
    }

    fn find_entry<'a>(entries: &'a [ActionMenuEntry], label: &str) -> &'a ActionMenuEntry {
        entries
            .iter()
            .find(|e| e.label() == label)
            .unwrap_or_else(|| panic!("no entry labeled {label}"))
    }

    #[test]
    fn test_git_args_for_commit_actions() {
        let hash = commit_hash(1);
        assert_eq!(
            GitAction::CreateBranch {
                name: "new-branch".into(),
                target: hash.clone()
            }
            .git_args(),
            Some(vec![
                "switch".into(),
                "-c".into(),
                "new-branch".into(),
                hash.as_str().into()
            ])
        );
        assert_eq!(
            GitAction::CopyCommitHash {
                commit_hash: hash.clone()
            }
            .git_args(),
            None
        );
        assert_eq!(GitAction::ShowCommitDetail.git_args(), None);
        assert_eq!(GitAction::RunUserCommand { number: 1 }.git_args(), None);
    }

    #[test]
    fn test_git_args_for_branch_actions() {
        assert_eq!(
            GitAction::SwitchBranch {
                name: "main".into()
            }
            .git_args(),
            Some(vec!["switch".into(), "main".into()])
        );
        assert_eq!(
            GitAction::MergeBranch {
                name: "feature".into()
            }
            .git_args(),
            Some(vec!["merge".into(), "feature".into()])
        );
        assert_eq!(
            GitAction::RebaseBranch {
                name: "origin/main".into()
            }
            .git_args(),
            Some(vec!["rebase".into(), "origin/main".into()])
        );
        assert_eq!(
            GitAction::RenameBranch {
                old_name: "old".into(),
                new_name: "new".into()
            }
            .git_args(),
            Some(vec![
                "branch".into(),
                "-m".into(),
                "old".into(),
                "new".into()
            ])
        );
        assert_eq!(
            GitAction::DeleteBranch {
                name: "old".into(),
                force: false
            }
            .git_args(),
            Some(vec!["branch".into(), "-d".into(), "old".into()])
        );
        assert_eq!(
            GitAction::DeleteBranch {
                name: "old".into(),
                force: true
            }
            .git_args(),
            Some(vec!["branch".into(), "-D".into(), "old".into()])
        );
        assert_eq!(
            GitAction::SwitchTrackingBranch {
                name: "main".into(),
                start_point: "origin/main".into()
            }
            .git_args(),
            Some(vec![
                "switch".into(),
                "-c".into(),
                "main".into(),
                "--track".into(),
                "origin/main".into()
            ])
        );
    }

    #[test]
    fn test_git_args_keeps_names_with_spaces_and_special_characters() {
        let hash = commit_hash(2);
        let name = "feature/foo bar $(echo pwned) `x` ; --force";
        assert_eq!(
            GitAction::CreateBranch {
                name: name.into(),
                target: hash.clone()
            }
            .git_args(),
            Some(vec![
                "switch".into(),
                "-c".into(),
                name.into(),
                hash.as_str().into()
            ])
        );
        assert_eq!(
            GitAction::SwitchBranch { name: name.into() }.git_args(),
            Some(vec!["switch".into(), name.into()])
        );
        assert_eq!(
            GitAction::DeleteBranch {
                name: "a b".into(),
                force: true
            }
            .git_args(),
            Some(vec!["branch".into(), "-D".into(), "a b".into()])
        );
    }

    #[test]
    fn test_git_args_for_commit_actions_of_the_third_phase() {
        let hash = commit_hash(1);
        assert_eq!(
            GitAction::CheckoutDetached {
                commit_hash: hash.clone()
            }
            .git_args(),
            Some(vec![
                "switch".into(),
                "--detach".into(),
                hash.as_str().into()
            ])
        );
        assert_eq!(
            GitAction::CherryPick {
                commit_hash: hash.clone()
            }
            .git_args(),
            Some(vec!["cherry-pick".into(), hash.as_str().into()])
        );
        assert_eq!(
            GitAction::Revert {
                commit_hash: hash.clone()
            }
            .git_args(),
            Some(vec![
                "revert".into(),
                "--no-edit".into(),
                hash.as_str().into()
            ])
        );
        for (mode, arg) in [
            (ResetMode::Soft, "--soft"),
            (ResetMode::Mixed, "--mixed"),
            (ResetMode::Hard, "--hard"),
        ] {
            assert_eq!(
                GitAction::ResetCommit {
                    commit_hash: hash.clone(),
                    mode,
                }
                .git_args(),
                Some(vec!["reset".into(), arg.into(), hash.as_str().into()]),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn test_git_args_for_tag_actions() {
        let hash = commit_hash(1);
        assert_eq!(
            GitAction::CreateTag {
                name: "v1.0.0".into(),
                target: hash.clone()
            }
            .git_args(),
            Some(vec!["tag".into(), "v1.0.0".into(), hash.as_str().into()])
        );
        assert_eq!(
            GitAction::DeleteTag {
                name: "v1.0.0".into()
            }
            .git_args(),
            Some(vec!["tag".into(), "-d".into(), "v1.0.0".into()])
        );
    }

    #[test]
    fn test_git_args_for_stash_actions_keeps_the_stash_ref() {
        for (name, expected) in [("stash@{0}", "apply"), ("stash@{1}", "pop")] {
            let action = if expected == "apply" {
                GitAction::StashApply { name: name.into() }
            } else {
                GitAction::StashPop { name: name.into() }
            };
            assert_eq!(
                action.git_args(),
                Some(vec!["stash".into(), expected.into(), name.into()])
            );
        }
        assert_eq!(
            GitAction::StashDrop {
                name: "stash@{12}".into()
            }
            .git_args(),
            Some(vec!["stash".into(), "drop".into(), "stash@{12}".into()])
        );
    }

    #[test]
    fn test_git_args_for_remote_actions() {
        assert_eq!(
            GitAction::Fetch {
                remote: "origin".into()
            }
            .git_args(),
            Some(vec!["fetch".into(), "origin".into()])
        );
        assert_eq!(
            GitAction::Pull {
                remote: "origin".into(),
                branch: "main".into()
            }
            .git_args(),
            Some(vec!["pull".into(), "origin".into(), "main".into()])
        );
        assert_eq!(
            GitAction::Push {
                remote: "upstream".into(),
                branch: "feature/x".into()
            }
            .git_args(),
            Some(vec!["push".into(), "upstream".into(), "feature/x".into()])
        );
        assert_eq!(
            GitAction::PushTag {
                remote: "origin".into(),
                name: "v1.0.0".into()
            }
            .git_args(),
            Some(vec!["push".into(), "origin".into(), "v1.0.0".into()])
        );
    }

    #[test]
    fn test_git_args_keeps_tag_names_with_spaces_and_special_characters() {
        let hash = commit_hash(2);
        let name = "release/1.0 $(echo pwned) `x`; --force";
        assert_eq!(
            GitAction::CreateTag {
                name: name.into(),
                target: hash.clone()
            }
            .git_args(),
            Some(vec!["tag".into(), name.into(), hash.as_str().into()])
        );
        assert_eq!(
            GitAction::DeleteTag { name: name.into() }.git_args(),
            Some(vec!["tag".into(), "-d".into(), name.into()])
        );
    }

    #[test]
    fn test_remote_actions_are_detected() {
        assert!(GitAction::Fetch {
            remote: "origin".into()
        }
        .is_remote());
        assert!(GitAction::Pull {
            remote: "origin".into(),
            branch: "main".into()
        }
        .is_remote());
        assert!(GitAction::Push {
            remote: "origin".into(),
            branch: "main".into()
        }
        .is_remote());
        assert!(GitAction::PushTag {
            remote: "origin".into(),
            name: "v1".into()
        }
        .is_remote());

        for action in [
            GitAction::SwitchBranch {
                name: "main".into(),
            },
            GitAction::MergeBranch {
                name: "main".into(),
            },
            GitAction::CherryPick {
                commit_hash: commit_hash(1),
            },
            GitAction::CopyCommitHash {
                commit_hash: commit_hash(1),
            },
            GitAction::ShowCommitDetail,
        ] {
            assert!(!action.is_remote(), "{action:?}");
        }
    }

    #[test]
    fn test_progress_message() {
        assert_eq!(
            GitAction::Push {
                remote: "origin".into(),
                branch: "main".into()
            }
            .progress_message(),
            "Pushing 'main' to 'origin'..."
        );
        assert_eq!(
            GitAction::Fetch {
                remote: "origin".into()
            }
            .progress_message(),
            "Fetching from 'origin'..."
        );
        assert_eq!(
            GitAction::Pull {
                remote: "upstream".into(),
                branch: "feature/x".into()
            }
            .progress_message(),
            "Pulling 'feature/x' from 'upstream'..."
        );
    }

    #[test]
    fn test_action_feedback_of_a_successful_action_with_multiple_lines() {
        let action = GitAction::Push {
            remote: "origin".into(),
            branch: "main".into(),
        };
        let output = "To /tmp/remote.git\n   abc1234..def5678  main -> main\n";
        assert_eq!(
            action_feedback(&action, Ok(output.into())),
            ActionFeedback::Success {
                summary: "Pushed 'main' to 'origin'".into(),
                output: Some(output.into()),
            }
        );
    }

    #[test]
    fn test_action_feedback_of_a_successful_action_with_a_single_line() {
        let action = GitAction::SwitchBranch {
            name: "main".into(),
        };
        assert_eq!(
            action_feedback(&action, Ok("Already up to date.".into())),
            ActionFeedback::Success {
                summary: "Switched to branch 'main'".into(),
                output: None,
            }
        );
        assert_eq!(
            action_feedback(&action, Ok("   \n".into())),
            ActionFeedback::Success {
                summary: "Switched to branch 'main'".into(),
                output: None,
            }
        );
    }

    #[test]
    fn test_action_feedback_of_a_failed_action() {
        let action = GitAction::Push {
            remote: "origin".into(),
            branch: "main".into(),
        };
        assert_eq!(
            action_feedback(&action, Err("error: failed to push".into())),
            ActionFeedback::Error {
                message: "error: failed to push".into()
            }
        );
    }

    #[test]
    fn test_destructive_actions_require_confirmation() {
        assert!(GitAction::DeleteBranch {
            name: "old".into(),
            force: false
        }
        .is_destructive());
        assert!(GitAction::DeleteBranch {
            name: "old".into(),
            force: true
        }
        .is_destructive());
        // rebasing rewrites the history of the current branch, so it is confirmed too
        assert!(GitAction::RebaseBranch {
            name: "main".into()
        }
        .is_destructive());
        // only the hard reset is destructive
        assert!(GitAction::ResetCommit {
            commit_hash: commit_hash(1),
            mode: ResetMode::Hard,
        }
        .is_destructive());
        assert!(GitAction::DeleteTag {
            name: "v1.0".into()
        }
        .is_destructive());
        assert!(GitAction::StashDrop {
            name: "stash@{0}".into()
        }
        .is_destructive());

        for action in [
            GitAction::SwitchBranch {
                name: "main".into(),
            },
            GitAction::MergeBranch {
                name: "main".into(),
            },
            GitAction::RenameBranch {
                old_name: "old".into(),
                new_name: "new".into(),
            },
            GitAction::CreateBranch {
                name: "new".into(),
                target: commit_hash(1),
            },
            GitAction::CreateTag {
                name: "v1.0".into(),
                target: commit_hash(1),
            },
            GitAction::CheckoutDetached {
                commit_hash: commit_hash(1),
            },
            GitAction::CherryPick {
                commit_hash: commit_hash(1),
            },
            GitAction::Revert {
                commit_hash: commit_hash(1),
            },
            GitAction::ResetCommit {
                commit_hash: commit_hash(1),
                mode: ResetMode::Soft,
            },
            GitAction::ResetCommit {
                commit_hash: commit_hash(1),
                mode: ResetMode::Mixed,
            },
            GitAction::StashApply {
                name: "stash@{0}".into(),
            },
            GitAction::StashPop {
                name: "stash@{0}".into(),
            },
            GitAction::Fetch {
                remote: "origin".into(),
            },
            GitAction::Pull {
                remote: "origin".into(),
                branch: "main".into(),
            },
            GitAction::Push {
                remote: "origin".into(),
                branch: "main".into(),
            },
            GitAction::PushTag {
                remote: "origin".into(),
                name: "v1.0".into(),
            },
            GitAction::CopyCommitHash {
                commit_hash: commit_hash(1),
            },
            GitAction::ShowCommitDetail,
            GitAction::RunUserCommand { number: 1 },
        ] {
            assert!(!action.is_destructive(), "{action:?}");
        }

        // a destructive action is confirmed only when the confirmation is enabled
        let destructive = GitAction::DeleteBranch {
            name: "old".into(),
            force: true,
        };
        assert!(needs_confirmation(&destructive, true));
        assert!(!needs_confirmation(&destructive, false));

        let not_destructive = GitAction::MergeBranch {
            name: "main".into(),
        };
        assert!(!needs_confirmation(&not_destructive, true));
        assert!(!needs_confirmation(&not_destructive, false));
    }

    #[test]
    fn test_menu_action_resolves_to_an_action() {
        assert_eq!(
            MenuAction::MergeRef.into_action_with_ref("origin/main"),
            Some(GitAction::MergeBranch {
                name: "origin/main".into()
            })
        );
        assert_eq!(
            MenuAction::RebaseRef.into_action_with_ref("main"),
            Some(GitAction::RebaseBranch {
                name: "main".into()
            })
        );
        assert_eq!(
            MenuAction::CreateBranch {
                target: commit_hash(3)
            }
            .into_action_with_name("new branch"),
            Some(GitAction::CreateBranch {
                name: "new branch".into(),
                target: commit_hash(3)
            })
        );
        assert_eq!(
            MenuAction::RenameBranch {
                old_name: "old".into()
            }
            .into_action_with_name("new"),
            Some(GitAction::RenameBranch {
                old_name: "old".into(),
                new_name: "new".into()
            })
        );
        assert_eq!(
            MenuAction::CreateTag {
                target: commit_hash(4)
            }
            .into_action_with_name("v1.0"),
            Some(GitAction::CreateTag {
                name: "v1.0".into(),
                target: commit_hash(4)
            })
        );

        // actions that need another kind of input
        assert_eq!(MenuAction::MergeRef.into_action_with_name("main"), None);
        assert_eq!(
            MenuAction::RenameBranch {
                old_name: "old".into()
            }
            .into_action_with_ref("main"),
            None
        );
    }

    fn menu_context<'a>(writable: bool, remotes: &'a [String]) -> ActionMenuContext<'a> {
        ActionMenuContext {
            writable,
            remotes,
            current_branch: None,
        }
    }

    fn menu_context_with_current_branch<'a>(
        remotes: &'a [String],
        current_branch: &'a str,
    ) -> ActionMenuContext<'a> {
        ActionMenuContext {
            writable: true,
            remotes,
            current_branch: Some(current_branch),
        }
    }

    #[test]
    fn test_commit_menu_entries() {
        let hash = commit_hash(1);
        let branch = Ref::Branch {
            name: "main".into(),
            target: hash.clone(),
        };
        let tag = Ref::Tag {
            name: "v1.0".into(),
            target: hash.clone(),
        };
        let remotes = vec!["origin".to_string()];
        let entries = commit_menu_entries(
            &hash,
            &[branch, tag],
            &user_commands(&[(1, "git diff"), (2, "echo hello")]),
            &menu_context(true, &remotes),
        );

        assert_eq!(
            labels(&entries),
            vec![
                "Copy commit hash",
                "Show commit details",
                "Checkout (detached)",
                "Create branch...",
                "Create tag...",
                "Cherry-pick",
                "Revert",
                "Reset",
                "Merge into current branch",
                "Merge branch...",
                "Rebase onto...",
                "Run user command 1: git diff",
                "Run user command 2: echo hello",
                // the refs attached to the commit
                "main",
                "v1.0",
                // the separator and the column toggles, which are not git actions
                "",
                "Date",
                "Hash",
                "Author",
            ]
        );
        assert_eq!(
            &entries[entries.len() - 4..],
            &[
                ActionMenuEntry::Separator,
                ActionMenuEntry::Toggle {
                    label: "Date".into(),
                    column: CommitListColumn::Date,
                },
                ActionMenuEntry::Toggle {
                    label: "Hash".into(),
                    column: CommitListColumn::Hash,
                },
                ActionMenuEntry::Toggle {
                    label: "Author".into(),
                    column: CommitListColumn::Author,
                },
            ],
            "the menu ends with a separator and the three column toggles"
        );
        assert!(
            entries.last().unwrap().is_selectable(),
            "the column toggles are selectable"
        );
        assert!(
            !entries[entries.len() - 4].is_selectable(),
            "the separator before the toggles cannot be selected"
        );

        let ActionMenuEntry::SubMenu {
            label,
            entries: reset_entries,
        } = find_entry(&entries, "Reset")
        else {
            panic!("the reset modes should be a submenu");
        };
        assert_eq!(label, "Reset");
        assert_eq!(
            labels(reset_entries),
            vec!["Reset (soft)", "Reset (mixed)", "Reset (hard)"]
        );
        assert_eq!(
            find_entry(reset_entries, "Reset (hard)"),
            &ActionMenuEntry::Action {
                label: "Reset (hard)".into(),
                action: MenuAction::Action(GitAction::ResetCommit {
                    commit_hash: hash.clone(),
                    mode: ResetMode::Hard,
                })
            }
        );

        assert_eq!(
            find_entry(&entries, "Copy commit hash"),
            &ActionMenuEntry::Action {
                label: "Copy commit hash".into(),
                action: MenuAction::Action(GitAction::CopyCommitHash {
                    commit_hash: hash.clone()
                })
            }
        );
        assert_eq!(
            find_entry(&entries, "Create branch..."),
            &ActionMenuEntry::Action {
                label: "Create branch...".into(),
                action: MenuAction::CreateBranch {
                    target: hash.clone()
                }
            }
        );
        assert_eq!(
            find_entry(&entries, "Create tag..."),
            &ActionMenuEntry::Action {
                label: "Create tag...".into(),
                action: MenuAction::CreateTag {
                    target: hash.clone()
                }
            }
        );
        assert_eq!(
            find_entry(&entries, "Revert"),
            &ActionMenuEntry::Action {
                label: "Revert".into(),
                action: MenuAction::Action(GitAction::Revert {
                    commit_hash: hash.clone()
                })
            }
        );

        let ActionMenuEntry::SubMenu {
            entries: branch_entries,
            ..
        } = find_entry(&entries, "main")
        else {
            panic!("the submenu of the branch should exist");
        };
        assert_eq!(
            labels(branch_entries),
            vec![
                "Checkout / Switch",
                "Merge into current branch",
                "Rebase current onto this branch",
                "Rename...",
                "Delete",
                "Delete (force)",
                "Fetch 'origin'",
                "Push to 'origin'",
            ]
        );

        let ActionMenuEntry::SubMenu {
            entries: tag_entries,
            ..
        } = find_entry(&entries, "v1.0")
        else {
            panic!("the submenu of the tag should exist");
        };
        assert_eq!(
            labels(tag_entries),
            vec!["Checkout (detached)", "Delete", "Push to 'origin'"]
        );
    }

    #[test]
    fn test_commit_menu_entries_is_read_only_when_the_repository_is_not_writable() {
        let hash = commit_hash(1);
        let branch = Ref::Branch {
            name: "main".into(),
            target: hash.clone(),
        };
        let entries = commit_menu_entries(
            &hash,
            &[branch],
            &user_commands(&[(1, "git diff")]),
            &menu_context(false, &[]),
        );

        assert_eq!(
            labels(&entries),
            vec![
                "Copy commit hash",
                "Show commit details",
                "Run user command 1: git diff",
                // the column toggles are not git actions, so they are not gated by `writable`
                "",
                "Date",
                "Hash",
                "Author",
            ]
        );
        assert_eq!(
            entries
                .iter()
                .filter_map(|entry| entry.column_toggle())
                .collect::<Vec<_>>(),
            vec![
                CommitListColumn::Date,
                CommitListColumn::Hash,
                CommitListColumn::Author
            ]
        );
    }

    #[test]
    fn test_commit_menu_entries_sorts_user_command_numbers_numerically() {
        let entries = commit_menu_entries(
            &commit_hash(1),
            &[],
            &user_commands(&[(10, "ten"), (2, "two"), (1, "one")]),
            &menu_context(true, &[]),
        );

        assert_eq!(
            labels(&entries),
            vec![
                "Copy commit hash",
                "Show commit details",
                "Checkout (detached)",
                "Create branch...",
                "Create tag...",
                "Cherry-pick",
                "Revert",
                "Reset",
                "Merge branch...",
                "Rebase onto...",
                "Run user command 1: one",
                "Run user command 2: two",
                "Run user command 10: ten",
                // the separator and the column toggles
                "",
                "Date",
                "Hash",
                "Author",
            ]
        );
    }

    #[test]
    fn test_commit_menu_entries_ignores_user_commands_without_a_number() {
        let entries = commit_menu_entries(
            &commit_hash(1),
            &[],
            &user_commands(&[(1, "one")]),
            &menu_context(true, &[]),
        );
        assert_eq!(
            labels(&entries).len(),
            11 + 4,
            "the separator and the three column toggles are added to the actions"
        );
    }

    #[test]
    fn test_commit_menu_entries_merges_the_branch_of_the_commit_into_the_current_branch() {
        let hash = commit_hash(1);
        let branch = Ref::Branch {
            name: "feature/x".into(),
            target: hash,
        };
        let entries = commit_menu_entries(
            &commit_hash(1),
            &[branch],
            &user_commands(&[]),
            &menu_context_with_current_branch(&[], "main"),
        );

        // A single branch is merged by one entry, without going through the ref picker
        assert_eq!(
            find_entry(&entries, "Merge into current branch"),
            &ActionMenuEntry::Action {
                label: "Merge into current branch".into(),
                action: MenuAction::Action(GitAction::MergeBranch {
                    name: "feature/x".into(),
                }),
            }
        );
    }

    #[test]
    fn test_commit_menu_entries_lists_the_branches_of_the_commit_when_several_can_be_merged() {
        let hash = commit_hash(1);
        let local = Ref::Branch {
            name: "feature/x".into(),
            target: hash.clone(),
        };
        let other = Ref::Branch {
            name: "feature/y".into(),
            target: hash.clone(),
        };
        let remote = Ref::RemoteBranch {
            name: "origin/feature/z".into(),
            target: hash.clone(),
        };
        let tag = Ref::Tag {
            name: "v1.0".into(),
            target: hash,
        };
        let entries = commit_menu_entries(
            &commit_hash(1),
            &[local, other, remote, tag],
            &user_commands(&[]),
            &menu_context_with_current_branch(&[], "main"),
        );

        // Several branches are listed in a submenu: the tags are not merged with them
        let ActionMenuEntry::SubMenu {
            label,
            entries: branch_entries,
        } = find_entry(&entries, "Merge into current branch")
        else {
            panic!("the merge of several branches should be a submenu");
        };
        assert_eq!(label, "Merge into current branch");
        assert_eq!(
            labels(branch_entries),
            vec!["feature/x", "feature/y", "origin/feature/z"]
        );
        assert_eq!(
            find_entry(branch_entries, "origin/feature/z"),
            &ActionMenuEntry::Action {
                label: "origin/feature/z".into(),
                action: MenuAction::Action(GitAction::MergeBranch {
                    name: "origin/feature/z".into(),
                }),
            }
        );
    }

    #[test]
    fn test_commit_menu_entries_does_not_merge_the_current_branch_into_itself() {
        let hash = commit_hash(1);
        let current = Ref::Branch {
            name: "main".into(),
            target: hash.clone(),
        };
        let tag = Ref::Tag {
            name: "v1.0".into(),
            target: hash,
        };
        let entries = commit_menu_entries(
            &commit_hash(1),
            &[current, tag],
            &user_commands(&[]),
            &menu_context_with_current_branch(&[], "main"),
        );

        // The current branch is the target of the merge, so it is not offered as its source, and
        // a tag is not a branch: there is nothing left to merge
        assert!(labels(&entries)
            .iter()
            .all(|label| label != "Merge into current branch"));
    }

    #[test]
    fn test_ref_menu_entries_for_a_branch() {
        let r = Ref::Branch {
            name: "feature/x".into(),
            target: commit_hash(1),
        };
        let entries = ref_menu_entries(&r, &menu_context(true, &[]));
        assert_eq!(
            labels(&entries),
            vec![
                "Checkout / Switch",
                "Merge into current branch",
                "Rebase current onto this branch",
                "Rename...",
                "Delete",
                "Delete (force)",
            ]
        );
        assert!(
            entries
                .iter()
                .all(|entry| entry.column_toggle().is_none() && entry.is_selectable()),
            "the column toggles are only in the menu of a commit"
        );
    }

    #[test]
    fn test_ref_menu_entries_for_the_current_branch_with_remotes() {
        let r = Ref::Branch {
            name: "main".into(),
            target: commit_hash(1),
        };
        let remotes = vec!["origin".to_string(), "upstream".to_string()];
        let entries = ref_menu_entries(&r, &menu_context_with_current_branch(&remotes, "main"));

        assert_eq!(
            labels(&entries),
            vec![
                "Checkout / Switch",
                "Merge into current branch",
                "Rebase current onto this branch",
                "Rename...",
                "Delete",
                "Delete (force)",
                "Fetch 'origin'",
                "Pull from 'origin'",
                "Push to 'origin'",
                "Fetch 'upstream'",
                "Pull from 'upstream'",
                "Push to 'upstream'",
            ]
        );
        assert_eq!(
            find_entry(&entries, "Push to 'origin'"),
            &ActionMenuEntry::Action {
                label: "Push to 'origin'".into(),
                action: MenuAction::Action(GitAction::Push {
                    remote: "origin".into(),
                    branch: "main".into(),
                })
            }
        );
    }

    #[test]
    fn test_ref_menu_entries_does_not_pull_a_branch_that_is_not_checked_out() {
        let r = Ref::Branch {
            name: "feature/x".into(),
            target: commit_hash(1),
        };
        let remotes = vec!["origin".to_string()];
        let entries = ref_menu_entries(&r, &menu_context_with_current_branch(&remotes, "main"));

        let labels = labels(&entries);
        assert!(labels.contains(&"Fetch 'origin'".to_string()));
        assert!(labels.contains(&"Push to 'origin'".to_string()));
        assert!(!labels.contains(&"Pull from 'origin'".to_string()));
    }

    #[test]
    fn test_ref_menu_entries_for_a_remote_branch() {
        let r = Ref::RemoteBranch {
            name: "origin/main".into(),
            target: commit_hash(1),
        };
        let entries = ref_menu_entries(&r, &menu_context(true, &[]));
        assert_eq!(
            labels(&entries),
            vec![
                "Merge into current branch",
                "Rebase current onto this branch"
            ]
        );
        assert_eq!(
            find_entry(&entries, "Merge into current branch"),
            &ActionMenuEntry::Action {
                label: "Merge into current branch".into(),
                action: MenuAction::Action(GitAction::MergeBranch {
                    name: "origin/main".into()
                })
            }
        );
    }

    #[test]
    fn test_ref_menu_entries_for_a_tag() {
        let r = Ref::Tag {
            name: "v1.0".into(),
            target: commit_hash(9),
        };
        let tag_target = commit_hash(9);
        let remotes = vec!["origin".to_string()];
        let entries = ref_menu_entries(&r, &menu_context(true, &remotes));

        assert_eq!(
            labels(&entries),
            vec!["Checkout (detached)", "Delete", "Push to 'origin'"]
        );
        // the tag is checked out at the commit it points to
        assert_eq!(
            find_entry(&entries, "Checkout (detached)"),
            &ActionMenuEntry::Action {
                label: "Checkout (detached)".into(),
                action: MenuAction::Action(GitAction::CheckoutDetached {
                    commit_hash: tag_target
                })
            }
        );
        assert_eq!(
            find_entry(&entries, "Push to 'origin'"),
            &ActionMenuEntry::Action {
                label: "Push to 'origin'".into(),
                action: MenuAction::Action(GitAction::PushTag {
                    remote: "origin".into(),
                    name: "v1.0".into(),
                })
            }
        );
    }

    #[test]
    fn test_ref_menu_entries_for_a_stash() {
        let r = Ref::Stash {
            name: "stash@{0}".into(),
            message: "WIP".into(),
            target: commit_hash(1),
        };
        let entries = ref_menu_entries(&r, &menu_context(true, &[]));

        assert_eq!(labels(&entries), vec!["Apply", "Pop", "Drop"]);
        assert_eq!(
            find_entry(&entries, "Apply"),
            &ActionMenuEntry::Action {
                label: "Apply".into(),
                action: MenuAction::Action(GitAction::StashApply {
                    name: "stash@{0}".into()
                })
            }
        );
    }

    #[test]
    fn test_ref_menu_entries_is_empty_when_not_writable() {
        let branch = Ref::Branch {
            name: "main".into(),
            target: commit_hash(1),
        };
        let tag = Ref::Tag {
            name: "v1.0".into(),
            target: commit_hash(1),
        };
        let stash = Ref::Stash {
            name: "stash@{0}".into(),
            message: "WIP".into(),
            target: commit_hash(1),
        };
        let context = menu_context(false, &[]);
        assert!(ref_menu_entries(&branch, &context).is_empty());
        assert!(ref_menu_entries(&tag, &context).is_empty());
        assert!(ref_menu_entries(&stash, &context).is_empty());
    }

    #[test]
    fn test_switch_actions_for_a_branch_checks_it_out() {
        let branch = Ref::Branch {
            name: "feature/x".into(),
            target: commit_hash(1),
        };
        let actions = switch_actions_for_ref(&branch, |_| false).unwrap();
        assert_eq!(
            actions,
            vec![GitAction::SwitchBranch {
                name: "feature/x".into()
            }]
        );
        assert_eq!(
            actions[0].git_args(),
            Some(vec!["switch".into(), "feature/x".into()])
        );
    }

    #[test]
    fn test_switch_actions_for_a_remote_branch_without_a_local_branch_creates_a_tracking_branch() {
        let remote_branch = Ref::RemoteBranch {
            name: "origin/main".into(),
            target: commit_hash(1),
        };
        let actions = switch_actions_for_ref(&remote_branch, |_| false).unwrap();
        assert_eq!(
            actions,
            vec![GitAction::SwitchTrackingBranch {
                name: "main".into(),
                start_point: "origin/main".into(),
            }]
        );
        assert_eq!(
            actions[0].git_args(),
            Some(vec![
                "switch".into(),
                "-c".into(),
                "main".into(),
                "--track".into(),
                "origin/main".into()
            ])
        );
    }

    #[test]
    fn test_switch_actions_for_a_remote_branch_with_a_local_branch_switches_and_pulls() {
        let remote_branch = Ref::RemoteBranch {
            name: "origin/main".into(),
            target: commit_hash(1),
        };
        let actions = switch_actions_for_ref(&remote_branch, |name| name == "main").unwrap();
        assert_eq!(
            actions,
            vec![
                GitAction::SwitchBranch {
                    name: "main".into()
                },
                GitAction::Pull {
                    remote: "origin".into(),
                    branch: "main".into(),
                },
            ]
        );
        // The existing local branch is checked out first, and then updated from the remote
        assert_eq!(
            actions[0].git_args(),
            Some(vec!["switch".into(), "main".into()])
        );
        assert_eq!(
            actions[1].git_args(),
            Some(vec!["pull".into(), "origin".into(), "main".into()])
        );
        assert!(actions[1].is_remote());
    }

    #[test]
    fn test_switch_actions_for_a_remote_branch_with_a_slash_in_its_name() {
        let remote_branch = Ref::RemoteBranch {
            name: "upstream/feature/x".into(),
            target: commit_hash(1),
        };
        let actions = switch_actions_for_ref(&remote_branch, |_| false).unwrap();
        assert_eq!(
            actions,
            vec![GitAction::SwitchTrackingBranch {
                name: "feature/x".into(),
                start_point: "upstream/feature/x".into(),
            }]
        );
        // The local branch that is looked up is the one without the remote
        let actions = switch_actions_for_ref(&remote_branch, |name| name == "feature/x").unwrap();
        assert_eq!(
            actions,
            vec![
                GitAction::SwitchBranch {
                    name: "feature/x".into()
                },
                GitAction::Pull {
                    remote: "upstream".into(),
                    branch: "feature/x".into(),
                },
            ]
        );
    }

    #[test]
    fn test_switch_actions_for_a_remote_branch_without_a_local_name() {
        let remote_branch = Ref::RemoteBranch {
            name: "origin".into(),
            target: commit_hash(1),
        };
        assert!(switch_actions_for_ref(&remote_branch, |_| false).is_err());
    }

    #[test]
    fn test_switch_actions_for_a_tag_checks_the_commit_out_detached() {
        let tag = Ref::Tag {
            name: "v1.0".into(),
            target: commit_hash(3),
        };
        let actions = switch_actions_for_ref(&tag, |_| false).unwrap();
        assert_eq!(
            actions,
            vec![GitAction::CheckoutDetached {
                commit_hash: commit_hash(3)
            }]
        );
        assert_eq!(
            actions[0].git_args(),
            Some(vec![
                "switch".into(),
                "--detach".into(),
                commit_hash(3).as_str().into()
            ])
        );
    }

    #[test]
    fn test_switch_actions_for_a_stash_are_not_possible() {
        let stash = Ref::Stash {
            name: "stash@{0}".into(),
            message: "WIP".into(),
            target: commit_hash(1),
        };
        assert!(switch_actions_for_ref(&stash, |_| false).is_err());
    }

    #[test]
    fn test_write_action_error_is_none_only_when_the_actions_are_available() {
        assert_eq!(write_action_error(true, false), None);
        assert!(write_action_error(false, false).is_some());
        assert!(write_action_error(true, true).is_some());
        assert!(write_action_error(false, true).is_some());
    }

    #[test]
    fn test_error_message_prefers_the_failing_line() {
        // `git pull` reports the remote before the failure
        let stderr = "From /tmp/remote.git\n * branch            main       -> FETCH_HEAD\nhint: You have divergent branches...\nfatal: Need to specify how to reconcile divergent branches.\n";
        assert_eq!(
            error_message(stderr, "", &["pull".into(), "origin".into(), "main".into()]),
            "fatal: Need to specify how to reconcile divergent branches."
        );

        // a rejected push reports the rejection before the error summary
        let stderr = "To /tmp/remote.git\n ! [rejected]        main -> main (non-fast-forward)\nerror: failed to push some refs to '/tmp/remote.git'\n";
        assert_eq!(
            error_message(stderr, "", &["push".into()]),
            "! [rejected]        main -> main (non-fast-forward)"
        );

        // a simple error
        assert_eq!(
            error_message(
                "error: the branch 'unmerged' is not fully merged\n",
                "",
                &["branch".into()]
            ),
            "error: the branch 'unmerged' is not fully merged"
        );

        // the standard output is used when the standard error is empty
        assert_eq!(
            error_message("", "something went wrong\n", &["push".into()]),
            "something went wrong"
        );

        // no output at all
        assert_eq!(
            error_message("", "  \n", &["push".into(), "origin".into()]),
            "git push origin failed"
        );
    }

    #[test]
    fn test_execute_git_action_returns_the_output_of_a_git_command() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        run_git(path, &["init", "-q", "-b", "main"]);
        run_git(path, &["commit", "-q", "--allow-empty", "-m", "first"]);

        let output = execute_git_action(
            &GitAction::CreateBranch {
                name: "new-branch".into(),
                target: CommitHash::from("HEAD"),
            },
            path,
        )
        .unwrap();
        assert!(output.is_empty());

        let branches = run_git(path, &["branch", "--list", "--format=%(refname:short)"]);
        assert_eq!(branches.trim(), "main\nnew-branch");
    }

    #[test]
    fn test_execute_git_action_reports_the_error_of_a_failed_git_command() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        run_git(path, &["init", "-q", "-b", "main"]);
        run_git(path, &["commit", "-q", "--allow-empty", "-m", "first"]);

        let err = execute_git_action(
            &GitAction::DeleteBranch {
                name: "does-not-exist".into(),
                force: false,
            },
            path,
        )
        .unwrap_err();
        assert!(err.contains("does-not-exist"), "{err}");
    }

    #[test]
    fn test_execute_git_action_rejects_actions_that_are_not_git_commands() {
        let dir = tempfile::tempdir().unwrap();
        let err = execute_git_action(&GitAction::ShowCommitDetail, dir.path()).unwrap_err();
        assert_eq!(err, "the action is not a git command");
    }

    #[test]
    fn test_success_summary() {
        assert_eq!(
            GitAction::SwitchBranch {
                name: "main".into()
            }
            .success_summary(),
            "Switched to branch 'main'"
        );
        assert_eq!(
            GitAction::DeleteBranch {
                name: "feature".into(),
                force: true
            }
            .success_summary(),
            "Deleted branch 'feature'"
        );
    }

    fn run_git(path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into()
    }
}
