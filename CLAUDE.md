# CLAUDE.md — Agent Context for Oneway

## What is this project?

Oneway is a programming language whose reference compiler transpiles to Rust. The compiler is itself written in Rust. Oneway inherits Rust's ownership model and zero-cost abstractions while presenting a much smaller surface area — no `let`, no `if`/`else`, no comments, no local variables. Branching is dispatch on a union. Effects are passed as capabilities. The guiding rule: **wherever ordering is discretionary, the compiler enforces alphabetical order**.

See `DESIGN.md` for the full language specification.

## Repository layout

| Path | Description |
|---|---|
| `.github/` | CI workflows (docs deployment, release pipeline) |
| `docs/` | mdBook documentation site (`book.toml`, `src/`) |
| `src/` | Compiler source (Rust) |
| `src/lexer/` | Lexer — tokenization (`scanner.rs`, `token.rs`) |
| `src/parser/` | Parser — AST construction (`parser.rs`) |
| `src/checker/` | Type checker and sort-order validation |
| `src/codegen/` | Code generation — Rust transpilation (`rust.rs`) |
| `src/ast.rs` | AST node definitions |
| `src/error.rs` | Error types and spans |
| `src/loader.rs` | File/module loading |
| `src/main.rs` | CLI entry point (`run`, `build`, `emit`, `check`, `ast`, `tokens`, `upgrade`) |
| `src/lib.rs` | Public crate modules |
| `std/` | Oneway standard library (`.ow` files + Rust FFI `.rs` files) |
| `examples/` | Example `.ow` programs |
| `githooks/` | Git hooks (`pre-commit`) |
| `tests/` | Rust integration tests |
| `editors/` | Tree-sitter grammar and Zed extension |
| `install.sh` | Installer script for prebuilt binaries |
| `DESIGN.md` | Language specification — the source of truth for language semantics |
| `README.md` | Project README |

## Build & dev commands

This project uses [`just`](https://github.com/casey/just) as a task runner and standard `cargo` underneath.

```sh
just build              # cargo build (debug)
just install            # cargo install --path . --force (release → ~/.cargo/bin)
just test               # cargo test
just examples           # compile + run all examples, report pass/fail/skip
just example <name>     # run a single example by name
just fmt                # cargo fmt
just clippy             # cargo clippy -- -W warnings
just ci                 # fmt + clippy + test (mirrors CI)
just clean              # cargo clean + remove compiled examples
just install-hooks      # install git hooks (pre-commit)
just uninstall-hooks    # uninstall git hooks
just build-extension    # build the Zed extension WASMs
```

To cut a release, use the **bump** GitHub Actions workflow (`Actions → bump → Run workflow`). It handles the version bump, commit, tag, and triggers the cross-build release pipeline automatically.

## Testing

- Run `just test` (or `cargo test`) to execute the test suite.
- Integration tests live in `tests/`.
- Running `just examples` is a good smoke test — it compiles and runs every example program and reports results.

## Key conventions

- **Alphabetical ordering** is central to the language. If you modify the parser or checker, be aware that sort-order enforcement applies to: product type fields, union variants, function declarations, dispatch arms, and imports.
- The compiler pipeline is: **source → lexer → parser → checker → codegen (Rust) → rustc**.
- Generated Rust is compiled by shelling out to `rustc` (single file) or `cargo` (when extern dependencies are needed).
- Standard library items in `std/` come in pairs: a `.ow` file declaring the Oneway interface and optionally a `.rs` file with the Rust FFI implementation.
- Example programs in `examples/` should always compile and run after changes — use `just examples` to verify.

## Code style

- Rust code follows standard `rustfmt` formatting (`just fmt`).
- Keep `clippy` clean (`just clippy`).
- The compiler avoids external Rust dependencies — `Cargo.toml` has no `[dependencies]`.

## Oneway language quick reference

```
Bool = False + True                            # union
User = Birthday * Username                     # product

greet = (Greeting * Name) -> Greeting {        # function (free, commutative)
    Greeting
}

main = (Stdout) -> Unit {                      # entry point
    "hello".print(Stdout)
}

True.(                                         # dispatch (branch on union)
    False => "no".print(Stdout),
    True  => "yes".print(Stdout),
)

List(1, 2, 3).map((Int) -> Int { Int.mul(2) }) # lambda
```

- No local variables, no `let`, no `if`/`else`, no comments in the language.
- Capabilities (`Stdout`, `Filesystem`, etc.) are passed explicitly.
- `use Foo` imports from `foo.ow` in the same module directory (or the corresponding folder for modules).
- See `DESIGN.md` for the complete specification.
