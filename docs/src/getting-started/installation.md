# Installation

### [mise](https://mise.jdx.dev) (recommended)

[mise](https://mise.jdx.dev) installs the pre-compiled binary for your platform from the GitHub releases and keeps it up to date:

```
$ mise use -g github:KonghaYao/vine
```

The command records the tool in the global `mise.toml` and puts `vine` on your `PATH`. It is updated with:

```
$ mise upgrade vine
```

and it can be pinned to a version with `mise use -g github:KonghaYao/vine@1.0.0`. mise picks the asset of the current platform (Linux x86_64/aarch64 with glibc or musl, and macOS Intel/Apple Silicon) by itself, so there is nothing else to configure.

### Downloading the binary

The archives of the supported platforms, together with a `SHA256SUMS` file, are attached to [releases](https://github.com/KonghaYao/vine/releases):

```
$ curl -LO https://github.com/KonghaYao/vine/releases/latest/download/vine-<version>-<target>.tar.gz
$ curl -LO https://github.com/KonghaYao/vine/releases/latest/download/SHA256SUMS
$ grep vine-<version>-<target>.tar.gz SHA256SUMS | shasum -a 256 -c - # on Linux: sha256sum -c -
$ tar -xzf vine-<version>-<target>.tar.gz
```

The `<target>` is one of `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-gnu`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, and `aarch64-apple-darwin`.

### Build from source

If you want to check the latest development version, build from source:

```
$ git clone https://github.com/KonghaYao/vine.git
$ cd vine
$ cargo build --release # Unless it's a release build, it's very slow.
$ ./target/release/vine
```
