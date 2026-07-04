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

## Channels

Canon ships on two channels:

- **stable** (default) — versioned releases (`vX.Y.Z`), promoted manually from
  a tested nightly.
- **nightly** — a rolling prerelease rebuilt automatically on every push to
  `main`. Latest features, less settled.

Install the nightly channel:

```sh
CANON_CHANNEL=nightly curl -fsSL https://raw.githubusercontent.com/almaju/canon/main/install.sh | sh
```

## Verify

```sh
canon --version            # e.g. "canon 0.3.1 (stable)"
```

## Update

```sh
canon upgrade              # install the latest build on your current channel
canon upgrade v0.3.0       # install a specific release
canon upgrade --check      # check whether a newer release is available
canon upgrade --nightly    # switch to nightly and update
canon upgrade --stable     # switch to stable and update
canon channel              # print the current channel
canon channel nightly      # switch channel without updating yet
```

## Editor Support

The Zed extension at
[`editors/zed-canon`](https://github.com/Almaju/canon/tree/main/editors/zed-canon)
provides syntax highlighting and a built-in language server. Install it
via Zed's *Install Dev Extension* command — see
[`editors/README.md`](https://github.com/Almaju/canon/blob/main/editors/README.md)
for the full instructions. The extension reuses the same `canon` binary
(`canon lsp` subcommand), so there is no separate LSP install.
