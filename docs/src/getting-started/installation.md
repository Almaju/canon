# Installation

## Prerequisites

None at runtime. Canon ships a single prebuilt `canon` binary with the
`wasmtime` Component Model runtime embedded: no Rust toolchain, no
`rustc`/`cargo` at build time, no external linker.

Building the compiler from source (a contributor concern, not a user
one) needs stable Rust via [rustup](https://rustup.rs).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/almaju/canon/main/install.sh | sh
```

The script downloads a prebuilt `canon` binary for your platform (macOS
arm64/x86_64, Linux arm64/x86_64) and installs it to `~/.canon/bin/canon`.
Add that directory to your PATH as the installer instructs.

Pin to a specific version:

```sh
curl -fsSL https://raw.githubusercontent.com/almaju/canon/main/install.sh | sh -s v0.3.0
```

## Toolchains

Two channels — **stable** (versioned releases, the fallback) and
**nightly** (rebuilt on every push to `main`) — live in one
installation, and the `canon` on your `PATH` picks the active one:

```sh
canon use nightly           # this directory (and below) now uses nightly
canon use                   # show the active toolchain and why
canon nightly run app.can   # one-shot: the channel as the first word
```

Run `canon use` from your home directory to set a global default.
There is no project config file; selections live in `~/.canon/uses`.

## Verify

```sh
canon --version            # reports the active toolchain's version
```

## Update

```sh
canon upgrade              # update the active toolchain to its channel's latest
canon upgrade --check      # check whether a newer stable release is available
```

## Editor Support

The Zed extension at
[`editors/zed-canon`](https://github.com/Almaju/canon/tree/main/editors/zed-canon)
provides syntax highlighting and a built-in language server. Install it
via Zed's *Install Dev Extension* command;
[`editors/README.md`](https://github.com/Almaju/canon/blob/main/editors/README.md)
has the full instructions. The extension runs the same `canon` binary
(`canon lsp` subcommand), so there is no separate LSP install.
