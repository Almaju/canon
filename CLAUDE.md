# CLAUDE.md — Agent Context for Canon

## What is this project?

Canon is a programming language whose reference compiler emits **WebAssembly components** directly (not transpiled to Rust — that was the original prototype). The compiler is itself written in Rust. Canon presents a small surface area — no `let`, no `if`/`else`, no comments, no local variables, no import statement (references resolve to files automatically). Branching is dispatch on a union. Effects are passed as capabilities. The guiding rule: **wherever ordering is discretionary, the compiler enforces alphabetical order**.

See the language spec under `docs/src/spec/` for the full
specification — it is the canonical design document.

**The docs site is itself a Canon program.** The documentation is not an
mdBook: `docs/src/main.can` is an Elm-architecture web app that renders
the `docs/src/*.md` pages with the standard library's Markdown renderer,
compiled by `canon build docs` to a WebAssembly module. It dogfoods the
web target (see `docs/src/reference/web-target.md`). Page content stays
plain Markdown under `docs/src/`; `main.can` (routing/sidebar) and
`styles.can` (the stylesheet) are the app shell.

**Root-doc policy:** the repository root holds no design/planning
markdown. `CLAUDE.md` (this file) is the only doc that lives at the
root, alongside `README.md` and the standard community-health files
(`CODE_OF_CONDUCT.md`, `CONTRIBUTING.md`, `SECURITY.md`). All language
and reference documentation lives under `docs/src/`. Do not add new
`*.md` files at the root; add a page under `docs/src/` (and a route in
`docs/src/main.can`) instead.

## Repository layout

| Path | Description |
|---|---|
| `.github/` | CI workflows (docs deployment, release pipeline) |
| `docs/` | Documentation site. `src/main.can` + `src/styles.can` are a Canon web app (the Elm triple) that renders the `src/*.md` pages via the stdlib Markdown renderer; `canon build docs` compiles it to `docs/build/` (a wasm module + `canon-web.js` host + `index.html`). `assets/docs-enhance.js` is a classic script the deploy injects for progressive enhancement — Canon syntax highlighting and click-to-run buttons. `runner/build.mjs` compiles each ` ```canon,run=… ` snippet to a jco bundle at build time (the renderer emits the fence info-string as `data-info`, which the enhancer reads for both language and run name). `landing/index.html` is the self-contained marketing page. The docs workflow deploys the landing at the site root and the app under `/doc/`. |
| `src/` | Compiler source (Rust) |
| `src/lexer/` | Lexer — tokenization (`scanner.rs`, `token.rs`) |
| `src/parser/` | Parser — AST construction (`parser.rs`) |
| `src/checker/` | Type checker and sort-order validation |
| `src/codegen/` | Code generation — WebAssembly components (`wasm/mod.rs` core module, `wasm/component.rs` component wrapper, `async_analysis.rs`) |
| `src/ast.rs` | AST node definitions |
| `src/error.rs` | Error types and spans |
| `src/loader.rs` | File/module loading |
| `src/bindgen/` | `canon bindgen` — WIT → Canon source emitter (`naming.rs`, `emit.rs`, `mod.rs`) |
| `src/main.rs` | CLI entry point (`run`, `build`, `check`, `test`, `fmt`, `inspect`, `bindgen`, `lsp`, `upgrade`) |
| `src/webhost.rs` | Web target's browser side — the generated JS host (`canon-web.js`), `index.html` shell, bundle writer, static server for `canon run` (see `docs/src/reference/web-target.md`) |
| `src/lib.rs` | Public crate modules |
| `src/manifest.rs` | `canon.toml` parser (TOML subset, hand-written) |
| `build.rs` | Walks `packages/` and emits a bundled-package registry baked into the compiler binary |
| `packages/canon/std/` | The standard library — one shipped package. Contains hand-written wrappers under `src/`, WIT-derived bindings under `bindgen/` (committed), and an `canon.toml` declaring its WIT imports under `[imports]`. The loader resolves references to stdlib names against this tree. |
| `packages/canon/std/bindgen/` | Generated WASI bindings in the versioned vendored layout (`wasi/<pkg>@<ver>/<iface>.can`), produced by `just regen-bindings` (= `canon install packages/canon/std`). Committed so `cargo build` works on a fresh clone; derived — never hand-edited. A same-`rel_path` file under `src/` shadows its `bindgen/` twin (how the hand-written `wasi/http@<ver>/types.can` supersedes the generated one until resource lowering lands). |
| `wit-vendor/wasi/` | Vendored upstream WIT files — source for `packages/canon/std/bindgen/`. Bumped when WASI advances. |
| `examples/` | Example `.can` programs |
| `githooks/` | Git hooks (`pre-commit`) |
| `tests/` | Rust integration tests (incl. `tests/fixtures/` & `tests/canon/`) |
| `editors/` | Tree-sitter grammar, Zed extension, VS Code extension (publishing runbook in `editors/PUBLISHING.md`) |
| `install.sh` | Installer script for prebuilt binaries |
| `README.md` | Project README |

## Build & dev commands

This project uses [`just`](https://github.com/casey/just) as a task runner and standard `cargo` underneath.

```sh
just build              # cargo build (debug)
just install            # cargo install --path . --force (release → ~/.cargo/bin)
just test               # cargo test (Rust unit/integration tests for the compiler)
just test-can           # run every tests/canon/*_test.can file via `canon test`
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
just build-vscode-extension  # package the VS Code extension (.vsix)
```

Releases run on two channels, and **no release workflow ever pushes to `main`** (a hard requirement — `main` has a ruleset requiring the `ci` status check, so a bot push of a version-bump commit is rejected with `GH013`). Versions come from tags, and `Cargo.toml`'s version is stamped at build time in CI (never committed):

- **nightly** — every push to `main` (except docs-, editors-, or markdown-only changes) runs the **nightly** workflow (`.github/workflows/nightly.yml`), which builds the cross-platform binaries and publishes/updates a rolling `nightly` tag as a GitHub **prerelease**. The version label is `<next-patch>-nightly.<date>.g<sha>`. `concurrency: cancel-in-progress` means a newer push supersedes an in-flight nightly.
- **stable** — the **promote** workflow (`.github/workflows/promote.yml`, `Actions → promote → Run workflow`) turns a commit (by default the current `nightly`) into the next `vX.Y.Z` release: it computes the version from the existing `v*` tags (`bump` input: patch/minor/major), creates that tag on the chosen commit, and publishes a normal (non-prerelease) release, which becomes the repo's **Latest**.

Both entry points call the reusable **`release.yml`** via `workflow_call` (inputs: `ref`, `tag`, `version`, `prerelease`, `make_latest`, `move_tag`). GitHub's `/releases/latest` ignores prereleases, so the stable channel never picks up a nightly.

**Client side: canon-style toolchains — two concepts.** One installation holds both channels under `$CANON_INSTALL/toolchains/<channel>/canon` (`$CANON_INSTALL` = `$HOME/.canon` by default), and `$CANON_INSTALL/bin/canon` (on `PATH`) is a thin **launcher** — a copy of a real toolchain binary; every canon binary can act as the launcher via `toolchain::launch` in `src/main.rs`. The CLI surface is deliberately two concepts (rustup's five — toolchain mgmt / default / override / `+sigil` / env — collapsed, per the "wherever choice is discretionary, remove the concept" rule): (1) **`canon use [stable|nightly]`**, scoped by cwd — records "this directory and below use X" in `$CANON_INSTALL/uses` (central registry, **no project config file**, in keeping with the no-`.toml` direction; longest-prefix match wins, so a deeper `use` shadows an outer one, and a `use` at `$HOME` acts as the global default); using a channel that isn't on disk installs it first; bare `canon use` prints the active toolchain + provenance + installed list. (2) **`canon stable <cmd>` / `canon nightly <cmd>`** — one-shot, the channel as first word like a dispatch arm. Resolution: explicit word → `CANON_TOOLCHAIN` env (undocumented CI escape hatch) → nearest `use` ancestor → `stable`; if that fallback isn't installed, the sole installed toolchain runs, else the launcher runs in-process — there is no persisted "default" state. The launcher `exec`s `toolchains/<tc>/canon` setting `CANON_RESOLVED` to guard re-entry; `is_launcher()` (running exe's parent == `<install>/bin`) gates dispatch so dev builds and the exec'd toolchain run the compiler in-process, and `canon use` itself runs in-launcher. `install.sh` reads `CANON_CHANNEL` (`stable` default / `nightly`) to lay a toolchain into `toolchains/<channel>/` and refreshes the launcher. `canon upgrade` updates the active toolchain. Pushes touching `editors/vscode-canon/` instead run **publish-vscode-extension**, which packages the `.vsix` and publishes it to the VS Code Marketplace / Open VSX when `package.json`'s version is new there (requires the `VSCE_PAT` / `OVSX_PAT` secrets — see `editors/PUBLISHING.md`); the release pipeline also attaches the `.vsix` to every GitHub release.

## Testing

One canonical entry point: **`cargo test`** (or `just test`). Every test layer
is a `tests/*.rs` integration-test binary, so a single command runs them all
and fails the build on any regression. CI runs nothing else.

### Layout

```
tests/
  checker/
    ok/<name>.can              # must check with zero errors
    fail/<name>.can            # must produce errors
    fail/<name>.stderr        # golden: exact expected stderr
  runtime/
    <name>.can                 # must run to completion (exit 0)
    <name>.stdout             # golden: exact captured stdout
  canon/
    <name>_test.can            # functions with signature `() => TestResult`
  common/mod.rs               # shared helpers (fixture loader, golden compare,
                              # subprocess invocation)
  checker_fixtures.rs         # harness for tests/checker/
  runtime_fixtures.rs         # harness for tests/runtime/
  canon_tests.rs             # harness for tests/canon/
  checker_api.rs              # Rust tests of compiler internals
```

Each layer answers a different question. Pick the layer that matches
**what the test observes**:

| You want to test… | Use… |
|---|---|
| "the checker accepts this source" | `tests/checker/ok/<name>.can` |
| "the checker rejects this source with a specific error" | `tests/checker/fail/<name>.can` + `.stderr` |
| "this program runs end-to-end and prints exactly this" | `tests/runtime/<name>.can` + `.stdout` |
| "this expression produces the right value at runtime" | `tests/canon/<file>_test.can` |
| "this stdlib function does what it claims" | `tests/canon/<file>_test.can` |
| "the parser handles this edge case" | `tests/checker/ok/<name>.can` (it parsed if the checker ran) |
| "a compiler API behaves correctly under unusual input" | `tests/checker_api.rs` |

### Adding a test

- **Checker fixture (ok)**: drop a new `.can` file into `tests/checker/ok/`. The harness picks it up automatically.
- **Checker fixture (fail)**: drop a new `.can` file into `tests/checker/fail/`, then run `just update-fixtures` to generate the sibling `.stderr` from the actual checker output. Review the golden file and commit both.
- **Runtime fixture**: drop a new `.can` file into `tests/runtime/`, then `just update-fixtures` to generate the sibling `.stdout` from the actual program output. Review and commit both.
- **Canon test**: add a function with signature `() => TestResult` to any `tests/canon/*_test.can` file (or create a new one). Discovery is by type signature — the `test*` name prefix is convention, not a requirement. (These are the one place camelCase names remain legal outside binding files: each test needs a distinct non-type name.)
- **Compiler API test**: only when the test needs to call the checker with synthetic arguments. Keep these rare.

### Updating goldens

When an error message or a program's output changes intentionally, run
`just update-fixtures` (mirrors `TRYBUILD=overwrite` from Rust's
`trybuild`). The harness rewrites every `.stderr` and `.stdout` from the
actual current output. The `git diff` is the review surface for
"did this output change in a sensible way?".

### Canon-language test framework

```
testAddPositive = () => TestResult {
    1 -> Sum(2) -> Eq(3) -> TestResult("1+2=3")
}
```

(No import needed: the `TestResult` reference auto-loads the stdlib's `test-result.can`.)

- `TestResult = Fail + Pass`, with `Fail = String` carrying the assertion's failure message.
- The `TestResult` constructor `(Bool * String) => TestResult` turns a `Bool` and a message into a `TestResult`. When the bool is `True`, returns `Pass()`; when `False`, returns `Fail(message)`. (Formerly `assert` — renamed in the types-only port; conversion is construction.)
- The synthesised `main` dispatches each test on its result and prints a `[ ok ] testName` line on `Pass` or a single `[FAIL] testName: message` line on `Fail`. Each dispatch yields 0/1; the failure count drives `wasi:cli/exit#exit-with-code` (any failure → exit 1), so `canon test` is honest to shells and CI.
- Each test ends in a chain that produces a `TestResult` (typically `-> Eq(...) -> TestResult("msg")`). Multi-assertion tests via `?`-propagation are a follow-up that lands when `?` itself learns short-circuit semantics (currently a payload-extractor only).
- The synthesised `main` is exempt from free-function alphabetical ordering (main is the entry point, distinguished by role).
- Exit codes are threaded: a failing suite exits 1, a passing one exits 0 (pinned by `tests/exit_code_test.rs::canon_test_exit_codes`). The `tests/canon_tests.rs` harness still parses stdout for `[FAIL]` as a belt-and-braces check.
- `just test-can` runs the same tests with pretty per-file output (faster local iteration); the canonical CI path is still `cargo test`.

### Examples are not tests

Files in `examples/` exist purely as documentation — readable programs that demonstrate how to use a language feature or stdlib idiom. They are not part of `cargo test`. `just examples` runs them as an optional smoke check ("does the whole pipeline still work end-to-end?"), but they are not a coverage layer. When a language feature needs test coverage, it goes into one of the test layers above; the `examples/` directory follows only when there's something worth demonstrating pedagogically.

Most example-shaped tests (small deterministic programs that exercise one language feature) live in `tests/runtime/` now. `examples/` is reserved for programs that show real-world usage — HTTP servers, file I/O, JSON parsing, randomness — things that intentionally have non-deterministic or environment-dependent output.

### Known codegen gaps

The checker accepts more than the codegen implements. These features
parse and typecheck (some are pinned by `tests/checker/ok/`) but don't
run yet — each is a self-contained PR:

- **Binding declarations returning `list<T>` for non-string `T`** — the
  byte-packed canonical-ABI element layout needs per-width read-back;
  `List<String>` returns already work.
- **Sub-u64 ints (`u8`/`u16`/`u32`/`s8`/`s16`/`s32`) inside a compound
  WIT shape** (`option`/`list`/`variant`/record param). Top-level and
  record-of-scalars *returns* are handled.
- **WIT `result` with no payloads** in binding declarations (the bare
  `result;` form lowers to a discriminant-only shape the codegen renders
  as `u32`).
- **Non-string `option<T>` extern returns** (no indirect-return decode).
- **WIT `resource` / `own<T>` / `borrow<T>` in binding
  signatures** — bindgen emits the resource *types* as `Foo = Handle`
  newtypes but skips every method / constructor / static and any
  function whose signature transitively mentions a handle.
- **`At(i)` / `First` on `List<String>`, nested `Mapped`** —
  `Ty::List` erases the element type at codegen; threading it is the
  enabling refactor.
- **HTTP handler request headers + body** — the handler body compiles
  fully (dynamic status, dispatch, string bodies), and `method()` /
  `path()` land, but reading request *headers* and *body* is not wired
  up. HTTP programs also can't use non-`wasi:http` extern imports (the
  `wasi:http/service` world can't satisfy `canon:builtins/*` bridges).
- **`Stream<T>` lowering + streaming response bodies** — the stdlib
  combinator surface and checker support exist, but codegen drops
  imports whose signatures mention `Stream<T>`, so such programs fail to
  link. The enabling move is routing Stream-using programs through
  `wit_component::ComponentEncoder` instead of the hand-rolled
  `wasm-encoder` type section.

### Gotchas

Non-obvious invariants the code won't spell out for you:

- **Scalar newtypes erase to their underlying primitive at the value
  level.** A stdlib wrapper must declare the primitive receiver, not the
  newtype — the `Exited` constructor dispatches on `Int` (the
  `exitWithCode` binding receiver is `Int`, not `Exit`).
- **`Parallel` / `Race` are methods** (`a -> Parallel(b)`, `a -> Race(b)`),
  never bare calls — Canon has no bare free-function call form anywhere.
- **`Json` and `Html` are prelude types** (`= String` intrinsically). A
  literal with an interpolation hole, or a `.ToJson()` / `.ToHtml()`
  call, auto-loads the stdlib module. Interpolation can't run in the
  `wasi:http/service` world (its host bridge is unsatisfiable there), so
  an interpolating handler fails at build with a clear error.
- **`le` / `ge` is the one comparison spelling** — there is no
  `lte` / `gte`.
- **Product construction is positionless (by type, not slot).**
  `build_product_value` binds each value to the field whose type it
  matches — exact newtype match first (`Value(x)` → the `Value` field),
  then shared base type, then declaration order as a floor
  (`field_match_score` / `widening_chain`). So `canon fmt` sorts a
  product-type constructor's values alphabetically and codegen still
  routes them correctly. Two consequences: (1) same-underlying-type
  fields (map's `Key` and `Value`, both `String`) must be **distinct
  newtypes** and their values tagged to bind unambiguously — the spec's
  "components are distinct types" rule guarantees the field types differ;
  (2) the formatter sorts **only** `Expr::Constructor` product args —
  never `List(…)` (ordered elements) and never method/pipe args
  (`.set(name * value)` is positional). `build_http_response` picks
  `Headers`/`Status` by type and treats the leftover as the body for the
  same reason.
- **Canonical call form: the first input pipes, the rest ride the
  parens.** `canon fmt` rewrites every call to `A -> B(rest)` (the
  `canon_expr` pass in `src/formatter.rs`): `B(A)` → `A -> B`, `B(A * C)`
  → `A -> B(C)`, `A.B(C)` / `A * C -> B` → `A -> B(C)`. Zero-arg calls
  and `List(…)` stay prefix. This is semantics-preserving because the
  compiler treats a piped call to a **type constructor** as construction:
  `compile_method_call` routes any type-name method (product / variant /
  newtype / primitive `Int`/`Float`/`String`/`Bool` / HTTP `Response`)
  through `compile_constructor` — the single construction path — *unless*
  the name is a builtin (`builtin_method_alias`) or has a func-table body
  (a shape / constructor family like `Route`, `TestResult`, keyed on the
  receiver's compiled type). Newtype-wrap (`"hi" -> Greeting`) and
  primitive (`1 -> Int`) piped forms are handled as method-path
  fallbacks. Scalar newtypes erase, so a piped `3000 -> Port` loses
  "Port" on the stack — `static_recv_type` recovers it from the
  receiver's *syntactic* constructor name, and `builtin_result_type` +
  the piped-construction arm of `infer_ctor_arg_type_name` give static
  types to builtin-terminated (`Eq(5)`) and construction (`7 -> Value`)
  chains so family dispatch and by-type product binding still resolve.
  The checker mirrors this: a piped call to a type name is construction
  (`is_piped_construction`; a variant widens to its union; a name with a
  shape body keeps its declared return type).
- **Three codegen encoder modes** (CLI / HTTP / web) each carry a fixed
  import block; adding a defined helper function shifts `fn_user_start`
  in all three. The emitted function/code sections derive from the
  single `compiled_user_funcs` list so key collisions can't
  desynchronize the two section lengths (the old `inconsistent lengths`
  internal error).

### Types-Only Canon

"The only names are type names": camelCase bodied definitions and
camelCase type aliases are **checker errors** outside binding files (the
FFI boundary) and `canon test` functions. See the spec
(`docs/src/spec/`). The invariants:

- **Constructor families.** A type may declare several self-named
  constructors, selected by the first argument's type
  (`() -> Greeting`, `(Bool) -> Greeting`, `(Int) -> Greeting` coexist).
  Codegen routes by first-arg static type walking variant parents +
  alias chains; the zero-arg member owns the `(T, "Self")` func-table
  key; each param component registers a commutative key. Duplicate
  `(receiver, name, first-input)` bodies are a **checked error** — this
  is what closed the old name-collision hazard (defining `button` while
  referencing `Html` now reports `duplicate function …` instead of
  emitting invalid wasm).
- **`=>` declares, `->` executes.** Declarations (constructor/shape
  signatures, lambdas, dispatch arms, function types) use the fat arrow
  `=>` — a `->` at a declaration site is a parse error with a targeted
  message. Execution sites (the postfix pipe) stay `->`-only. The
  legacy `value.( arms )` dispatch, parenthesized arm patterns
  `* (X) =>`, turbofish `::<T>`, and comma-separated declaration params
  are all parse errors now. The endgame retires `.`-method-calls and
  `B(a)` prefix-calls in favour of `->` (with `()` as the construction/
  partial-application operator); see the spec
  (`docs/src/spec/types-only.md` § The One-Operator Endgame).
- **Anonymous arrows.** `(A) => B { … }` at top level declares the `B`
  constructor (return type with `Result`/`Option`/`Future` peeled).
  `FunctionDef.anonymous` drives the formatter to round-trip the arrow
  form. Both the named (`B = (A) => …`) and anonymous forms are legal.
  A **single named input drops its parentheses** — `A => B { … }` is
  exactly `(A) => B { … }` (`Parser::parse_paren_free_ctor`; the
  formatter emits the paren-free form). Products (`(A * B) => C`) and
  generic inputs (`(Some<T>) => C`) keep their parens, so `*`/`<`/`(`
  never open a paren-free arrow.
- **Nullary is `Unit => X`, not `() => X`.** `Unit` is the single-value
  type, so it is the name of "no input": a lone `Unit` parameter
  normalizes to zero params in `resolve_new_syntax` (call sites stay
  `X()` — `Unit` is auto-supplied), and the formatter prints every
  nullary anonymous constructor as `Unit => X`. `()` is no longer a
  declaration-position form; parens appear only to group a product.
- **Entries are anonymous, selected by world-shaped return.** The CLI
  entry is `Unit => Program` (or `Unit => Result<Program, _>`) and the
  HTTP handler `Request => Response` — neither needs a name. `Program`
  (`= Unit`, from `canon/std`) is the CLI world type, the mirror of the
  HTTP `Response`. `resolve_new_syntax` renames an anonymous
  Cli-world-returning entry back to the canonical `main` so entry
  selection, the ordering exemption, and codegen's `$start` inlining all
  still key on `main`; `Unit`/`ExitCode` returns and the literal `main`
  name stay legal (the `canon test` harness synthesizes one). Because
  `Unit` is zero-width and single-valued, all `Unit`-rooted types
  (`Program`, `Exited`, …) are interchangeable in a return position.
  The web-app triple is type-selected too — no names: `Model => Html`
  (view), `Unit => Init` (init), `Model * Msg => Update` (update), where
  `Init` / `Update` are model-alias marker newtypes giving `init` and
  `update` distinct constructor keys. `find_web_entry` anchors on the
  view (the sole non-primitive-receiver `_ => Html`) and returns each
  member's func-table key for codegen.
- **Value-level pipe.** `value -> B` is the call-site mirror of the
  declaration arrow — parsed into a `MethodCall` with `piped: true`,
  semantically identical to `B(value)` / `value.B()`. `-> B?` is the
  pipe plus ordinary postfix `?`.
- **Structural type merge.** Two files declaring the same name with the
  *same* canonical body (`ast::type_expr_canonical`) are one type, not a
  clash — `Length = Int` in both map.can and set.can. The loader
  co-resolves any candidate set whose type declarations share a
  canonical spelling; function-only names always co-resolve. Differing
  bodies still hard-error.
- **Minimal primitives doctrine.** A compiler builtin is justified only
  by wasm numerics, linear-memory layout, canonical-ABI machinery, or a
  host boundary. Everything else is stdlib Canon — `Bool`'s `And`/`Or`/
  `Not` are pure dispatch in `canon/std/bool.can` (the codegen
  `i32.and`/`or`/`eqz` arms are deleted). The stdlib wrapper layer is
  fully ported (JSON/int parsers, HTML element vocabulary, `TestResult`,
  Map/Set); camelCase survives only in binding files
  (`builtins@0.1.0/`, `wasi/`) — the FFI boundary — and `canon test`
  function names, enforced by the checker.
- **Newtype substitutability in returns.** A body producing `Html`
  satisfies `-> Button` where `Button = Html` (the return check walks
  both alias chains). This is what lets tag-newtype constructors return
  the underlying value without self-recursive wrapping.

## Key conventions

- **Alphabetical ordering** is central to the language. If you modify the parser or checker, be aware that sort-order enforcement applies to: product type fields, union variants, function declarations, and dispatch arms. `canon fmt` auto-sorts all of these; the checker errors are the backstop. Union dispatch is also **exhaustive** (no wildcard arm) and duplicate-free; literal dispatch (String/Int) requires a trailing catch-all. Dead code — declarations unreachable from the entry — is a hard checker error, and `cargo test` includes `tests/format_corpus.rs`, which fails if any checked-in `.can` file drifts from canonical format.
- **There is no `use` keyword — imports are automatic.** A reference to a name the file doesn't define resolves name → file: the file's own directory tree (`kebab(Name).can` or `kebab(name)/main.can`, recursive, skipping `deps/`/`bindgen/`), then the project `bindgen/` tree, `deps/`, and the bundled packages — the last three by *declared name* (declaration indexes), because binding functions like `getRandomU64` don't kebab back to their file. Resolving in more than one place is a hard error (no shadowing); resolving nowhere is left for the checker. Inside `canon/std`, wrapper (`src/`) declarations shadow the package's bindgen substrate for outside referrers, and bindgen files prefer bindgen (that's how `filesystem/types.can` gets the clocks `Instant` while user code gets `std`'s). The machinery lives in `src/loader.rs` (`discover_references`, `resolve_reference`, `bundled_decl_matches`). Consequence for bindgen: each generated binding is discovered by the type it constructs, so `canon install` mints a result newtype per function and interface-qualifies it when the WIT leaf name collides across the install set (`MonotonicClockNow` / `SystemClockNow`) — see `binding_return` in `src/bindgen/emit.rs`.
- **Indexing is 1-based** everywhere (`ByteAt(1)`, `list -> At(1)`, positional access `.1`), and `substring(a, b)` is inclusive on both ends. Don't "fix" this to 0-based — it is a deliberate language decision (see the language spec, `docs/src/spec/`).
- The compiler pipeline is: **source → lexer → parser → checker → codegen (wasm core module → Component Model wrapper)**. No external toolchain is invoked — `wasm-encoder` / `wit-component` produce the final `.wasm` in-process, and `canon run` executes it on the embedded wasmtime.
- Standard library is **layered** but ships as a single bundled package, `canon/std`. The package's manifest declares its WIT dependencies under `[imports]`; `canon install` materializes the bindings into `packages/canon/std/bindgen/<ns>/<pkg>@<version>/<iface>.can` (one file per interface, the vendored-package layout). The hand-written wrappers under `packages/canon/std/src/` pipe into the binding constructors by the type each constructs (`GetRandomU64() -> Random`, `Path -> Opened`) and the loader resolves them within the package by declared name (versioned directory), exactly as user code's references resolve against bindings installed into its own `bindgen/` or `deps/` tree. The stdlib's own `canon:builtins/*` host bridges follow the identical shape — string-anchored constructor bindings under `src/canon/builtins@0.1.0/` (`Float => Number { "from-float" }`), idioms (the `File`/`Fetched` constructors, `ToJson`'s `Float` instance) as ordinary bodied wrappers over them; string-processing idioms that once bridged to the host (`Url` validation, JSON escaping/`Field`/`Decoded`, `Now`'s RFC-3339 formatting, `Uppercased`/`Lowercased`) are pure Canon. Where two interfaces collide on a function name (`wasi:clocks` monotonic + system `now`), the generated binding mints an interface-qualified result type (`MonotonicClockNow` / `SystemClockNow`) so discovery resolves on the unique type. There is no privileged shape that only the stdlib can use. The compiler's runtime fulfils the WASI imports through `wasmtime_wasi::p3`.
- The `packages/canon/std/bindgen/` tree is regenerated by `just regen-bindings` (which is just `canon install packages/canon/std`). Don't hand-edit it; bump the vendored WIT and regenerate.
- A binding file is recognized by **shape and path**, never by a header (there is no `bindings` or `package` keyword — the grammar has zero packaging vocabulary). A file directly under a versioned package directory (`<ns>/<name>@<ver>/<iface>.can`, under `deps/`, a project's `bindgen/`, or inside a bundled package) has its bindings lifted into externs by `apply_bindings` in `src/loader.rs`. **Preferred form (types-only): a string-anchored anonymous constructor** — `Float => Number { "from-float" }`, `File => Result<Read, IoError> { "read" }`. The single string-literal body is the WIT fragment verbatim (`"from-float"`, `"[method]fields.set"`); `apply_bindings` sets `extern_wasm.path = "<urn>#<string>"`, unwraps a `Future<T>` return for async, and normalises the constructor into a `Self`-constructor (registering the commutative `(Param, Type)` key that makes `x -> Type` dispatch to the extern). Because a binding is discovered by the type it *constructs*, `canon install` mints a result newtype per function (`Now = Instant`, `GetRandomU64 = Int`), interface-qualified when the WIT leaf name collides (`MonotonicClockNow` / `SystemClockNow`) — this replaces the old capability-marker-method trick. **Legacy form (still supported):** a camelCase body-less function-type alias (`getRandomU64 = () => Int`) rewritten to `extern_wasm.path = "…#<fn-kebab>"`. The generated bindgen tree keeps the legacy form only for return shapes the self-constructor extern lowering can't decode yet — no result (`Unit`), `option`, `list`, and record returns; everything else (scalars, `string`, `result`) takes the new form. Resource fragments derive from shape in the legacy path: a camelCase decl whose first param is an in-file `X = Handle` newtype binds `[method]x.<fn>`; a PascalCase decl named like an in-file resource binds `[constructor]x`. Other PascalCase function-type aliases stay callback types everywhere.
- Renames don't exist at the binding layer: kebab↔camelCase round-trips, so a WIT name that doesn't match the desired Canon idiom gets a raw binding under its mechanical name plus an ordinary bodied wrapper (or, for `canon:builtins/*` where Canon owns the host, the host function is renamed to match). Scalar-newtype receivers must be declared as bare `Int` in binding files — scalar newtypes erase to `Int` at the value level (the `exitWithCode` receiver is `Int`, not `Exit`).
- Each `bindgen/` directory also contains an `_install.toml` sidecar written by `canon install`: a map from `<rel-path>.can` to the WIT interface URN that file was generated from. It's a derived artifact used only for install staleness detection now (the loader derives URNs from paths); committed for `canon/std`, gitignored for user projects. Parser: `src/install.rs::parse_install_index`.
- Manifest schema: `[deps]` declares Canon-package dependencies (`"name" = "version"`), `[imports]` declares external bindings (`"<path-prefix>" = "<source>"` where source is a local `.wit` file, a directory of `.wit` files, or a `.wasm` component; remote sources are deferred). Both tables are alphabetical. See `src/manifest.rs` for the parser and `src/install.rs` for the install logic.
- `build.rs` walks `packages/` at build time and emits a bundled-package registry the loader consults at runtime. Both `src/` and `bindgen/` under each package contribute files; `rel_path` is taken relative to whichever root the file lived under, so they share a flat namespace. Collisions between the two roots panic at build time. Drop a new file under `packages/<ns>/<pkg>/` and the next `cargo build` picks it up — there is no hand-maintained STDLIB array.
- Example programs in `examples/` should always compile and run after changes — use `just examples` to verify.

## Code style

- Rust code follows standard `rustfmt` formatting (`just fmt`).
- Keep `clippy` clean (`just clippy`).
- Dependencies are limited to the Bytecode Alliance wasm toolchain (`wasm-encoder`, `wit-parser`, `wit-component`, `wasmparser`), the embedded runtime (`wasmtime`, `wasmtime-wasi`, `wasmtime-wasi-http`, `tokio`), and the hyper HTTP stack for `canon run --addr`. Don't add dependencies outside that orbit.

## Canon language quick reference

```
Bool = False + True                            # union
User = Birthday * Username                     # product

Greeting => Loud {                             # constructor (paren-free single input)
    Greeting -> Joined("!")
}

Unit => Loud {                                 # nullary constructor (Unit = "no input")
    "HELLO"
}

Unit => Program {                              # CLI entry (anonymous, returns the Program world)
    "hello" -> Print
}

True() -> (                                    # dispatch (branch on union); scrutinee pipes in with `->`
    * False => Unit { "no" -> Print }
    * True  => Unit { "yes" -> Print }
)

path -> (                                      # literal dispatch (String/Int scrutinee);
    * "/notes" => Body { Index() }             # the catch-all arm is required, always last
    * String => Body { NotFound() }
)

List(1 * 2 * 3) -> Mapped((Int) => Int { Int -> Product(2) })  # lambda (keeps parens)

Model => Html {                                # HTML literal ({…} interpolates;
    <div><span>{Model -> String}</span></div>  # String/Int escape, Html passes through)
}

`count is {Int}, doubled {Int -> Product(2)}`  # format string (backtick + {expr} holes;
                                               # holes convert via `-> String`, {{ }} escape
                                               # braces; plain "..." stays inert)
```

- No local variables, no `let`, no `if`/`else`, no comments in the language.
- There are no imports: referencing `Foo` loads `foo.can` from the file's directory tree (or `foo/main.can`), then `bindgen/`, `deps/`, and the bundled stdlib by declared name. Ambiguity is a hard error.
- See the language spec under `docs/src/spec/` for the complete specification.
