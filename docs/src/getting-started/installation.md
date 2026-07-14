# Installation

Canon is a single prebuilt binary with the runtime embedded — no Rust
toolchain, no external linker, no prerequisites. (Building the
*compiler* from source is a contributor concern and needs stable Rust.)

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

Extensions for Zed and VS Code live under
[`editors/`](https://github.com/Almaju/canon/tree/main/editors) —
syntax highlighting plus a language server that is just the `canon`
binary (`canon lsp`), so there is nothing separate to install.
