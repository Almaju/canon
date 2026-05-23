# Installation

## Prerequisites

- **Rust** (stable) — install via [rustup](https://rustup.rs).
  Oneway transpiles `.ow` programs to Rust and shells out to `rustc` to
  compile them, so a working Rust toolchain is needed at runtime as well.
- A working **C linker** (clang or gcc) — already present on most systems.

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
oneway version
```

## Update

```sh
oneway upgrade              # install the latest release
oneway upgrade v0.3.0       # install a specific release
oneway upgrade --check      # check whether a newer release is available
```

## Editor Support

A Zed extension with syntax highlighting is available in the
[`editors/`](https://github.com/Almaju/oneway/tree/main/editors) directory
of the repository.
