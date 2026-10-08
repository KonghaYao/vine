# Keybindings

You can see the keybindings by pressing the `?` key.

The default key bindings can be overridden.

## List of all default keybindings

#### Common

| Key                            | Description | Corresponding keybind |
| ------------------------------ | ----------- | --------------------- |
| <kbd>Ctrl-c</kbd> <kbd>q</kbd> | Quit app    | `force_quit` `quit`   |
| <kbd>?</kbd> <kbd>F1</kbd>     | Open help   | `help_toggle`         |

#### Commit List

| Key                                  | Description                                        | Corresponding keybind                        |
| ------------------------------------ | -------------------------------------------------- | -------------------------------------------- |
| <kbd>Down/Up</kbd> <kbd>j/k</kbd>    | Move down/up                                       | `navigate_down` `navigate_up`                |
| <kbd>J/K</kbd>                       | Move down/up                                       | `select_down` `select_up`                    |
| <kbd>Alt-Down</kbd> <kbd>Alt-j</kbd> | Move to parent commit                              | `go_to_parent`                               |
| <kbd>g/G</kbd>                       | Go to top/bottom                                   | `go_to_top` `go_to_bottom`                   |
| <kbd>Ctrl-f/b</kbd>                  | Scroll page down/up                                | `page_down` `page_up`                        |
| <kbd>Ctrl-d/u</kbd>                  | Scroll half page down/up                           | `half_page_down` `half_page_up`              |
| <kbd>Ctrl-e/y</kbd>                  | Scroll down/up                                     | `scroll_down` `scroll_up`                    |
| <kbd>H/M/L</kbd>                     | Select top/middle/bottom of the screen             | `select_top` `select_middle` `select_bottom` |
| <kbd>Enter</kbd>                     | Show commit details<br>Apply search (if searching) | `confirm`                                    |
| <kbd>Tab</kbd>                       | Open refs list                                     | `ref_list`                                   |
| <kbd>x</kbd>                         | Open action menu                                   | `open_actions`                               |
| <kbd>/</kbd>                         | Start search                                       | `search`                                     |
| <kbd>Esc</kbd>                       | Cancel search                                      | `cancel`                                     |
| <kbd>n/N</kbd>                       | Go to next/previous search match                   | `go_to_next` `go_to_previous`                |
| <kbd>Ctrl-t</kbd>                    | Toggle search target                               | `search_target_toggle`                       |
| <kbd>Ctrl-g</kbd>                    | Toggle ignore case                                 | `ignore_case_toggle`                         |
| <kbd>Ctrl-x</kbd>                    | Toggle fuzzy match                                 | `fuzzy_toggle`                               |
| <kbd>R</kbd>                         | Refresh                                            | `refresh`                                    |
| <kbd>c/C</kbd>                       | Copy commit short/full hash                        | `short_copy` `full_copy`                     |
| <kbd>d</kbd>                         | Toggle custom user command view                    | `user_command_1`                             |

Note: <kbd>Ctrl-e/y</kbd> (the `scroll_down`/`scroll_up` keybinds) scroll the list without moving the selection, so the selected commit can be scrolled out of the screen and no row is highlighted while it is out of the screen. Press <kbd>j</kbd>/<kbd>k</kbd> (or any other key that moves the selection) to scroll it back into the screen.

Note: The commit list is also refreshed automatically when the repository is changed by another process, such as a commit or a fetch in another terminal (see `ui.common.auto_refresh`). The refresh key is only needed to refresh it by hand.

#### Mouse (Commit List)

The mouse operations below can be used while the mouse is enabled by `ui.common.mouse`.

| Operation                          | Description                                                             |
| ---------------------------------- | ----------------------------------------------------------------------- |
| Left click on a commit             | Select the commit (the viewport is not scrolled)                        |
| Double click on a ref badge        | Switch to that ref                                                      |
| Right click                        | Open the action menu for the commit under the cursor                    |
| Wheel up/down                      | Scroll the list by 3 rows                                               |
| Drag the scrollbar thumb           | Scroll the list                                                         |
| Left click on the scrollbar track  | Scroll the list to the clicked position, then continue dragging         |

Note: The refs of a commit are shown as reversed badges in front of its subject. Double clicking one of them switches to that ref: a local branch is checked out, a remote branch is checked out as the local branch that tracks it and then pulled (or created as a new tracking branch when the local branch does not exist), and a tag is checked out detached. When the switch fails, the reason is shown in the status line.

Note: Like <kbd>Ctrl-e/y</kbd>, the wheel and the scrollbar only scroll the list; they do not change the selected commit, and the selection may be scrolled out of the screen.

The scrollbar is shown on the right edge of the commit list only when the list is longer than the screen.

#### Commit Detail

| Key                                  | Description                     | Corresponding keybind           |
| ------------------------------------ | ------------------------------- | ------------------------------- |
| <kbd>Esc</kbd> <kbd>Backspace</kbd>  | Close commit details            | `close` `cancel`                |
| <kbd>Down/Up</kbd> <kbd>j/k</kbd>    | Scroll down/up                  | `navigate_down` `navigate_up`   |
| <kbd>Ctrl-f/b</kbd>                  | Scroll page down/up             | `page_down` `page_up`           |
| <kbd>Ctrl-d/u</kbd>                  | Scroll half page down/up        | `half_page_down` `half_page_up` |
| <kbd>g/G</kbd>                       | Go to top/bottom                | `go_to_top` `go_to_bottom`      |
| <kbd>J/K</kbd>                       | Select older/newer commit       | `select_down` `select_up`       |
| <kbd>Alt-Down</kbd> <kbd>Alt-j</kbd> | Select parent commit            | `go_to_parent`                  |
| <kbd>R</kbd>                         | Refresh                         | `refresh`                       |
| <kbd>c/C</kbd>                       | Copy commit short/full hash     | `short_copy` `full_copy`        |
| <kbd>d</kbd>                         | Toggle custom user command view | `user_command_1`                |

#### Refs List

| Key                                                | Description      | Corresponding keybind            |
| -------------------------------------------------- | ---------------- | -------------------------------- |
| <kbd>Esc</kbd> <kbd>Backspace</kbd> <kbd>Tab</kbd> | Close refs list  | `close` `cancel` `ref_list`      |
| <kbd>Down/Up</kbd> <kbd>j/k</kbd>                  | Move down/up     | `navigate_down` `navigate_up`    |
| <kbd>J/K</kbd>                                     | Move down/up     | `select_down` `select_up`        |
| <kbd>g/G</kbd>                                     | Go to top/bottom | `go_to_top` `go_to_bottom`       |
| <kbd>Right/Left</kbd> <kbd>l/h</kbd>               | Open/Close node  | `navigate_right` `navigate_left` |
| <kbd>R</kbd>                                       | Refresh          | `refresh`                        |
| <kbd>c</kbd>                                       | Copy ref name    | `short_copy`                     |

#### User Command

| Key                                  | Description                 | Corresponding keybind           |
| ------------------------------------ | --------------------------- | ------------------------------- |
| <kbd>Esc</kbd> <kbd>Backspace</kbd>  | Close user command          | `close` `cancel`                |
| <kbd>Down/Up</kbd> <kbd>j/k</kbd>    | Scroll down/up              | `navigate_down` `navigate_up`   |
| <kbd>J/K</kbd>                       | Scroll down/up              | `select_down` `select_up`       |
| <kbd>Ctrl-f/b</kbd>                  | Scroll page down/up         | `page_down` `page_up`           |
| <kbd>Ctrl-d/u</kbd>                  | Scroll half page down/up    | `half_page_down` `half_page_up` |
| <kbd>g/G</kbd>                       | Go to top/bottom            | `go_to_top` `go_to_bottom`      |
| <kbd>J/K</kbd>                       | Select older/newer commit   | `select_down` `select_up`       |
| <kbd>Alt-Down</kbd> <kbd>Alt-j</kbd> | Select parent commit        | `go_to_parent`                  |
| <kbd>R</kbd>                         | Refresh                     | `refresh`                       |

#### Help

| Key                                                            | Description              | Corresponding keybind           |
| -------------------------------------------------------------- | ------------------------ | ------------------------------- |
| <kbd>Esc</kbd> <kbd>Backspace</kbd> <kbd>?</kbd> <kbd>F1</kbd> | Close help               | `close` `cancel` `help_toggle`  |
| <kbd>Down/Up</kbd> <kbd>j/k</kbd>                              | Scroll down/up           | `navigate_down` `navigate_up`   |
| <kbd>J/K</kbd>                                                 | Scroll down/up           | `select_down` `select_up`       |
| <kbd>Ctrl-f/b</kbd>                                            | Scroll page down/up      | `page_down` `page_up`           |
| <kbd>Ctrl-d/u</kbd>                                            | Scroll half page down/up | `half_page_down` `half_page_up` |
| <kbd>g/G</kbd>                                                 | Go to top/bottom         | `go_to_top` `go_to_bottom`      |

</details>

#### Action menu

The action menu is opened with <kbd>x</kbd> (or a right click on the commit list) and lists the
actions available for the selected commit or ref:

- Commit: Copy commit hash, Show commit details, Checkout (detached), Create branch..., Create tag...,
  Cherry-pick, Revert, Reset (soft/mixed/hard), Merge another branch, Rebase onto another branch,
  Run user command, and the actions of the refs attached to the commit (as a submenu).
- Branch: Checkout / Switch, Merge into current branch, Rebase current onto this branch, Rename...,
  Delete, Delete (force), and the remote actions below.
- Remote branch: Merge into current branch, Rebase current onto this branch.
- Tag: Checkout (detached), Delete, Push to a remote.
- Stash: Apply, Pop, Drop.

Navigation keys (<kbd>Up/Down</kbd>, <kbd>Enter</kbd>, <kbd>Esc</kbd>) are fixed and are not
customizable.

Actions run `git` commands in the repository. Destructive actions (deleting branches and tags,
dropping stashes, rebasing, and `reset --hard`) ask for a confirmation (<kbd>y</kbd>/<kbd>n</kbd>)
when `actions.confirm_destructive` is enabled, and the whole feature can be disabled with
`actions.enable = false`.

##### Remote actions

`Fetch`, `Pull` and `Push` are listed once per remote (with no remote, they are not shown):

- `Push` pushes the selected branch with an explicit refspec
  (`git push <remote> <branch>`), so the upstream configuration is never changed.
- `Pull` is only offered for the branch that is currently checked out, because pulling another
  branch would merge it into the current branch (`git pull <remote> <branch>`).
- `Fetch` fetches all refs of the remote (`git fetch <remote>`).

Remote actions run in the background: the status line shows the operation in progress and the
application keeps responding to keys while it runs. Only one remote action can run at a time, and
its result is reported in the status line (or in the output view for multi-line output).

Terminal prompts are disabled for remote actions (a prompt cannot be answered while the application
owns the terminal), so a remote that requires credentials fails with an error instead of waiting:
configure a credential helper (or an SSH key) to push and pull without prompting.

----

- [Custom Keybindings](./custom-keybindings.md)
