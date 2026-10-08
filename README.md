# Serie

[![Crate Status](https://img.shields.io/crates/v/serie.svg)](https://crates.io/crates/serie)
[![Built With Ratatui](https://img.shields.io/badge/Built_With-Ratatui-000?logo=ratatui&logoColor=fff&labelColor=000&color=fff)](https://ratatui.rs)

A rich git commit graph in your terminal, like magic 📚

<img src="./img/demo.gif">

(This demo shows [Ratatui](https://github.com/ratatui/ratatui) repository!)

## About

Serie ([`/zéːriə/`](https://lusingander.github.io/serie/faq/index.html#how-do-i-pronounce-serie)) is a TUI application that renders commit graphs like `git log --graph --all`, using the terminal emulators' image display protocol for high-quality output where it is available, and a Unicode box-drawing fallback in any other terminal.

### Why?

While some users prefer to use Git via CLI, they often rely on a GUI or feature-rich TUI to view commit logs. Others may find `git log --graph` sufficient.

Personally, I found the output from `git log --graph` difficult to read, even with additional options. Learning complex tools just to view logs seemed cumbersome.

### Goals

- Provide a rich `git log --graph` experience in the terminal.
- Offer commit graph-centric browsing of Git repositories.

### Non-Goals

- Implement a fully-featured Git client.
- Create a TUI application with a complex UI.
- Render the graph with the same quality everywhere: terminals without an image display protocol use the simpler Unicode fallback.

## Documentation

For detailed usage, configuration, and advanced features, see [the full documentation](https://lusingander.github.io/serie/).

## Requirements

- Git
- A terminal emulator
  - Terminals with an image display protocol get the high-quality graph, and any other terminal gets the Unicode box-drawing fallback.
  - Refer to [Compatibility](https://lusingander.github.io/serie/getting-started/compatibility.html) for details.

## Installation

If you're using Cargo:

```
$ cargo install --locked serie
```

For other download options, see [Installation](https://lusingander.github.io/serie/getting-started/installation.html).

## Usage

### Basic

Run `serie` in the directory where your git repository exists.

```
$ cd <your git repository>
$ serie
```

### Options

```
Serie - A rich git commit graph in your terminal, like magic 📚

Usage: serie [OPTIONS]

Options:
  -n, --max-count <NUMBER>        Maximum number of commits to render
  -p, --protocol <TYPE>           Image protocol to render graph [default: auto] [possible values: auto, iterm, kitty, kitty-unicode, ascii]
  -o, --order <TYPE>              Commit ordering algorithm [default: chrono] [possible values: chrono, topo]
  -g, --graph-width <TYPE>        Commit graph image cell width [default: auto] [possible values: auto, double, single]
  -s, --graph-style <TYPE>        Commit graph image edge style [default: rounded] [possible values: rounded, angular]
  -i, --initial-selection <TYPE>  Initial selection of commit [default: latest] [possible values: latest, head]
  -b, --primary-branch <BRANCH>   Primary branch to keep on the leftmost column
  -h, --help                      Print help
  -V, --version                   Print version
```

For details on each option, see [Command Line Options](https://lusingander.github.io/serie/getting-started/command-line-options.html).

### Keybindings

You can see the keybindings by pressing the `?` key.

The [default key bindings](https://lusingander.github.io/serie/keybindings/index.html) can be overridden. See [Custom Keybindings](https://lusingander.github.io/serie/keybindings/custom-keybindings.html) for more information.

### Mouse

The commit list can also be operated with the mouse (enabled by default, see `ui.common.mouse`):

- Left click on a commit: select the commit
- Double click on a ref badge: switch to that ref (a remote branch is checked out as its local branch and pulled, or created as a tracking branch when the local branch does not exist)
- Hover over a commit: show the `…` more button at the right edge of that row; rest the cursor on it, or left click it, to open the action menu for that commit
- Left click on the `⇡` segment of the status bar: push the current branch to the remote
- Wheel up/down: scroll the commit list by one row per event
- Drag the scrollbar thumb, or left click on the scrollbar track: scroll the commit list

The refs of a commit are shown as reversed badges in front of its subject, with the local branches first, then the remote branches, and the tags last. The branch that HEAD points to is marked with a `●` right in front of its badge; when HEAD is detached, the `●` marks the commit itself.

Scrolling with the wheel or the scrollbar does not change the selected commit, in the same way as <kbd>Ctrl-e</kbd>/<kbd>Ctrl-y</kbd>. Note that the selected commit can be scrolled out of the screen, in which case no row is highlighted.

While the mouse is captured by the application, hold `Shift` to select and copy text with your terminal. Set `ui.common.mouse = false` to disable mouse operations and always use the terminal native mouse behavior.

### Auto refresh

The commit graph is refreshed automatically (enabled by default, see `ui.common.auto_refresh`) when the repository is changed by another process, such as a commit, a fetch, or a branch switch in another terminal, so there is no need to reload it by hand.

The refresh is postponed while you are entering a search query, while a menu, a dialog or the help screen is open, while the output of a user command is shown, and while a git action is running, and it is performed as soon as the application is idle again. It keeps the selected commit. Set `ui.common.auto_refresh = false` to disable watching the repository; the graph can still be refreshed with <kbd>R</kbd>.

### Status bar

The last line of the screen shows the state of the repository and the operations that can be run on the current branch:

- The name of the branch that is checked out, or `detached <short hash>` when HEAD is detached
- `↑1` / `↓2`: the number of commits the branch is ahead of and behind its upstream. Each of them is shown only when it is not zero, and neither is shown when the branch does not track an upstream
- `⇡ origin`: the push of the current branch, which is run by clicking it with the left mouse button

The branch is pushed to the remote of its upstream. A branch that does not track an upstream is pushed to the only remote of the repository, and a menu that lists the remotes is shown when there are several. The push runs in the background like the remote actions of the action menu, and the status bar is reloaded when it finishes. Nothing is pushed from the status bar when the repository has no remote, or when the actions are disabled with `actions.enable = false`.

The colors of the bar can be changed with `color.status_bar_fg` and `color.status_bar_bg`.

### Actions

Pressing <kbd>x</kbd> (or right clicking the commit list) opens the action menu for the selected commit or ref, where common git operations (create/checkout/merge/rebase/rename/delete branches, copying the commit hash, running user commands, ...) can be executed without leaving the application.

Actions cover the common commit, branch, tag and stash operations (including checkout, merge, rebase, cherry-pick, revert, reset, and pushing/pulling/fetching from a remote), and the destructive ones are confirmed first. Remote operations run in the background, so the application stays responsive while they are in progress. The whole feature can be disabled with `actions.enable = false`.

### Config

Config files are loaded in the following order of priority:

- `$SERIE_CONFIG_FILE`
  - If `$SERIE_CONFIG_FILE` is set but the file does not exist, an error occurs.
- `$XDG_CONFIG_HOME/serie/config.toml`
  - If `$XDG_CONFIG_HOME` is not set, `~/.config/` will be used instead.

If the config file does not exist, the default values will be used for all items.
If the config file exists but some items are not set, the default values will be used for those unset items.

For detailed information about the config file format, see [Config File Format](https://lusingander.github.io/serie/configurations/config-file-format.html).

### User command

The User command feature allows you to execute custom external commands.
You can display the output of commands like `git diff` in a dedicated view, execute commands like branch deletion in the background, or run interactive commands like `vim` by suspending the application.

For details on how to set commands, see [User Command](https://lusingander.github.io/serie/features/user-command.html).

## Compatibility

### Supported terminals

These rendering modes are supported:

- [Inline Images Protocol (iTerm2)](https://iterm2.com/documentation-images.html)
- [Terminal graphics protocol (kitty)](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
  - Supports both the existing graphics protocol mode and the [Unicode placeholder](https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders) mode.
- ASCII graph (Unicode box-drawing characters, works in any terminal)
  - Used as the fallback when `auto` cannot identify a terminal that supports an image protocol, or selected explicitly with `--protocol ascii`.

The mode can also be forced with the `$SERIE_PROTOCOL` environment variable (e.g. `SERIE_PROTOCOL=ascii`), which is useful when the automatic detection does not fit the environment (ssh sessions, unknown terminal emulators, ...).
The `--protocol` option takes precedence over it, and it takes precedence over the `protocol` config option.

For more information, see [Compatibility](https://lusingander.github.io/serie/getting-started/compatibility.html).

### Partially supported environments

- tmux uses the ASCII graph by default, since whether tmux forwards the image escapes to the outer terminal cannot be detected.
  - To use the high-quality image graph inside tmux, enable `set -g allow-passthrough on` in tmux.conf (version 3.2+) and specify `--protocol kitty-unicode` (or `protocol = "kitty-unicode"` in the config) with a terminal emulator that supports the kitty graphics protocol.
- Only the kitty graphics protocol works under tmux; the iTerm2 inline images protocol does not.

### Unsupported environments

- Sixel graphics is not supported.
- Other terminal multiplexers (screen, Zellij, etc.) cannot display the image graph; they use the ASCII graph instead.
- Windows is not officially supported. Please refer to [the related issue](https://github.com/lusingander/serie/issues/147#issuecomment-4192875627).

## Screenshots

<img src="./img/list.png" width=600>
<img src="./img/detail.png" width=600>
<img src="./img/refs.png" width=600>
<img src="./img/searching.png" width=600>
<img src="./img/applied.png" width=600>
<img src="./img/diff_git.png" width=600>
<img src="./img/diff_difft.png" width=600>

The following repositories are used as these examples:

- [ratatui/ratatui](https://github.com/ratatui/ratatui)
- [charmbracelet/vhs](https://github.com/charmbracelet/vhs)
- [lusingander/stu](https://github.com/lusingander/stu)

## Contributing

To get started with contributing, please review [CONTRIBUTING.md](CONTRIBUTING.md).

Contributions that do not follow these guidelines may not be accepted.

## License

MIT
