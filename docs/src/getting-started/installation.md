# Installation

Oneway is distributed as source. You build the compiler with `cargo`, then
use it to compile `.ow` programs.

## Prerequisites

- **Rust** (stable) — install via [rustup](https://rustup.rs).
- **just** — a command runner. Install with `cargo install just` or your
  package manager.
- A working **C linker** (clang or gcc) — already present on most systems.

## Building the Compiler

Clone the repository and build:

```sh
git clone https://github.com/Almaju/oneway
cd oneway
just build
```

This produces a debug binary at `target/debug/oneway`. The `justfile` at the
project root wraps the compiler in convenient recipes, so you rarely call
the binary directly.

## Installing the Binary

To install a release build to `~/.cargo/bin/oneway` (which should already be
on your `PATH` if you installed Rust via rustup):

```sh
just install
```

After this you can invoke `oneway` directly from any directory, without going
through `just`.

## Verifying the Install

Run the bundled hello-world example:

```sh
just run examples/hello.ow
```

You should see:

```oneway
hello
```

## Repository Layout

| Path          | What it is                                              |
|---------------|---------------------------------------------------------|
| `src/`        | The compiler (lexer, parser, checker, codegen).         |
| `examples/`   | Sample `.ow` programs.                                  |
| `editors/`    | Tree-sitter grammar and Zed extension.                  |
| `DESIGN.md`   | The full language specification.                        |

For editor support, see `editors/README.md` in the repository.
