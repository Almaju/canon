# Contributing to Oneway

Thanks for your interest in Oneway! This document covers everything you need
to build the compiler, run tests, and submit a pull request.

## Prerequisites

- [Rust](https://rustup.rs) (stable toolchain — `rustup toolchain install stable`)
- [`just`](https://github.com/casey/just) task runner (`cargo install just`)

`rustfmt` and `clippy` come with the stable toolchain; no extra install needed.

## Getting the code

```sh
git clone https://github.com/almaju/oneway.git
cd oneway
```

## Building

```sh
just build        # debug build
just release      # release build (optimized)
```

## Running programs

```sh
just example clock                  # run examples/clock/ (a package)
just examples                       # run every member of the examples/ workspace
```

## Testing

```sh
just test           # run the compiler test suite
just test-verbose   # same, with stdout
just ci             # fmt check + clippy + test (run this before opening a PR)
```

`just ci` mirrors exactly what the CI workflow runs. If it passes locally,
it will pass on GitHub.

## Code style

```sh
just fmt            # format Rust source in place
just clippy         # lint (warnings are errors in CI)
```

Install the pre-commit hook to run these automatically before every commit:

```sh
just install-hooks
```

## Project layout

| Path | What lives here |
|---|---|
| `src/lexer/` | Tokenizer — `scanner.rs`, `token.rs` |
| `src/parser/` | AST construction |
| `src/checker/` | Type checker and sort-order validation |
| `src/codegen/` | Rust code generation |
| `src/formatter.rs` | Source formatter (`oneway fmt`) |
| `src/lsp/` | Language server |
| `src/ast.rs` | AST node definitions |
| `src/error.rs` | Error types and spans |
| `src/loader.rs` | File/module loading |
| `std/` | Standard library (`.ow` declarations + Rust FFI) |
| `examples/` | Example programs — always keep these passing |
| `tests/` | Integration tests |
| `docs/` | mdBook documentation site |
| `editors/` | Tree-sitter grammar and Zed extension |

## Compiler pipeline

```
source → lexer → parser → checker → codegen (Rust) → rustc
```

The compiler has **no external Rust dependencies** — keep `Cargo.toml`
dependency-free.

## Kinds of change

### Fixing a compiler bug

Open an issue first if the bug is non-trivial, and include the `.ow` snippet
that triggers it. Write a failing test in `tests/` before fixing the bug so
the fix is verifiable.

### Adding a language or compiler feature

Features that touch language semantics require a `DESIGN.md` update first.
Open an issue to discuss before writing code — language changes are
intentionally conservative.

### Adding a standard library item

Add the `.ow` declaration in `std/` and, if it needs Rust FFI backing, the
corresponding `.rs` file. Update `docs/src/reference/stdlib.md` and add an
example to `examples/` if the feature warrants one.

### Documentation

Documentation lives in `docs/src/` (mdBook). Preview it locally with:

```sh
cd docs && mdbook serve
```

## Opening a pull request

1. Fork the repo and create a branch.
2. Make your changes and run `just ci` — it must pass cleanly.
3. Run `just examples` and confirm no previously-passing examples regress.
4. Open a PR against `main` and fill in the PR template.
5. Link the relevant issue (`Closes #NNN`) in the PR description.

PRs that introduce new failing examples without justification will not be
merged.

## Questions?

Open a [GitHub Discussion](https://github.com/almaju/oneway/discussions) or
file an issue.
