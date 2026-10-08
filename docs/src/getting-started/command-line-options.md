# Command Line Options

## -n, --max-count \<NUMBER\>

Maximum number of commits to render.

If not specified, all commits will be rendered.
It behaves similarly to the `--max-count` option of `git log`.

## -p, --protocol \<TYPE\>

The protocol used to render the commit graph.

_Possible values:_ `auto`, `iterm`, `kitty`, `kitty-unicode`, `ascii`

- `iterm`: [iTerm2 inline images protocol](https://iterm2.com/documentation-images.html)
- `kitty`: [kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
- `kitty-unicode`: kitty graphics protocol with [Unicode placeholders](https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders), which also works under tmux
- `ascii`: Unicode box-drawing characters (`● │ ─ ╭ ╮ ╰ ╯`), which works in any terminal

By default `auto` picks the best supported mode for the current terminal:

1. Inside tmux (`TMUX`, `TERM=tmux*` or `TERM_PROGRAM=tmux`), the ASCII graph is used.
   - Whether tmux forwards the image escapes to the outer terminal cannot be detected, so the always-working ASCII graph is used instead.
   - To use the image graph inside tmux, enable `set -g allow-passthrough on` in tmux.conf (version 3.2+) and specify `kitty-unicode` explicitly.
2. Otherwise, the kitty graphics protocol is used if `KITTY_WINDOW_ID`, `GHOSTTY_RESOURCES_DIR`, or `TERM=xterm-ghostty` is set.
3. Otherwise, the iTerm2 inline images protocol is used if `TERM_PROGRAM` is `iTerm.app`, `WezTerm`, `mintty`, or `vscode`, or if `LC_TERMINAL=iTerm2`.
4. Otherwise, the ASCII graph is used.

The ASCII graph follows the `--graph-style` and `--graph-width` options in the same way as the image graph.

## -o, --order \<TYPE\>

Commit ordering algorithm.

_Possible values:_ `chrono`, `topo`

`chrono` will order commits by commit date if possible.

<img src="https://raw.githubusercontent.com/lusingander/serie/master/img/order-chrono.png" width=300>

`topo` will order commits on the same branch consecutively if possible.

<img src="https://raw.githubusercontent.com/lusingander/serie/master/img/order-topo.png" width=300>

## -g, --graph-width \<TYPE\>

The character width that a graph image unit cell occupies.

_Possible values:_ `auto`, `double`, `single`

If not specified or `auto` is specified, `double` will be used automatically if there is enough width to display it, `single` otherwise.

<img src="https://raw.githubusercontent.com/lusingander/serie/master/img/graph-width-double.png" width=300>

<img src="https://raw.githubusercontent.com/lusingander/serie/master/img/graph-width-single.png" width=300>

</details>

## -s, --graph-style \<TYPE\>

The commit graph image edge style.

_Possible values:_ `rounded`, `angular`

`rounded` will use rounded edges for the graph lines.

<img src="https://raw.githubusercontent.com/lusingander/serie/master/img/graph-width-double.png" width=300>

`angular` will use angular edges for the graph lines.

<img src="https://raw.githubusercontent.com/lusingander/serie/master/img/style-angular.png" width=300>

## -i, --initial-selection \<TYPE\>

The initial selection of commit when starting the application.

_Possible values:_ `latest`, `head`

`latest` will select the latest commit.

`head` will select the commit at HEAD.

## -b, --primary-branch \<BRANCH\>

The primary branch to keep on the leftmost column.

When specified, Serie preserves the first-parent commit spine of the specified branch along the leftmost column, displaying other branches in subsequent columns even if they contain newer commits.
