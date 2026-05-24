# Installation

## Prerequisites

None at runtime. Oneway ships a single prebuilt `oneway` binary that
embeds the `wasmtime` Component Model runtime — there is no Rust
toolchain to install, no `rustc`/`cargo` invoked at build time, and no
external linker needed.

(If you want to build the compiler from source rather than install a
prebuilt release, you'll need stable Rust via [rustup](https://rustup.rs)
— but that's a contributor concern, not a user one.)

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/almaju/oneway/main/install.sh | sh
```

The script downloads a prebuilt `oneway` binary for your platform (macOS
arm64/x86_64, Linux arm64/x86_64) and installs it to `~/.oneway/bin/oneway`.
Add that directory to your PATH as instructed by the installer.

Pin to a specific version:

```sh
curl -fsSL https://raw.githubusercontent.com/almaju/oneway/main/install.sh | sh -s v0.3.0
```

## Verify

```sh
oneway --version
```

## Update

```sh
oneway upgrade              # install the latest release
oneway upgrade v0.3.0       # install a specific release
oneway upgrade --check      # check whether a newer release is available
```

## Editor Support

The Zed extension at
[`editors/zed-oneway`](https://github.com/Almaju/oneway/tree/main/editors/zed-oneway)
provides syntax highlighting and a built-in language server. Install it
via Zed's *Install Dev Extension* command — see
[`editors/README.md`](https://github.com/Almaju/oneway/blob/main/editors/README.md)
for the full instructions. The extension reuses the same `oneway` binary
(`oneway lsp` subcommand), so there is no separate LSP install.
