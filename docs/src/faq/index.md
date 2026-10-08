# FAQ

## Why doesn't the graph display?

Serie renders graphs in one of two ways, depending on what your terminal supports:

1. **High-quality images** via the kitty graphics protocol or the iTerm2 inline images protocol, used automatically in terminals that support them.
2. **Unicode box-drawing characters** (`● │ ─ ╭ ╮ ╰ ╯`), used in any other terminal as an automatic fallback.

If you see no graph at all, the most likely cause is that `auto` picked an image protocol that your terminal does not actually support. Try `serie --protocol ascii` to force the Unicode fallback, and see [Compatibility](../getting-started/compatibility.md) for the mode each environment uses.

## What are the advantages over other git TUI clients?

Compared to other git TUI clients, Serie offers the following advantages:

- High-quality graph visualization using terminal graphics protocols (with a Unicode fallback for terminals that lack them)
- Simple and clean interface

On the other hand, Serie may not be for you if:

- You're satisfied with `git log --graph` or the graph display in existing TUI clients
- You need to perform complex git operations within a TUI client

## How do I pronounce "Serie"?

It is pronounced as the German word Serie (**/ˈzeːriə/**), roughly like **"ZAY-ree-eh"**, not like the English "series".
