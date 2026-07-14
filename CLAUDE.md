# CLAUDE.md — Agent Context for Canon

## Working with Claude on Canon

Canon is small on purpose. Treat entropy as the enemy and subtraction as the
default. These are rules, not moods — hold to them without being re-asked:

- **Prefer net-negative diffs.** When a change adds more than it removes, say
  so and justify why the addition earns its place. Fewer concepts beats more.
- **Replacing means deleting.** No legacy shims, no deprecated-but-kept code or
  docs, no "for now" branches, no dead parameters. Remove the old thing in the
  same change that introduces the new one.
- **Don't add what wasn't asked for** — no speculative features, options,
  abstractions, or docs. If you think one is warranted, propose it in one
  sentence and wait; don't build it.
- **End substantial work with a subtraction pass** (`/simplify`): what did I
  add that could be removed while keeping behaviour? Report the result, even if
  it's "nothing."
- **Delete aggressively, but confirm before removing what you can't prove is
  dead.** The compiler's dead-code check makes deletion safe inside `.can`; for
  everything else, if you can't tell it's unused, ask.
- **Match the surrounding style** — comment density, naming, idiom. New code
  should be indistinguishable from old.

The language itself is the model: *wherever choice is discretionary, remove the
concept.* Apply that to the code, the docs, and this file.

## What is this project?

Canon is a programming language whose reference compiler (written in Rust)
emits **WebAssembly components** directly. Small surface area — no `let`, no
`if`/`else`, no comments, no local variables, no import statement (references
resolve to files automatically). Branching is dispatch on a union; effects are
passed as capabilities. Guiding rule: **wherever ordering is discretionary, the
compiler enforces alphabetical order.**

The full specification lives under `docs/src/spec/` — the canonical design doc.

**The docs site is itself a Canon program.** `docs/src/main.can` is an
Elm-architecture web app that renders the `docs/src/*.md` pages via the stdlib
Markdown renderer, compiled by `canon build docs` to wasm (dogfoods the web
target — see `docs/src/reference/web-target.md`). Page content stays plain
Markdown; `main.can` (routing/sidebar) and `styles.can` (stylesheet) are the app
shell.

**Root-doc policy:** the repo root holds no design/planning markdown. This file
is the only root doc, alongside `README.md` and the standard community-health
files. Add new documentation as a page under `docs/src/` (and a route in
`docs/src/main.can`) — never a new root `*.md`.

## Repository layout

| Path | Description |
|---|---|
| `.github/` | CI workflows (docs deploy, release pipeline) |
| `docs/` | Documentation site. `src/main.can` + `src/styles.can` are a Canon web app (Elm triple) rendering `src/*.md` via the stdlib Markdown renderer; `canon build docs` → `docs/build/` (wasm + `canon-web.js` host + `index.html`). `assets/docs-enhance.js` is injected by the deploy for progressive enhancement (highlighting, click-to-run). `runner/build.mjs` compiles each ` ```canon,run=… ` snippet to a jco bundle at build time. `landing/index.html` is the marketing page. Deploy puts landing at root, app under `/doc/`. |
| `src/` | Compiler source (Rust) |
| `src/lexer/` | Tokenization (`scanner.rs`, `token.rs`) |
| `src/parser/` | AST construction (`parser.rs`) |
| `src/checker/` | Type checker + sort-order validation |
| `src/codegen/` | WebAssembly component gen (`wasm/mod.rs`, `wasm/component.rs`, `async_analysis.rs`) |
| `src/ast.rs` | AST node definitions |
| `src/error.rs` | Error types and spans |
| `src/loader.rs` | File/module loading + reference resolution |
| `src/bindgen/` | WIT → Canon source emitter (`naming.rs`, `emit.rs`, `mod.rs`) |
| `src/main.rs` | CLI entry (`run`, `build`, `check` (`--fix`), `test`, `inspect`, `bindgen`, `install`, `publish`, `lsp`, `upgrade`, `use`) |
| `src/webhost.rs` | Web target's browser side — generated JS host, `index.html` shell, static server for `canon run` |
| `src/lib.rs` | Public crate modules |
| `build.rs` | Walks `packages/` → bundled-package registry baked into the binary |
| `packages/canon/std/` | The standard library — one shipped package. Hand-written wrappers under `src/`, WIT-derived bindings under `bindgen/` (committed), vendored upstream WIT under `wit/` (the import declaration — no manifest). |
| `packages/canon/std/bindgen/` | Generated WASI bindings (`wasi/<pkg>@<ver>/<iface>.can`), from `just regen-bindings`. Derived — never hand-edit. A same-`rel_path` file under `src/` shadows its `bindgen/` twin. |
| `packages/canon/std/wit/wasi/` | Vendored upstream WIT — source for the bindings. Bumped when WASI advances. |
| `examples/` | Example `.can` programs |
| `githooks/` | Git hooks (`pre-commit`) |
| `tests/` | Rust integration tests (incl. `tests/fixtures/`, `tests/canon/`) |
| `editors/` | Tree-sitter grammar, Zed + VS Code extensions (runbook in `editors/PUBLISHING.md`) |
| `install.sh` | Installer for prebuilt binaries |

## Build & dev commands

`just` is the task runner over `cargo`:

```sh
just build              # cargo build (debug)
just install            # cargo install --path . --force
just test               # cargo test (the canonical test command; CI runs this)
just test-can           # run every tests/canon/*_test.can via `canon test`
just update-fixtures    # regenerate golden .stderr/.stdout files
just examples           # compile + run all examples
just example <name>     # run a single example
just bench              # benchmark codegen::generate()
just docs               # build + serve docs on 127.0.0.1:8080
just regen-bindings     # regenerate packages/canon/std/bindgen/ from wit/
just fmt / fmt-can      # rustfmt / canonicalize every corpus .can file
just clippy             # cargo clippy -- -W warnings
just ci                 # fmt + clippy + test (mirrors CI)
just clean              # cargo clean + remove compiled examples
just install-hooks      # install git pre-commit hook
just build-extension / build-vscode-extension  # editor extensions
```

**Releases.** Two channels, driven by tags; `Cargo.toml`'s version is stamped in
CI, never committed. **No release workflow ever pushes to `main`** (the `main`
ruleset requires the `ci` check; a bot version-bump push is rejected `GH013`).

- **nightly** — every push to `main` (except docs/editors/markdown-only) builds
  cross-platform binaries and updates a rolling `nightly` prerelease tag.
- **stable** — the **promote** workflow (`Actions → promote`) tags a commit
  `vX.Y.Z` (bump = patch/minor/major from existing `v*` tags) as the Latest
  release. GitHub's `/releases/latest` ignores prereleases, so stable never
  picks up a nightly.

Both call the reusable `release.yml`. Pushes touching `editors/vscode-canon/`
run **publish-vscode-extension** instead (needs `VSCE_PAT`/`OVSX_PAT`).

**Toolchains (client side).** One install holds both channels under
`$CANON_INSTALL/toolchains/<channel>/canon` (`$CANON_INSTALL` = `~/.canon`);
`$CANON_INSTALL/bin/canon` on `PATH` is a thin **launcher** (a copy of a real
toolchain). Two concepts only: (1) **`canon use [stable|nightly]`**, scoped by
cwd, recorded in `$CANON_INSTALL/uses` (longest-prefix wins; a `use` at `$HOME`
is the global default; no project config file — the language has no `canon.toml`
either); (2) **`canon stable|nightly <cmd>`** one-shot. Resolution: explicit word
→ `CANON_TOOLCHAIN` env → nearest `use` → `stable`. `canon upgrade` updates the
active toolchain; `install.sh` reads `CANON_CHANNEL`. Machinery: `toolchain::`
in `src/main.rs`.

## Testing

One entry point: **`cargo test`** (or `just test`). Every layer is a `tests/*.rs`
integration binary, so one command runs all and fails on any regression. CI runs
nothing else.

```
tests/
  checker/ok/<name>.can        # must check with zero errors
  checker/fail/<name>.can      # must produce errors
  checker/fail/<name>.stderr   # golden: exact expected stderr
  runtime/<name>.can           # must run to completion (exit 0)
  runtime/<name>.stdout        # golden: exact captured stdout
  canon/<name>_test.can        # `X = TestResult` newtypes + `Unit => X` ctors
  common/mod.rs                # shared harness helpers
  *_fixtures.rs / canon_tests.rs / checker_api.rs  # per-layer harnesses
```

Pick the layer by **what the test observes**:

| To test… | Use… |
|---|---|
| checker accepts this source | `tests/checker/ok/<name>.can` |
| checker rejects with a specific error | `tests/checker/fail/<name>.can` + `.stderr` |
| program runs end-to-end, prints exactly this | `tests/runtime/<name>.can` + `.stdout` |
| an expression / stdlib fn produces the right value | `tests/canon/<file>_test.can` |
| the parser handles an edge case | `tests/checker/ok/<name>.can` |
| a compiler API under unusual input | `tests/checker_api.rs` (keep rare) |

**Adding a test:** drop the `.can` file in the right dir — the harness discovers
it. For `fail/` and `runtime/`, run `just update-fixtures` to generate the
sibling golden, then review and commit both. For a canon test, add a
`TestResult` newtype + its `Unit => X` constructor to any `tests/canon/*_test.can`
(discovery is by that shape; the name is a type named for the behaviour it
asserts, reported `[ ok ] SumAddsOperands`).

**Updating goldens:** when output changes intentionally, `just update-fixtures`
rewrites every `.stderr`/`.stdout`; the `git diff` is your review surface.

**Canon test framework:**

```
SumAddsOperands = TestResult

Unit => SumAddsOperands {
    1 -> Sum(2) -> Eq(3) -> TestResult
}
```

- `TestResult = Fail + Pass`; `Fail = String` carries an optional message. The
  `Bool => TestResult` constructor maps `True`→`Pass()`, `False`→`Fail("")`. A
  bare `-> TestResult` ends a boolean chain (the test's name is the label). For a
  real diagnostic, construct `Fail("why")` in a dispatch arm.
- The synthesised `main` dispatches each test, prints `[ ok ] Name` / `[FAIL]
  Name: msg`, and any failure drives `exit-with-code` → exit 1 (pinned by
  `tests/exit_code_test.rs`). It's exempt from alphabetical ordering.
- `canon test <dir>` runs every `*_test.can` in **one process** (shared stdlib
  parse + one wasmtime engine); `<file>` keeps the single-file path. A per-file
  compile failure is isolated. `just test-can` is the local-iteration view.

**Examples are not tests.** `examples/` holds readable programs that demonstrate
real-world usage (HTTP servers, file I/O, JSON, randomness — non-deterministic by
nature). They're a `just examples` smoke check, not a coverage layer. Small
deterministic feature tests go in `tests/runtime/`.

**Known codegen gaps.** The checker accepts more than codegen implements. The
canonical list is `docs/src/reference/codegen-gaps.md`, mirrored by `CODEGEN_GAPS`
in `src/checker/mod.rs` (pinned together by `tests/codegen_gaps.rs`). Reaching a
statically-detectable gap emits a non-fatal warning. Add new gaps in both places,
not here.

## Language invariants

These are the non-obvious rules the code won't spell out. Together with
**Types-Only Canon** below, they're the backstop behind `canon check --fix`.

- **Scalar newtypes erase to their underlying primitive.** A wrapper/binding
  declares the primitive receiver, not the newtype (the `exitWithCode` receiver
  is `Int`, not `Exit`).
- **`Parallel` / `Race` are methods** (`a -> Parallel(b)`), never bare calls —
  Canon has no bare free-function call form.
- **`Json` / `Html` are prelude types** (`= String`). A literal with an
  interpolation hole auto-loads the stdlib module; a hole lowers to a piped
  construction through a stdlib family — JSON `-> Encoded` (`Encoded = Json`),
  HTML `-> Escaped` (`Escaped = Html`), format-string `-> String`. Interpolation can't run in the `wasi:http/service` world (host bridge
  unsatisfiable) — an interpolating handler fails at build. `Json("…")`/`Html("…")`
  fed a static literal the literal form can express is a checker error
  (`check_literal_form_ceremony`): the validating constructor is for runtime-built
  strings.
- **`le` / `ge` is the one comparison spelling** — no `lte`/`gte`.
- **Product construction is positionless (by type, not slot).**
  `build_product_value` binds each value to the field whose type it matches
  (exact newtype first, then shared base, then declaration order as a floor). So
  `canon check --fix` may sort a constructor's inputs and codegen still routes
  them — **but only when every input carries its type syntactically**. Literal
  operands are NEVER reordered (`Padded(5 * 4)` ≠ `Padded(4 * 5)`). Consequences:
  same-underlying-type fields (map's `Key`/`Value`, both `String`) must be
  distinct newtypes with tagged values; the formatter never reorders `List(…)` or
  method/pipe args.
- **Canonical call form: values flow through pipes, literals are born in the
  parens.** `canon check --fix` (`canon_expr` in `src/formatter.rs`) rewrites
  every call: computed first input pipes (`B(A)` → `A -> B`); a lone scalar
  literal never pipes into a construction (`"hi" -> Greeting` → `Greeting("hi")`);
  a same-kind primitive wrap unwraps (`Int(3)` → `3`); builtins
  (`is_builtin_pipe_vocabulary` in `src/ast.rs`) have no prefix form so literals
  keep piping; multi-input calls keep the pipe; zero-arg and `List(…)` stay
  prefix. A `Joined` chain containing literal text folds into a backtick format
  string (`fold_joined_chain`) — all-computed chains stay pipes (`Joined` is
  also list concat; only literal text proves strings). An interpolation hole
  that overflows its line breaks onto indented lines (`emit_base_at` /
  `LitWriter` in `src/formatter.rs`); static literal text is content and never
  moves. Semantics-preserving because `compile_method_call` routes any type-name
  method through the single `compile_constructor` path unless the name is a
  builtin or has a func-table body. Scalar newtypes erase, so `static_recv_type`
  recovers the type from the syntactic constructor name. The checker mirrors this
  (`is_piped_construction`).
- **Three codegen encoder modes** (CLI / HTTP / web) each carry a fixed import
  block; adding a defined helper shifts `fn_user_start` in all three. Both emitted
  sections derive from the single `compiled_user_funcs` list so lengths can't
  desync.

### Types-Only Canon

"The only names are type names": camelCase bodied definitions and camelCase type
aliases are checker errors outside binding files (the FFI boundary) — no test
exception (tests are `TestResult` newtypes with anonymous constructors). Full
treatment in `docs/src/spec/types-only.md`.

- **Constructor families.** A type may declare several self-named constructors,
  selected by the first argument's type (`Unit => Greeting`, `Bool => Greeting`,
  `Int => Greeting` coexist). Duplicate `(receiver, name, first-input)` bodies are
  a checked error.
- **`=>` declares, `->` executes.** Declaration sites (constructor/shape sigs,
  lambdas, dispatch arms, function types) use `=>`; a `->` there is a parse error.
  Execution (the postfix pipe) is `->`-only. Legacy `value.( arms )`, `* (X) =>`,
  `::<T>`, and comma params are parse errors. `.`-method-calls survive only for
  camelCase FFI bindings; `B(a)` prefix survives only where the literals-in-parens
  rule puts it. **No implicit dependency threading:** an omitted argument is a
  missing-argument error even when exactly one in-scope value matches.
- **Anonymous arrows are the one constructor form.** `(A) => B { … }` declares the
  `B` constructor (return type with `Result`/`Option`/`Future` peeled). The named
  spelling `B = (A) => …` still parses but `--fix` rewrites it when the name is
  exactly the constructed type; named declarations survive only as shape impls.
  Checked: a receiver-less bodied decl must be named after the type it constructs;
  a receiver-carrying one must be a declared shape or a newtype of its return; an
  arrow whose constructed type appears in its own input is an error (endomorphisms
  take result newtypes, `Inserted = Map`). **Shapes are rejected outright** —
  no exceptions; the interpolation hooks are ordinary result-newtype families. A single
  named input drops its parens (`A => B { … }` == `(A) => B { … }`); products and
  generic inputs keep them.
- **Nullary is `Unit => X`, not `() => X`.** `Unit` is the single-value type — the
  name of "no input". A lone `Unit` param normalizes to zero params; call sites
  stay `X()`. `()` is not a declaration form.
- **Entries are anonymous, selected by world-shaped return.** CLI entry is
  `Unit => Program` (`Program = Unit` from `canon/std`); HTTP handler is
  `Request => Response`. `resolve_new_syntax` renames a Cli-world-returning entry
  back to `main` (so ordering exemption + `$start` inlining key on it); a literal
  `main` name is a checker error. All `Unit`-rooted types are interchangeable in
  return position. The web triple is type-selected too: `Model => Html` (view),
  `Unit => Init`, `Model * Msg => Update` (`Init`/`Update` are marker newtypes for
  distinct keys).
- **Value-level pipe.** `value -> B` == `B(value)` == `value.B()` (a `MethodCall`
  with `piped: true`); `-> B?` is the pipe plus postfix `?`.
- **Structural type merge.** Two files declaring the same name with the same
  canonical body (`ast::type_expr_canonical`) are one type (`Length = Int` in both
  map.can and set.can); differing bodies hard-error.
- **Minimal primitives doctrine.** A builtin is justified only by wasm numerics,
  linear-memory layout, canonical-ABI machinery, or a host boundary. Everything
  else is stdlib Canon — `Bool`'s `And`/`Or`/`Not` are pure dispatch in
  `bool.can`. camelCase survives only in binding files.
- **Newtype substitutability in returns.** A body producing `Html` satisfies
  `-> Button` where `Button = Html` (return check walks both alias chains).

## Key conventions

- **Alphabetical ordering** applies to product fields, union variants, function
  declarations, and dispatch arms. `--fix` auto-sorts; checker errors are the
  backstop. Union dispatch is exhaustive (no wildcard) and duplicate-free; literal
  dispatch (String/Int) requires a trailing catch-all. Dead code (declarations
  unreachable from the entry) is a hard error, and `tests/format_corpus.rs` fails
  if any checked-in `.can` drifts from canonical format.
- **No `use` keyword — imports are automatic.** A reference resolves name → file:
  the file's own directory tree (`kebab(Name).can` or `kebab(name)/main.can`,
  recursive, skipping `deps/`/`bindgen/`), then the project `bindgen/`, `deps/`,
  and bundled packages by declared name. Resolving in >1 place is a hard error (no
  shadowing); nowhere is left to the checker. Machinery: `src/loader.rs`.
- **Indexing is 1-based** everywhere (`ByteAt(1)`, `-> At(1)`, `.1`);
  `substring(a, b)` is inclusive both ends. Deliberate — don't "fix" to 0-based.
- **Pipeline:** source → lexer → parser → checker (format phase + semantics) →
  codegen (wasm core → Component Model wrapper). Formatting is a compiler phase:
  `check_loaded` diffs each source against its canonical rendering (divergence =
  `CanonError::FormatError`) fused with the semantic checker. `wasm-encoder` /
  `wit-component` produce the `.wasm` in-process; `canon run` executes it on
  embedded wasmtime.
- **Standard library** is layered but ships as one bundled package, `canon/std`.
  It declares WIT deps by vendoring under `wit/`; `canon install` materializes
  bindings into `bindgen/<ns>/<pkg>@<ver>/<iface>.can`. Hand-written wrappers under
  `src/` pipe into binding constructors by the type each constructs
  (`GetRandomU64() -> Random`). String-processing idioms (`Url` validation, JSON
  escaping, RFC-3339 `Now`, case mapping) are pure Canon. The runtime fulfils WASI
  through `wasmtime_wasi::p3`. Regenerate with `just regen-bindings`; never
  hand-edit `bindgen/`.
- **Binding files are recognized by shape and path**, never a header (the grammar
  has no packaging keyword). A file directly under a versioned package dir has its
  bindings lifted into externs by `apply_bindings` in `src/loader.rs`. Preferred
  form: a string-anchored anonymous constructor — `Float => Number { "from-float" }`
  — whose single string body is the WIT fragment verbatim; `apply_bindings` sets
  `extern_wasm.path = "<urn>#<string>"`, unwraps `Future<T>` for async, and mints
  a `Self`-constructor. Each binding is discovered by the type it constructs, so
  `canon install` mints a result newtype per function (`Now = Instant`), interface-
  qualified on WIT leaf-name collisions (`MonotonicClockNow`/`SystemClockNow`); a
  no-result function mints a `Unit` newtype. The camelCase alias form survives only
  in hand-written binding files for shapes the string form can't cover yet:
  resource methods (`wasi/http`'s `types.can`) and generic combinators
  (`canon:builtins`' `concurrent.can`/`stream.can`). Scalar-newtype receivers are
  declared as bare `Int`.
- **No package manifest** — file structure is the whole declaration. A **package**
  is a dir with `src/main.can` (name = dir name; artifacts in its `build/`); a
  **workspace** is a dir whose immediate subdirs include packages. **External
  imports** = the `wit/` dir; **dependencies** = `deps/<ns>/<name>@<ver>/`. The
  **project root** is the nearest ancestor with a structural marker (`src/main.can`,
  `wit/`, `bindgen/`, `deps/`) — `src/install.rs`. Each `bindgen/` has an
  `_install.toml` sidecar (staleness detection only; committed for `canon/std`,
  gitignored for user projects).
- `build.rs` walks `packages/` at build time into a bundled-package registry; drop
  a file under `packages/<ns>/<pkg>/` and the next `cargo build` picks it up.
- Examples must compile and run after changes — `just examples` to verify.

## Code style

- Rust follows `rustfmt` (`just fmt`); keep `clippy` clean (`just clippy`).
- Dependencies are limited to the Bytecode Alliance wasm toolchain
  (`wasm-encoder`, `wit-parser`, `wit-component`, `wasmparser`), the embedded
  runtime (`wasmtime`, `wasmtime-wasi`, `wasmtime-wasi-http`, `tokio`), and the
  hyper HTTP stack for `canon run --addr`. Don't add dependencies outside that
  orbit.

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
- No imports: referencing `Foo` loads `foo.can` from the file's directory tree,
  then `bindgen/`, `deps/`, and the bundled stdlib by declared name. Ambiguity is
  a hard error.
- See the language spec under `docs/src/spec/` for the complete specification.
</content>
</invoke>
