# Contributing to Canon

Thanks for your interest in Canon! This document covers everything you need
to build the compiler, run tests, and submit a pull request.

## Prerequisites

- [Rust](https://rustup.rs) (stable toolchain — `rustup toolchain install stable`)
- [`just`](https://github.com/casey/just) task runner (`cargo install just`)

`rustfmt` and `clippy` come with the stable toolchain; no extra install needed.

## Getting the code

```sh
git clone https://github.com/almaju/canon.git
cd canon
```

## Building

```sh
just build        # debug build
just install      # release build, installed to ~/.cargo/bin/canon
```

## Running programs

```sh
just example multifile              # run examples/multifile/ (a package)
just examples                       # run every member of the examples/ workspace
```

## Testing

```sh
just test           # run the compiler test suite (cargo test)
just test-can       # run every tests/canon/*_test.can via `canon test`
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
| `src/codegen/` | WebAssembly component code generation |
| `src/bindgen/` | `canon bindgen` — WIT → Canon source emitter |
| `src/formatter.rs` | Source formatter (`canon check --fix`, format diagnostics) |
| `src/lsp/` | Language server |
| `src/webhost.rs` | Web target's generated JS host and static server |
| `src/ast.rs` | AST node definitions |
| `src/error.rs` | Error types and spans |
| `src/loader.rs` | File/module loading and reference resolution |
| `packages/canon/std/` | Standard library — one bundled package (Canon wrappers over WIT-derived bindings) |
| `examples/` | Example programs — always keep these passing |
| `tests/` | Integration tests |
| `docs/` | mdBook documentation site |
| `editors/` | Tree-sitter grammar, Zed extension, VS Code extension |

## Compiler pipeline

```
source → lexer → parser → checker (format phase + semantics) → codegen (WASM core module → Component Model wrapper)
```

Formatting is a compiler phase: the checker diffs each source against
its canonical rendering and reports a divergence as a `format error`,
fused into the same run as sort-order and type errors.

No external toolchain is invoked: `wasm-encoder` / `wit-component` produce
the final `.wasm` in-process, and `canon run` executes it on the embedded
wasmtime runtime. Dependencies are limited to the Bytecode Alliance wasm
toolchain, the embedded runtime (`wasmtime`, `tokio`), and the hyper HTTP
stack — don't add dependencies outside that orbit.

## Kinds of change

### Fixing a compiler bug

Open an issue first if the bug is non-trivial, and include the `.can` snippet
that triggers it. Write a failing test in `tests/` before fixing the bug so
the fix is verifiable.

### Adding a language or compiler feature

Features that touch language semantics require a spec update first
(`docs/src/spec/`). Open an issue to discuss before writing code —
language changes are intentionally conservative.

### Adding a standard library item

Add the Canon wrapper under `packages/canon/std/src/`. If it needs a new
host binding, declare the WIT import in the package's `canon.toml`
(`[imports]`) and regenerate the vendored bindings with `just
regen-bindings` (= `canon install packages/canon/std`) — never hand-edit
the `bindgen/` tree. Add an example to `examples/` if the feature warrants
one.

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

Open a [GitHub Discussion](https://github.com/almaju/canon/discussions) or
file an issue.
