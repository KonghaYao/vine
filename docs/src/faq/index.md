# FAQ

## Why doesn't the graph display?

Serie renders graphs in one of two ways:

1. **Unicode box-drawing characters** (`┃ ─ ╭ ╮ ╰ ╯`), used by default, in any terminal.
2. **High-quality images** via the kitty graphics protocol or the iTerm2 inline images protocol, used only when they are selected explicitly with `--protocol kitty`, `--protocol iterm`, `--protocol auto`, and so on.

If you see no graph at all, an image protocol is selected for a terminal that does not actually support it: check the `--protocol` option, the `$SERIE_PROTOCOL` environment variable, and the `protocol` config option, and select `ascii` again (or remove all of them, since `ascii` is the default). See [Compatibility](../getting-started/compatibility.md) for the mode each environment uses.

## What are the advantages over other git TUI clients?

Compared to other git TUI clients, Serie offers the following advantages:

- High-quality graph visualization using terminal graphics protocols (with a Unicode graph that works in any terminal)
- Simple and clean interface

On the other hand, Serie may not be for you if:

- You're satisfied with `git log --graph` or the graph display in existing TUI clients
- You need to perform complex git operations within a TUI client

## How do I pronounce "Serie"?

It is pronounced as the German word Serie (**/ˈzeːriə/**), roughly like **"ZAY-ree-eh"**, not like the English "series".
