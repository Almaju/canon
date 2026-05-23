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
| `src/main.rs` | CLI entry point (`run`, `build`, `emit`, `check`, `test`, `ast`, `tokens`, `upgrade`) |
| `src/lib.rs` | Public crate modules |
| `std/` | Oneway standard library (`.ow` files + Rust FFI `.rs` files) |
| `examples/` | Example `.ow` programs |
| `githooks/` | Git hooks (`pre-commit`) |
| `tests/` | Rust integration tests (incl. `tests/fixtures/` & `tests/oneway/`) |
| `editors/` | Tree-sitter grammar and Zed extension |
| `install.sh` | Installer script for prebuilt binaries |
| `DESIGN.md` | Language specification — the source of truth for language semantics |
| `README.md` | Project README |

## Build & dev commands

This project uses [`just`](https://github.com/casey/just) as a task runner and standard `cargo` underneath.

```sh
just build              # cargo build (debug)
just install            # cargo install --path . --force (release → ~/.cargo/bin)
just test               # cargo test (Rust unit/integration tests for the compiler)
just test-ow            # run every tests/oneway/*_test.ow file via `oneway test`
just update-fixtures    # regenerate golden .stderr files under tests/fixtures/checker/fail/
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

One canonical entry point: **`cargo test`** (or `just test`). Every test layer
is a `tests/*.rs` integration-test binary, so a single command runs them all
and fails the build on any regression. CI runs nothing else.

### Layout

```
tests/
  checker/
    ok/<name>.ow              # must check with zero errors
    fail/<name>.ow            # must produce errors
    fail/<name>.stderr        # golden: exact expected stderr
  runtime/
    <name>.ow                 # must run to completion (exit 0)
    <name>.stdout             # golden: exact captured stdout
  oneway/
    <name>_test.ow            # functions with signature `() -> TestResult`
  common/mod.rs               # shared helpers (fixture loader, golden compare,
                              # subprocess invocation)
  checker_fixtures.rs         # harness for tests/checker/
  runtime_fixtures.rs         # harness for tests/runtime/
  oneway_tests.rs             # harness for tests/oneway/
  checker_api.rs              # Rust tests of compiler internals
```

Each layer answers a different question. Pick the layer that matches
**what the test observes**:

| You want to test… | Use… |
|---|---|
| "the checker accepts this source" | `tests/checker/ok/<name>.ow` |
| "the checker rejects this source with a specific error" | `tests/checker/fail/<name>.ow` + `.stderr` |
| "this program runs end-to-end and prints exactly this" | `tests/runtime/<name>.ow` + `.stdout` |
| "this expression produces the right value at runtime" | `tests/oneway/<file>_test.ow` |
| "this stdlib function does what it claims" | `tests/oneway/<file>_test.ow` |
| "the parser handles this edge case" | `tests/checker/ok/<name>.ow` (it parsed if the checker ran) |
| "a compiler API behaves correctly under unusual input" | `tests/checker_api.rs` |

### Adding a test

- **Checker fixture (ok)**: drop a new `.ow` file into `tests/checker/ok/`. The harness picks it up automatically.
- **Checker fixture (fail)**: drop a new `.ow` file into `tests/checker/fail/`, then run `just update-fixtures` to generate the sibling `.stderr` from the actual checker output. Review the golden file and commit both.
- **Runtime fixture**: drop a new `.ow` file into `tests/runtime/`, then `just update-fixtures` to generate the sibling `.stdout` from the actual program output. Review and commit both.
- **Oneway test**: add a function with signature `() -> TestResult` to any `tests/oneway/*_test.ow` file (or create a new one). Discovery is by type signature — the `test*` name prefix is convention, not a requirement.
- **Compiler API test**: only when the test needs to call the checker with synthetic arguments. Keep these rare.

### Updating goldens

When an error message or a program's output changes intentionally, run
`just update-fixtures` (mirrors `TRYBUILD=overwrite` from Rust's
`trybuild`). The harness rewrites every `.stderr` and `.stdout` from the
actual current output. The `git diff` is the review surface for
"did this output change in a sensible way?".

### Oneway-language test framework

```
use std/TestResult

testAddPositive = () -> TestResult {
    1.add(2).eq(3).assert()
}
```

- `TestResult = Fail + Pass`, with `Fail = String` carrying the assertion's failure message.
- `assert = (Bool * String) -> TestResult` turns a `Bool` and a message into a `TestResult`. When the bool is `True`, returns `Pass()`; when `False`, returns `Fail(message)`.
- The synthesised `main` dispatches each test on its result and prints a `[ ok ] testName` line on `Pass` or a `[FAIL] testName` line followed by the failure message on `Fail`. The message and banner are on separate lines because `String.concat` is currently a codegen stub — once concat lands they merge into one.
- Each test ends in a chain that produces a `TestResult` (typically `.eq(...).assert()`). Multi-assertion tests via `?`-propagation are a follow-up that lands when `?` itself learns short-circuit semantics (currently a payload-extractor only).
- The synthesised `main` is exempt from free-function alphabetical ordering (main is the entry point, distinguished by role).
- Runtime exit code is currently **always 0** even when tests print `[FAIL]`. The `tests/oneway_tests.rs` harness parses stdout for `[FAIL]` to detect failures. Once exit-code threading lands, this parsing becomes redundant but harmless.
- `just test-ow` runs the same tests with pretty per-file output (faster local iteration); the canonical CI path is still `cargo test`.

### Examples are not tests

Files in `examples/` exist purely as documentation — readable programs that demonstrate how to use a language feature or stdlib idiom. They are not part of `cargo test`. `just examples` runs them as an optional smoke check ("does the whole pipeline still work end-to-end?"), but they are not a coverage layer. When a language feature needs test coverage, it goes into one of the test layers above; the `examples/` directory follows only when there's something worth demonstrating pedagogically.

Most example-shaped tests (small deterministic programs that exercise one language feature) live in `tests/runtime/` now. `examples/` is reserved for programs that show real-world usage — HTTP servers, file I/O, JSON parsing, randomness — things that intentionally have non-deterministic or environment-dependent output.

### Known codegen gaps (test-visible)

The checker accepts more than the codegen currently implements. Each item below is a feature whose *syntax* and *types* are pinned by fixtures in `tests/checker/ok/`, but whose *runtime* behaviour isn't ready yet. Pick any of these up as a self-contained PR.

| Gap | Symptom | Where it bites |
|---|---|---|
| **`String.concat` is a no-op stub** | `"a".concat("b").print()` produces no output — concat drops args and returns Unit | The test framework's failure banner + message print as two separate lines instead of one joined string |
| **`Int(1)` explicit constructor** | Breaks codegen; numeric literal `1` works | Forces Oneway tests to use literals, not explicit constructors |
| **`Bool.and` / `.or` / `.not` method chains** | Break codegen even with value instances (`True().and(False())`) | Limits boolean composition in tests |
| **3-variant user union dispatch** | Wrong arm selected at runtime; 2-variant works (see `tests/runtime/variant_payload_extraction.ow`) | Tests must use binary unions |
| **Option/Result string payloads on the construct side** | `Some("hi")` doesn't write the string to the Option struct; reading the bound `String` in `(Some<String>)` arms then yields garbage | User-defined unions with string payloads work end-to-end (see `tests/runtime/variant_payload_extraction.ow`); the stdlib Option/Result paths are separate and need their own fix in `build_option_some` / `build_result_ok` / `store_payload_at_offset` |
| **`?` short-circuit propagation** | `Result<T,E>?` extracts the payload but doesn't actually short-circuit on `Err` | Multi-step error handling is fragile |
| **`use` ordering check unreachable** | The "use must come first" check in the checker is dead code under the loader path | Cosmetic; loader strips uses before checking |

The following gaps were *closed* in recent passes — mentioned here so the
shape of the working machinery is documented somewhere:

| Recently fixed | Mechanism |
|---|---|
| Newtype field access (`Greeting("hi").String`) | `newtype_unwrap_ty` in `src/codegen/wasm/mod.rs`: retypes the on-stack value when the field name matches the underlying type. No-op at the wasm level. |
| Variant payloads in arm bodies (user variants, string payloads) | `bind_arm_payload` + `arm_payload_binding` in `src/codegen/wasm/mod.rs`: extracts `(ptr, len)` from offsets 4/8 of the union struct into adjacent locals, then binds the arm's pattern name in the scope so `Expr::Ident` lookups find it. Pairs with `SavedField::Str0` on the construction side. |

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
