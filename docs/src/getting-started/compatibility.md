# Compatibility

## Rendering modes

Serie renders the commit graph in one of the following ways:

- [Inline Images Protocol (iTerm2)](https://iterm2.com/documentation-images.html)
- [Terminal graphics protocol (kitty)](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
  - Supports both the existing graphics protocol mode and [the Unicode placeholder](https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders) mode.
- ASCII graph
  - Unicode box-drawing characters (`● │ ─ ╭ ╮ ╰ ╯`), colored with the graph colors. This is the fallback that every terminal can display, and it is used automatically when the terminal cannot be identified as one that supports an image protocol.

The terminals on which each has been confirmed to work are listed below.

### Inline Images Protocol

| Terminal emulator                                                                   | Note                                                                                                                                         |
| ----------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| [iTerm2](https://iterm2.com)                                                        | But slower than other terminals                                                                                                              |
| [WezTerm](https://wezfurlong.org/wezterm/)                                          |                                                                                                                                              |
| [Rio](https://rioterm.com)                                                          |                                                                                                                                              |
| [VSCode integrated terminal](https://code.visualstudio.com/docs/terminal/basics) \* | Requires the [`terminal.integrated.enableImages` setting](https://code.visualstudio.com/docs/terminal/advanced#_image-support) to be enabled |

\*Not only the VSCode integrated terminal, but any terminal emulator using [xterm.js](https://xtermjs.org) may basically work in the same way as long as [image display feature is enabled](https://github.com/xtermjs/xterm.js/tree/master/addons/addon-image).

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

It is selected automatically when `auto` cannot identify a terminal that supports an image protocol, and it can also be selected explicitly with `--protocol ascii`.

### How `auto` picks a mode

1. Explicitly specifying `--protocol` or the `protocol` config option always takes precedence.
2. Inside tmux, the ASCII graph is used.
   - Image protocols have to be forwarded by tmux with `allow-passthrough`, and whether it is enabled cannot be detected (see [Partially supported environments](#partially-supported-environments)).
3. Otherwise, the kitty graphics protocol is used in terminals that are detected as supporting it (kitty, Ghostty).
4. Otherwise, the iTerm2 inline images protocol is used in terminals that are detected as supporting it (iTerm2, WezTerm, VSCode, mintty, ...).
5. Otherwise, the ASCII graph is used.

### Partially supported environments

- tmux uses the ASCII graph by default.
  - To use the image graph under tmux, tmux must forward the image escapes to the outer terminal: set `set -g allow-passthrough on` in tmux.conf (version 3.2+), and specify `--protocol kitty-unicode` (or `protocol = "kitty-unicode"` in the config). The outer terminal must support the kitty graphics protocol (kitty, Ghostty).
  - The iTerm2 inline images protocol cannot be used under tmux.

### Unsupported environments

- Sixel graphics is not supported.
- Other terminal multiplexers (screen, Zellij, etc.) cannot display the image graph, and use the ASCII graph instead.
- Windows is not officially supported. Please refer to [the related issue](https://github.com/lusingander/serie/issues/147#issuecomment-4192875627).
