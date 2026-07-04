# Installation

## Prerequisites

None at runtime. Canon ships a single prebuilt `canon` binary that
embeds the `wasmtime` Component Model runtime — there is no Rust
toolchain to install, no `rustc`/`cargo` invoked at build time, and no
external linker needed.

(If you want to build the compiler from source rather than install a
prebuilt release, you'll need stable Rust via [rustup](https://rustup.rs)
— but that's a contributor concern, not a user one.)

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/almaju/canon/main/install.sh | sh
```

The script downloads a prebuilt `canon` binary for your platform (macOS
arm64/x86_64, Linux arm64/x86_64) and installs it to `~/.canon/bin/canon`.
Add that directory to your PATH as instructed by the installer.

Pin to a specific version:

```sh
curl -fsSL https://raw.githubusercontent.com/almaju/canon/main/install.sh | sh -s v0.3.0
```

## Toolchains

Canon manages toolchains the way `rustup` does: one installation holds several
toolchains, and the `canon` on your `PATH` is a thin launcher that picks the
active one. Two channels are available:

- **stable** (default) — versioned releases (`vX.Y.Z`), promoted from a tested
  nightly.
- **nightly** — a rolling prerelease rebuilt automatically on every push to
  `main`. Latest features, less settled.

The bootstrap script installs **stable**. Add and manage the others:

```sh
canon toolchain install nightly   # add the nightly toolchain
canon toolchain list              # show installed toolchains (marks the default)
canon toolchain uninstall nightly # remove one
```

## Switching toolchains

No project config file is involved — switching is global, per-directory, or
per-command:

```sh
canon default nightly        # global default
canon override set nightly   # pin the current directory (and children)
canon override unset         # remove that pin
canon +nightly run app.can   # one command, one toolchain
CANON_TOOLCHAIN=nightly canon build app.can   # via environment
```

A bare `canon` resolves in this order: `+toolchain` → `CANON_TOOLCHAIN` →
directory override → global default → `stable`.

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
via Zed's *Install Dev Extension* command — see
[`editors/README.md`](https://github.com/Almaju/canon/blob/main/editors/README.md)
for the full instructions. The extension reuses the same `canon` binary
(`canon lsp` subcommand), so there is no separate LSP install.
