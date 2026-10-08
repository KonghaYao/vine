# Introduction

**Serie** ([`/zéːriə/`](https://lusingander.github.io/serie/faq/index.html#how-do-i-pronounce-serie)) is a TUI application that renders commit graphs like `git log --graph --all`, drawing the graph with Unicode box-drawing characters by default, and using the terminal emulators' image display protocol for high-quality output when it is selected explicitly.

<img src="https://raw.githubusercontent.com/lusingander/serie/master/img/demo.gif">

(This demo shows [Ratatui](https://github.com/ratatui/ratatui) repository!)

## Why?

While some users prefer to use Git via CLI, they often rely on a GUI or feature-rich TUI to view commit logs. Others may find `git log --graph` sufficient.

Personally, I found the output from `git log --graph` difficult to read, even with additional options. Learning complex tools just to view logs seemed cumbersome.

## Goals

- Provide a rich `git log --graph` experience in the terminal.
- Offer commit graph-centric browsing of Git repositories.

## Non-Goals

- Implement a fully-featured Git client.
- Create a TUI application with a complex UI.
- Render the graph with the same quality everywhere: the image graph is not used without being asked for, since the Unicode graph works in any terminal.

---

_Built with Rust and [ratatui](https://github.com/ratatui/ratatui)._  
_Serie is available on [GitHub](https://github.com/lusingander/serie) under the MIT license._
