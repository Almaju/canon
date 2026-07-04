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

One installation holds both channels, and the `canon` on your `PATH` is a thin
launcher that picks the active one:

- **stable** (the fallback) — versioned releases (`vX.Y.Z`), promoted from a
  tested nightly.
- **nightly** — a rolling prerelease rebuilt automatically on every push to
  `main`. Latest features, less settled.

Switching is one word, scoped by where you run it — no config file in your
project, and no separate "default" vs "override" machinery:

```sh
canon use nightly       # this directory (and below) now uses nightly —
                        # installs it first if it isn't on disk
cd ~ && canon use nightly   # run it in your home directory: global default
canon use               # show the active toolchain, why, and what's installed
```

For a single command, the channel is the first word — like a dispatch arm:

```sh
canon nightly run app.can
canon stable test suite.can
```

A bare `canon` resolves: explicit channel word → nearest `canon use` ancestor
→ `stable`. Selections live centrally in `~/.canon/uses`; to remove a
toolchain from disk, delete `~/.canon/toolchains/<channel>`.

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
