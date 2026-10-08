# Compatibility

## Rendering modes

Serie renders the commit graph in one of the following ways:

- ASCII graph
  - Unicode box-drawing characters (`● ○ │ ─ ╭ ╮ ╰ ╯`), colored with the graph colors. Every terminal can display it, so it is the default: the image protocols are used only when they are selected explicitly.
- [Inline Images Protocol (iTerm2)](https://iterm2.com/documentation-images.html)
- [Terminal graphics protocol (kitty)](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
  - Supports both the existing graphics protocol mode and [the Unicode placeholder](https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders) mode.

The terminals on which each has been confirmed to work are listed below.

### Inline Images Protocol

| Terminal emulator                                                                   | Note                                                                                                                                         |
| ----------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| [iTerm2](https://iterm2.com)                                                        | But slower than other terminals                                                                                                              |
| [WezTerm](https://wezfurlong.org/wezterm/)                                          |                                                                                                                                              |
| [Rio](https://rioterm.com)                                                          |                                                                                                                                              |
| [VSCode integrated terminal](https://code.visualstudio.com/docs/terminal/basics) \* | Requires the [`terminal.integrated.enableImages` setting](https://code.visualstudio.com/docs/terminal/advanced#_image-support) to be enabled |

\*Not only the VSCode integrated terminal, but any terminal emulator using [xterm.js](https://xtermjs.org) may basically work in the same way as long as [image display feature is enabled](https://github.com/xtermjs/xterm.js/tree/master/addons/addon-image).

`auto` identifies these terminals by their environment variables (`TERM_PROGRAM`, `LC_TERMINAL`), so a terminal that supports the protocol but cannot be identified that way (Rio, and other xterm.js based terminals with a different `TERM_PROGRAM`) is not detected. Specify `iterm` as the protocol option or config to use the image graph there.

### Terminal graphics protocol

| Terminal emulator                         | Unicode placeholder | Note |
| ----------------------------------------- | ------------------- | ---- |
| [kitty](https://sw.kovidgoyal.net/kitty/) | ○                  |      |
| [Ghostty](https://ghostty.org)            | ○                  |      |

Rendering using Unicode Placeholder is available by explicitly specifying `kitty-unicode` as `protocol` option or config.

### ASCII graph

Any terminal emulator that can display Unicode box-drawing characters can render this mode, which makes the graph visible even where the image protocols are unavailable:

- terminals that do not support an image protocol (alacritty, gnome-terminal, xterm, ...)
- ssh sessions and other environments where the terminal identity cannot be detected
- terminal multiplexers such as tmux, screen, and Zellij

It is used by default, in any terminal, and it can also be selected explicitly with `--protocol ascii`. When an image protocol does not render properly in the current environment, select `ascii` again (or unset the option, the environment variable, and the config option) to go back to the default.

### Forcing a mode

The mode can be selected, in order of priority:

1. The `--protocol` command line option.
2. The `$SERIE_PROTOCOL` environment variable, e.g. `SERIE_PROTOCOL=kitty`.
   - This is useful for selecting the mode per environment (a shell on a remote host, a specific terminal, ...) without editing the config file.
3. The `protocol` config option.

### How `auto` picks a mode

`auto` is used only when it is specified explicitly with `--protocol auto`, `SERIE_PROTOCOL=auto`, or `protocol = "auto"`. It then works as follows.

1. Explicitly specifying `--protocol`, `$SERIE_PROTOCOL`, or the `protocol` config option takes precedence over it (see [Forcing a mode](#forcing-a-mode)).
2. Inside tmux, the ASCII graph is used.
   - Image protocols have to be forwarded by tmux with `allow-passthrough`, and whether it is enabled cannot be detected (see [Partially supported environments](#partially-supported-environments)).
3. Otherwise, the kitty graphics protocol is used in terminals that are detected as supporting it (kitty, Ghostty).
4. Otherwise, the iTerm2 inline images protocol is used in terminals that are detected as supporting it (iTerm2, WezTerm, VSCode, mintty, ...).
5. Otherwise, the ASCII graph is used.

### Partially supported environments

- Under tmux, `auto` selects the ASCII graph.
  - To use the image graph under tmux, tmux must forward the image escapes to the outer terminal: set `set -g allow-passthrough on` in tmux.conf (version 3.2+), and specify `--protocol kitty-unicode` (or `protocol = "kitty-unicode"` in the config). The outer terminal must support the kitty graphics protocol (kitty, Ghostty).
  - The iTerm2 inline images protocol cannot be used under tmux.

### Unsupported environments

- Sixel graphics is not supported.
- Other terminal multiplexers (screen, Zellij, etc.) cannot display the image graph; they use the default ASCII graph.
- Windows is not officially supported. Please refer to [the related issue](https://github.com/lusingander/serie/issues/147#issuecomment-4192875627).
