# Oneway

> *"There is ONE WAY to do everything."*

Oneway is a maximally opinionated, strict, functional-leaning systems language that compiles to Rust. Every design decision eliminates choice. If there's a best practice, it's the *only* practice.

- **One syntax** per concept — no aliases, no shortcuts.
- **Compiler-enforced style** — sorting, naming, structure are all checked.
- **Functional-leaning, systems-capable** — immutable by default, expression-based, native code output.
- **Transparent effects** — dependencies and errors live in the type system.

See [`DESIGN.md`](DESIGN.md) for the full language specification.

---

## Repository Layout

| Path | Description |
|------|-------------|
| [`src/`](src/) | The `oneway` compiler (lexer, parser, checker, codegen) and `oneway-lsp` server |
| [`oneway-lints/`](oneway-lints/) | Standalone [dylint](https://github.com/trailofbits/dylint) library applying Oneway's philosophy to plain Rust |
| [`cargo-oneway/`](cargo-oneway/) | `cargo oneway` — one-command runner for rustfmt + clippy + `oneway-lints` |
| [`examples/`](examples/) | Example `.ow` programs |
| [`editors/`](editors/) | Tree-sitter grammar and Zed extension |
| [`DESIGN.md`](DESIGN.md) | Language design document |

---

## Quick Start

### Compile and run an Oneway program

```sh
just run examples/hello.ow
```

### Other compiler commands

```sh
just build              # Build the compiler
just test               # Run all tests
just emit examples/hello.ow    # Show generated Rust
just ast  examples/hello.ow    # Show AST
just examples           # Run every example in examples/
```

### LSP

```sh
just install-lsp        # Build and install oneway-lsp to ~/.cargo/bin
```

The `install-lsp` recipe prints the Zed `settings.json` snippet for hooking it up.

---

## Oneway Lints for Rust

[`oneway-lints/`](oneway-lints/) is a [dylint](https://github.com/trailofbits/dylint) library that enforces Oneway's rules — sorting, newtype discipline, no `unwrap`, no manual loops, etc. — on a regular Rust codebase. It works on any Rust project; you don't have to use the Oneway language to use the lints.

See [`oneway-lints/README.md`](oneway-lints/README.md) for the full lint catalog with examples.

### Using the lints in another project

You don't need to publish anything — dylint loads compiled libraries at runtime. Pick one of three integration paths:

#### 1. Git (no local checkout)

In the consumer project's root `Cargo.toml`:

```toml
[workspace.metadata.dylint]
libraries = [
    { git = "https://github.com/Almaju/oneway", pattern = "oneway-lints" },
]
```

Then:

```sh
cargo install cargo-dylint dylint-link
cargo dylint --all
```

#### 2. Local path

```toml
[workspace.metadata.dylint]
libraries = [
    { path = "/absolute/path/to/oneway/oneway-lints" },
]
```

```sh
cargo dylint --all
```

#### 3. Environment variable

```sh
cd oneway-lints && cargo build
DYLINT_LIBRARY_PATH="$(pwd)/target/debug" cargo dylint --all
```

This repo's `just lint` recipe uses this mode to dogfood the lints against the compiler source.

### Toolchain requirement

`oneway-lints` pins nightly Rust with the `rustc-dev` and `llvm-tools-preview` components (see [`oneway-lints/rust-toolchain.toml`](oneway-lints/rust-toolchain.toml)). The consuming project doesn't need to switch toolchains for its own code — `cargo-dylint` handles building the lint library with the right toolchain — but you do need that nightly installed:

```sh
rustup toolchain install nightly --component rustc-dev,llvm-tools-preview
```

---

## License

See [`DESIGN.md`](DESIGN.md) for project status. This is an experimental design exploration.
