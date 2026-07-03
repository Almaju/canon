# CLAUDE.md — Agent Context for Canon

## What is this project?

Canon is a programming language whose reference compiler emits **WebAssembly components** directly (not transpiled to Rust — that was the original prototype). The compiler is itself written in Rust. Canon presents a small surface area — no `let`, no `if`/`else`, no comments, no local variables. Branching is dispatch on a union. Effects are passed as capabilities. The guiding rule: **wherever ordering is discretionary, the compiler enforces alphabetical order**.

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
| `src/codegen/` | Code generation — WebAssembly components (`wasm/mod.rs` core module, `wasm/component.rs` component wrapper, `async_analysis.rs`) |
| `src/ast.rs` | AST node definitions |
| `src/error.rs` | Error types and spans |
| `src/loader.rs` | File/module loading |
| `src/bindgen/` | `canon bindgen` — WIT → Canon source emitter (`naming.rs`, `emit.rs`, `mod.rs`) |
| `src/main.rs` | CLI entry point (`run`, `build`, `check`, `test`, `fmt`, `inspect`, `bindgen`, `lsp`, `upgrade`) |
| `src/webhost.rs` | Web target's browser side — the generated JS host (`canon-web.js`), `index.html` shell, bundle writer, static server for `canon run` (see `WEB-TARGET.md`) |
| `src/lib.rs` | Public crate modules |
| `src/manifest.rs` | `canon.toml` parser (TOML subset, hand-written) |
| `build.rs` | Walks `packages/` and emits a bundled-package registry baked into the compiler binary |
| `packages/canon/std/` | The standard library — one shipped package. Contains hand-written wrappers under `src/`, WIT-derived bindings under `bindgen/` (committed), and an `canon.toml` declaring its WIT imports under `[imports]`. The loader resolves `use canon/std/X` against this tree. |
| `packages/canon/std/bindgen/` | Generated WASI bindings (produced by `just regen-bindings`, which runs `canon install packages/canon/std`). Committed so `cargo build` works on a fresh clone; treated as a derived artifact — never hand-edited. |
| `wit-vendor/wasi/` | Vendored upstream WIT files — source for `packages/canon/std/bindgen/`. Bumped when WASI advances. |
| `examples/` | Example `.can` programs |
| `githooks/` | Git hooks (`pre-commit`) |
| `tests/` | Rust integration tests (incl. `tests/fixtures/` & `tests/canon/`) |
| `editors/` | Tree-sitter grammar and Zed extension |
| `install.sh` | Installer script for prebuilt binaries |
| `WEB-TARGET.md` | The web target — Elm-triple entry, browser ABI, JS host conventions |
| `DESIGN.md` | Language specification — the source of truth for language semantics |
| `README.md` | Project README |

## Build & dev commands

This project uses [`just`](https://github.com/casey/just) as a task runner and standard `cargo` underneath.

```sh
just build              # cargo build (debug)
just install            # cargo install --path . --force (release → ~/.cargo/bin)
just test               # cargo test (Rust unit/integration tests for the compiler)
just test-ow            # run every tests/canon/*_test.can file via `canon test`
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
    ok/<name>.can              # must check with zero errors
    fail/<name>.can            # must produce errors
    fail/<name>.stderr        # golden: exact expected stderr
  runtime/
    <name>.can                 # must run to completion (exit 0)
    <name>.stdout             # golden: exact captured stdout
  canon/
    <name>_test.can            # functions with signature `() -> TestResult`
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
- **Canon test**: add a function with signature `() -> TestResult` to any `tests/canon/*_test.can` file (or create a new one). Discovery is by type signature — the `test*` name prefix is convention, not a requirement.
- **Compiler API test**: only when the test needs to call the checker with synthetic arguments. Keep these rare.

### Updating goldens

When an error message or a program's output changes intentionally, run
`just update-fixtures` (mirrors `TRYBUILD=overwrite` from Rust's
`trybuild`). The harness rewrites every `.stderr` and `.stdout` from the
actual current output. The `git diff` is the review surface for
"did this output change in a sensible way?".

### Canon-language test framework

```
use std/TestResult

testAddPositive = () -> TestResult {
    1.add(2).eq(3).assert()
}
```

- `TestResult = Fail + Pass`, with `Fail = String` carrying the assertion's failure message.
- `assert = (Bool * String) -> TestResult` turns a `Bool` and a message into a `TestResult`. When the bool is `True`, returns `Pass()`; when `False`, returns `Fail(message)`.
- The synthesised `main` dispatches each test on its result and prints a `[ ok ] testName` line on `Pass` or a single `[FAIL] testName: message` line on `Fail`. Each dispatch yields 0/1; the failure count drives `wasi:cli/exit#exit-with-code` (any failure → exit 1), so `canon test` is honest to shells and CI.
- Each test ends in a chain that produces a `TestResult` (typically `.eq(...).assert()`). Multi-assertion tests via `?`-propagation are a follow-up that lands when `?` itself learns short-circuit semantics (currently a payload-extractor only).
- The synthesised `main` is exempt from free-function alphabetical ordering (main is the entry point, distinguished by role).
- Exit codes are threaded: a failing suite exits 1, a passing one exits 0 (pinned by `tests/exit_code_test.rs::canon_test_exit_codes`). The `tests/canon_tests.rs` harness still parses stdout for `[FAIL]` as a belt-and-braces check.
- `just test-ow` runs the same tests with pretty per-file output (faster local iteration); the canonical CI path is still `cargo test`.

### Examples are not tests

Files in `examples/` exist purely as documentation — readable programs that demonstrate how to use a language feature or stdlib idiom. They are not part of `cargo test`. `just examples` runs them as an optional smoke check ("does the whole pipeline still work end-to-end?"), but they are not a coverage layer. When a language feature needs test coverage, it goes into one of the test layers above; the `examples/` directory follows only when there's something worth demonstrating pedagogically.

Most example-shaped tests (small deterministic programs that exercise one language feature) live in `tests/runtime/` now. `examples/` is reserved for programs that show real-world usage — HTTP servers, file I/O, JSON parsing, randomness — things that intentionally have non-deterministic or environment-dependent output.

### Known codegen gaps (test-visible)

The checker accepts more than the codegen currently implements. Each item below is a feature whose *syntax* and *types* are pinned by fixtures in `tests/checker/ok/`, but whose *runtime* behaviour isn't ready yet. Pick any of these up as a self-contained PR.

| Gap | Symptom | Where it bites |
|---|---|---|
| **`use` ordering check unreachable** | The "use must come first" check in the checker is dead code under the loader path | Cosmetic; loader strips uses before checking |
| **`extern Wasm` functions returning `list<T>` for non-string `T`** | `list<string>` returns work (see the closed-gap row on indirect-return shapes below), but other element types (e.g. `list<u8>` from `wasi:random#get-random-bytes`) are byte-packed at the canonical ABI while Canon's `List` uses a uniform 8-byte stride — the decode needs per-width element read-back. | `canon bindgen` still skips non-string list returns. Same enabling refactor as `List<String>.get`: element-type tracking through the list machinery. |
| **Sub-u64 integers inside compound WIT shapes (non-record)** | Narrow ints at top level and inside record-of-scalars *returns* are handled (closed-gap rows below), but a narrow int inside an `option`/`list`/`variant`/record-*param* still rides on the unsupported compound shape. | `canon bindgen` skips such functions with `"sub-u64 integer inside a compound shape"`. Mostly affects the sockets surface and record parameters. |
| **WIT `result` with no `ok`/`err` payloads in `extern Wasm`** | The bare `result;` form lowers to a discriminant-only canonical-ABI shape that the codegen renders as `u32`, mismatched with the host's `result` shape. Even unused imports fail validation. | `canon bindgen` skips affected functions (e.g. `wasi:cli/exit#exit`). Same codegen origin as the narrow-int gap. |
| **Non-string `option<T>` returned from `extern Wasm`** | `option<string>` and record-of-scalars returns work (closed-gap rows below), but options over non-string payloads still have no indirect-return decode. | Same pattern as the existing `IndirectReturnShape` variants when needed. |
| **WIT `resource` / `own<T>` / `borrow<T>` in `extern Wasm` signatures** | The canonical-ABI requires a `(type $Foo (resource (rep i32)))` declaration and `(own $Foo)` / `(borrow $Foo)` parameter/return shapes; the current extern-import lowering only knows scalar `PrimitiveValType`s and strings. Even unused imports fail validation. | `canon bindgen` emits the resource *types* as `Foo = Handle` newtypes (see DESIGN.md §Resources), but skips every resource method / constructor / static, and skips any free function whose signature transitively mentions a handle, with reason `"... (codegen lowering pending, see CLAUDE.md)"`. This drops the entire `wasi:filesystem/types#descriptor` method surface and the `wasi:sockets/*` resources. Fix in `src/codegen/wasm/mod.rs` extern-import lowering: declare resource types in the component type section, extend `ParamKind` with `OwnedResource` / `BorrowedResource` variants, and emit ownership-tagged params/returns. Linear-use checking belongs in a separate checker pass once lowering works (slice 4 in the resources plan). |
| **`list.get(i)` on `List<String>`, `list.first` beyond `Int`, nested `.map`** | `list.map(f)` now really applies `f` (see the closed-gap row below) and `list.get(i)` works for `Int`-slot elements, but `get`/`first` still read the 8-byte slot as one i64 — on a `List<String>` the `Some` payload comes back type-confused. And a `.map` nested inside another `.map`'s lambda body clobbers the outer element binding (`map_elem_i64` / `map_elem_ptr` are single dedicated locals). | `Ty::List` erases the element type at codegen; threading it (e.g. `Ty::List(Box<Ty>)`) is the enabling refactor for string `get`/`first` and for `Option<String>` results. Nested maps need per-depth element slots or real iteration state. |
| **WASI HTTP handler: request introspection & response composition** | Slice 1b landed: a `(Request) -> Response` program compiles to a standard `wasi:http/service` component and serves under `canon run --addr` (see the closed-gap row below). The handler body is now **fully compiled** (`WasmGen::compile_http` — HTTP encoder mode: self-contained module, `wit-component` import naming, `Headers()`/`Response(…)` constructors mapped onto real `wasi:http/types` calls in `build_http_response`), so dynamic status, dispatch, helpers, prints, and every language feature work inside a handler. String response bodies landed too: the `Response(Body * Headers * Status)` constructor pipes the body through a real contents stream — the `handle` export is now **async-stackful** (`[async-lift-stackful]`, result via an 8-slot flat `[task-return]handle`), so after `task.return` hands the response to the host, the still-running task performs the *sync* `stream.write` + `stream.drop-writable` + trailers `future.write`/`drop-writable` while the host consumes them. No leaked writer handles remain (the slice-1b park-and-leak hack is gone). Still missing: request *headers* and the request *body* (the stdlib `body` accessor in `packages/canon/std/src/http/request.can` is declared but not lowered — path and method landed, see the closed-gap rows below). Also: HTTP programs can't use non-`wasi:http` extern imports (`canon:builtins/*` host bridges) — `new_http` rejects them with a clear error because the `wasi:http/service` world can't satisfy them. | Slice 3 of `WASI-HTTP-HANDLER.md`. Request *bodies* need the incoming contents stream read (sync `stream.read` after… careful: reading blocks until the host feeds it, which works in the stackful task *before* `task.return`). The stdlib `Response`/`Headers` bindings in `packages/canon/std/src/http/` also need re-pointing: they bind `[constructor]response`, which doesn't exist in the real rc WIT (`[static]response.new` with a trailers future is the actual constructor). |
| **`Stream<T>` lowering + streaming response bodies (SSE / NDJSON)** | The current SSE pathway is a one-shot `Content-Type:` prefix on the handler's return `String` (see the closed-gap row below) — no per-event streaming. Real streaming needs `Stream<T>` as a canonical-ABI handle, a `canon/std/Stream` combinator surface (`map` / `filter` / `take` / `concat`), and a `(Headers * Status * Stream<String>) -> Response` constructor that pipes the stream into `wasi:http/types`' `writable-body`. Today the codegen silently drops imports whose signatures mention `Stream<T>` (`build_extern_component_params` returns `None` on unknown-shape params), so a program that imports stream combinators fails at runtime with "matching implementation was not found in the linker" — the imports were never even emitted. | Architecture in `STREAMING.md`. Slice 0 (stdlib skeleton + `tests/checker/ok/stream_compose.can` + the `Stream<T>` no-peel fix in `method_return_summary`) and slice 1a (`tests/wit_component_stream_prototype.rs` proving `wit-component` lifts `stream<u8>` params/returns without per-stream codegen) have landed. Slice 1b is the integration push: route programs whose surface mentions `Stream<T>` (or any `Handle`-typed param) through `ComponentEncoder::module(…)` instead of the hand-rolled `wasm-encoder` type section — the same architectural shortcut from the WASI-HTTP-HANDLER row. Slices 2–4 layer combinators (pure Canon), the `Response` streaming-body constructor, and a real time-driven endpoint test on top. SSE itself is two helpers in `canon/std/http/sse.can` (`eventStream`, `formatSse`) — no primitive, no host bridge. |

**Slice-1b architectural note (Nov 2025).** All three prototypes—
`tests/wit_component_prototype.rs::{wit_component_round_trip_minimal_world,
wit_component_round_trip_http_like_world}` plus
`tests/wit_component_stream_prototype.rs::wit_component_round_trip_stream_world`—
validate the same claim: when the codegen routes a program through
`wit_component::ComponentEncoder` instead of hand-rolled
`wasm-encoder` (the path the CLI entry uses), **every gap above that's
been blocking `wasi:http/handler` or `Stream<T>` becomes irrelevant on
our side**. The `http_like` prototype's WIT carries a `resource`, an
`own<T>` parameter, a `u16` (sub-u64), and a variant with `option<u16>`
/ `record` / `option<u64>` / `option<string>` payloads; the `streams`
prototype's WIT carries `stream<u8>` as both a parameter and a return.
`wit-component` emits all the canonical-ABI bytes for those without any
of the lowering work CLAUDE.md was tracking on the right-hand side. Our
codegen's remaining responsibility for slice 1b shrinks to: (a) emit a
core module with the correct `(import ...)` + `(export ...)` names
using `<iface>#<func>` for interface-bound exports, (b) embed the WIT
metadata via `wit_component::embed_component_metadata`, (c) run through
`ComponentEncoder::default().module(…).encode()`. The hand-rolled
`wasm-encoder` path stays in place for the CLI world.

**Update (Jul 2026): slice 1b landed** on exactly that plan — see
`wrap_http_service` in `src/codegen/wasm/component.rs` and the
closed-gap row below. Two practical findings worth knowing beyond the
prototypes: (1) the canonical ABI refuses to drop an unwritten future
writer, so "no trailers" must be an `[async-lower]`ed `future.write`
of `ok(none)` left pending (the writer handle is leaked on purpose);
(2) on the host side everything that touches a guest body — including
`Response::into_http` and collecting the body — has to happen inside
the `run_concurrent` scope, because the pipe tasks feeding the body
channels are only polled while the store is driven.

The following gaps were *closed* in recent passes — mentioned here so the
shape of the working machinery is documented somewhere:

| Recently fixed | Mechanism |
|---|---|
| **The web target: Canon frontends in the browser** | `WEB-TARGET.md`. A program defining the Elm triple (`init = () -> Model`, `update = (Model * String) -> Model`, `view = (Model) -> Html`) compiles through `WasmGen::compile_web` to a self-contained *core module* (browsers don't run components) plus a generated JS host: `canon build` writes `<stem>.wasm` + `canon-web.js` + `index.html`, `canon run` serves the bundle (`src/webhost.rs`). Entry detection is `find_web_entry` in `src/ast.rs` (names + shapes — every view helper returns `Html`, so a return-type rule can't work); the checker's entry matrix rejects mixed worlds. The model stays in guest memory as an opaque i64 (`WebModelShape` normalizes ptr/i64/f64/string reprs); messages are strings in, HTML strings out; `.print()` maps to `console.log` via stubbed stdout imports. Events are declarative attributes (`data-msg`, `data-msg-form`, `data-fetch` — the last one is the host-mediated bridge to a Canon backend). New pure-Canon `canon/std/web` package: `Html`/`Msg`/`Tag`/`Attr` newtypes, element helpers, recursive HTML-escaping `text()`, `toText()` for Int rendering. Pinned by `tests/web_target_test.rs` (drives the exact JS-host ABI under wasmtime) and verified in headless Chromium; `examples/counter-web` and `examples/todo-fullstack` are the demos. |
| Response headers: `Headers().set(name, value)` | `[method]fields.append` joined the HTTP encoder mode's fixed import space (`FN_HTTP_FIELDS_APPEND`); the `set` builtin arm parks receiver + both string args on the operand stack (user code can't clobber it), peels into locals, and ignores the `result<_, header-error>` ret area — a rejected name degrades to "header absent", same posture as `set-status-code`. `set` binds to *append* deliberately: on a freshly-built `fields`, appends have set semantics with the simpler single-value WIT shape. Pinned by `wasi_http_service_response_headers`. |
| `Request.method()` — REST verbs route via literal dispatch | `[method]request.get-method` (`FN_HTTP_GET_METHOD`) returns the WIT `method` variant through a 12-byte ret area; the builtin arm maps static discriminants onto interned strings ("GET", "POST", …) and passes `other(string)` payloads through, surfacing the result as a plain `String` — so routing is literal dispatch with a catch-all, not a 10-arm union dispatch. Stdlib decl changed to `method = (Request) -> String`. Pinned by `wasi_http_service_method_dispatch`. |
| Param scope: exact names beat alias-derived names | `build_local_scope` used to register every param under its whole alias chain in order, so a later same-underlying param clobbered an earlier exact one — in `elAttr = (Attr * String * Tag)`, `Tag`'s alias registration stole the body's `String` references. Exact declared names now register unconditionally; alias-chain names only fill unclaimed slots (receiver-first). Pinned by `tests/runtime/param_alias_precedence.can`. |
| Literal dispatch on a newtype scrutinee no longer shadows `String` | `emit_literal_dispatch` bound the scrutinee under the bare primitive name unconditionally, so `Prefix(String.substring(1, 4)).(…)` hijacked the enclosing function's `String` param inside arm bodies (empty payloads, OOB substrings). The primitive base binds only for *bare* string scrutinees; a newtype-wrapped scrutinee binds its own name — distinguishing the two is why the user wrapped it. Pinned by `tests/runtime/literal_dispatch_newtype_scrutinee.can`. |
| Bump allocator grows memory on demand | `build_alloc` emits a `memory.size`/`memory.grow` check instead of trapping once past the initial two pages — long-lived instances (web apps dispatching events, HTTP handlers under load) outlive the fixed heap. A failed grow is ignored; the subsequent store traps, which is the honest failure. |
| Formatter: field access on a method-chain receiver | `emit_base_inline`'s `FieldAccess` arm recursed with `emit_base_inline`, which renders chain shapes as empty strings — `Counter(1).bump().Int.print()` formatted to `.Int.print()`, destroying the program. It now routes through the chain-aware `emit_inline`. |
| Record-of-scalars extern returns (`wasi:clocks/system_clock#now`) | `IndirectReturnShape::ScalarRecord`: the vendored WIT (`component::vendored_extern_record_return`) provides the record's field names/prims; collect computes the canonical layout (size/align per prim), the component wrapper defines the record type and exports it under the WIT name (imported-function types must be *named* — same alias pattern as the stdout `error-code` enum), and the decode copies each canonical field into a fresh Canon product struct (bindgen's `Product = ProductFieldA * ProductFieldB` rendering), widening narrow ints to i64. First user: `now = () -> Instant`; `canon/std/time/Unix` wraps `now().InstantSeconds` as Unix seconds. Pinned by `tests/canon/time_test.can` (asserts the clock reads after 2001 — deterministic on any sane host). Note: `bindgen` still skips record *params* and records containing non-scalar fields. |
| `option<string>` and `list<string>` extern returns | Two new `IndirectReturnShape` variants. `OptionString`: 12-byte ret area (disc byte at +0, ptr/len at +4/+8) re-shaped into a fresh Canon Option struct so ordinary `(None, Some<String>)` dispatch works — same decode as `request.path()`. `ListString`: 8-byte ret area (list ptr, count) pushed directly as `Ty::List` — the canonical-ABI element layout for `list<string>` (8-byte stride, i32 ptr + i32 len per element) is byte-identical to Canon's `List<String>`. `classify_return` recognises `Option<String-alias>` / `List<String-alias>` return declarations; the component wrapper emits the matching `option<string>` / `list<string>` defined types. Unblocks `wasi:cli/environment` — `canon/std/cli` grows `Args()` (`getArguments`) and `Cwd()` (`getInitialCwd`) wrappers, pinned by `tests/runtime/cli_args_cwd.can`. |
| WIT-informed extern lowering: top-level narrow ints (u8/u16/u32/s8/s16/s32) | `collect_extern_imports` now consults the vendored WASI WIT (`component::vendored_extern_prim_sig`, backed by a `OnceLock<Resolve>` shared with the HTTP world emission) for every `wasi:*` extern URN: the component-level types take the WIT's exact widths and signedness, narrow slots lower to core i32, and `emit_func_table_call` inserts `i32.wrap_i64` on arguments / `i64.extend_i32_{s,u}` on results (conversion flags are recorded per-param in `ExternImport::narrow_params` — keying on "prim is narrow" alone broke `canon:builtins` Bool params, which are i32 on both sides). The bindgen skip is relaxed to top-level-only. First user: `wasi:cli/exit#exit-with-code` (u8) — the stdlib `canon/std/cli` `exit` now rides the real WASI interface (the `canon:builtins/cli` bridge is deleted; note the `@unstable(feature = cli-exit-with-code)` gate must be enabled on the `Resolve`), and `canon run` maps the guest's `I32Exit` onto the process exit code. Pinned by `tests/exit_code_test.rs`. Gotcha for wrapper authors: scalar newtypes erase to bare `Int` at the value level, so a stdlib wrapper must declare its receiver as `Int`, not the newtype (`Exit(3).exit()` dispatches on `Int`). |
| **`wasi:http/service` world emission (WASI-HTTP-HANDLER.md slice 1b)** | `wrap_http_service` + `build_http_service_core_module` in `src/codegen/wasm/component.rs`. A program with a free `(Request) -> Response` function compiles to a *standard* `wasi:http/service` component: a self-contained core module (own memory + `cabi_realloc`) exports `wasi:http/handler@0.3.0-rc-2026-03-15#handle` `(i32) -> i32` (request handle in, ret-area out; ok-discriminant byte at +0, response handle at +8 — the `error-code` variant's `option<u64>` cases force 8-byte payload alignment). The response is built by importing `wasi:http/types` functions under `wit-component`'s name-mangling conventions: `[constructor]fields`, `[static]response.new`, the `[future-new-1][static]response.new` intrinsic for the trailers future, and an `[async-lower][future-write-1]…` write of `ok(none)` that parks as pending (a sync write deadlocks — the host reads trailers only after `handle` returns; the writer handle is deliberately leaked since the ABI forbids dropping an unwritten future writer). WIT metadata from the vendored `wit-vendor/wasi/http.wit` (+ clocks/cli/random/filesystem/sockets, pushed in dependency order) is embedded via `wit_component::embed_component_metadata` and the whole thing encoded by `ComponentEncoder` — none of the resource/variant/narrow-int lowering is hand-rolled. Host side, `dispatch_request` in `src/runtime.rs` consumes the guest body **inside** `run_concurrent` (the body-feeding pipe tasks are only polled while the store is driven; collecting outside the scope hangs forever). Pinned by `tests/wasi_http_service_test.rs`. The handler body is fixed at empty-200 until slices 2–3 (see the open-gap row above). |
| Option/Result string payloads on the construct side | Verified fixed — `Some("payload")` writes the `(ptr, len)` pair through `store_payload_at_offset` and the `(Some<String>)` arm reads it back intact; `Ok`/`Err` string payloads are covered by the `?`-short-circuit fixture. Pinned by `tests/runtime/option_string_payload.can` and `tests/runtime/try_short_circuit.can`. |
| Method lookup follows the newtype alias chain (`Foo("x").ToJson()`) | `compile_method_call` keyed `func_table` lookups on the receiver's exact type name only, so a `ToJson` declared on `String` was invisible from a `Foo = String` receiver and the call was silently dropped. The lookup now walks `collect_alias_chain(receiver)` — `Foo`, then `String` — before falling back to free functions and builtins. Pinned by `tests/runtime/tojson_newtype_receiver.can`. |
| `Int(1)` / `Float(2.5)` / `String("x")` explicit constructors | Primitive identity constructors in `compile_constructor`: the argument already has the target representation, so compiling it *is* the construction (with defensive `I64ExtendI32S` / `F64ConvertI64S` widening for shape drift, and zero values for the zero-arg forms). Previously they fell through to the union-variant path and produced nothing. Pinned by `tests/runtime/int_float_explicit_constructors.can`. |
| `?` short-circuits on `Err` and `None` | `Expr::Try` checks the container tag (Ok/Some = 1, Err/None = 0 at offset 0) and, when the *enclosing* function returns the **same** container kind (one-i32-pointer core shape; kind recorded in `cur_fn_early_return` by `build_user_function`), returns the whole value unchanged. `Option<String-alias>` returns surface as `Ty::NamedPtrStr("Option", payload, payload)` (both body fns via block-2 classify and externs) so `?` extracts the `(ptr, len)` payload instead of misreading the slot as i64 — dispatch is unaffected since it keys on the container name. In non-matching contexts (e.g. `main`) extraction stays unconditional. Pinned by `tests/runtime/try_short_circuit.can` and `tests/runtime/option_try_short_circuit.can`. |
| `Bool.and` / `.or` / `.not` chains; bool prints get their newline | The three methods had no `Ty::I32` arms in `compile_builtin_method`, so they fell into the drop-everything fallback and the chain evaporated to `Unit`. Now `I32And` / `I32Or` / `I32Eqz` (eager, non-short-circuiting — fine for effect-free Canon expressions). Separately, `build_print_bool` never emitted the trailing `'\n'` every other print path appends, so `True().print()` jammed against the next line ("Trueyes" in the old `arithmetic.stdout` golden — the golden had pinned the bug). Pinned by `tests/runtime/bool_chains.can`. |
| `list.map(f)` applies `f`; `list.get(i)` lands | `compile_list_map` in `src/codegen/wasm/mod.rs`: because Canon lambdas are non-capturing (no local variables in the language), `.map`'s lambda body is *inlined* into an element loop — no function lifting, no `call_indirect`. The lambda parameter's type name (Canon bodies refer to a param by its type) binds to a dedicated element local (`map_elem_i64` for `Int`, the adjacent `map_elem_ptr`/`+1` pair for strings). Loop state (`src`, `dst`, `remaining`) is carried on the wasm operand stack via multi-value block/loop params — the body is arbitrary user code and may clobber every scratch local, but it can't reach values parked below its own stack activity. Supports `Int` and string-shaped elements, and any result shape (`Int`/`Float`/`String`/pointers), so cross-type maps like `List<String>.map((String) -> Int { String.length() })` work. `list.get(i)` mirrors `first`: unsigned bounds check, `Some(i64 slot at ptr + i*8)` or `None`. Pinned by `tests/runtime/list_map_get.can` and `tests/runtime/list_map_strings.can`. |
| N-variant union dispatch (the old "3-variant dispatch picks the wrong arm" gap) | Verified fixed — closed as a side effect of the newtype-alias/dispatch passes. Pinned by `tests/runtime/union_four_variants_mixed.can`: a 4-variant union with `Int` / `Unit` / `String` / `Float` payloads dispatches and extracts every payload correctly. |
| `Float` values silently dropped by `.print()` and invalid wasm from `Float` payloads | Two related holes. (1) `emit_print`'s `Ty::F64` arm was a `Drop` stub — `2.5.print()` produced *nothing*. New `build_print_float` helper (`fn_print_float`, core sig `(f64) -> ()`): fixed-point rendering into the shared int buffer, up to 6 fraction digits with trailing zeros trimmed (`2.0` → `2`), `NaN`/`Inf`/`-Inf` specials, saturating on >u64 integer parts — pragmatic, not shortest-round-trip dtoa. (2) `Ty::F64` was muddled with `Ty::I64` across the value plumbing: f64 values were `LocalSet` into the i64-typed `tmp_i64` scratch (invalid wasm — "expected i64, found f64" at component compile) and float fields/payloads were read back with `I64Load` (type-confused values downstream). Fixed by adding an f64-typed `tmp_f64` scratch (wasm locals are monomorphic) and splitting every `Ty::I64 \| Ty::F64` match arm that touches locals or memory: `save_ty_to_scratch` / `load_from_scratch`, `store_payload_at_offset` (F64Store), `load_product_field` (F64Load), `build_union_value` (new `SavedField::F64_0`), `bind_arm_payload`, `build_list_literal`. Pinned by `tests/runtime/float_print.can`, `tests/runtime/float_in_product.can`, and the `Float` variant in `union_four_variants_mixed.can`. |
| Bound arm payloads corrupted by `String.concat` / `String.substring` in the arm body | `String.concat` and `String.substring` stashed scratch `(ptr, len)` pairs in `scope.arm_payload_ptr()` / `+1` — the same locals `bind_arm_payload` uses to hold the arm's bound payload. The first `concat` whose argument wasn't the bound name itself overwrote those locals, so any later reference to the bound name in the same arm body read garbage. Fixed by adding a dedicated `scope.str_scratch_ptr()` pair (`extra_locals_decl()` in `src/codegen/wasm/mod.rs`) and routing both builtins through it. Pinned by `tests/runtime/arm_payload_after_concat.can` — the third line (`echo: model:model`) is the regression marker: pre-fix it printed `echo: model::` because the second `String` reference saw the clobbered slot. Unblocks the dispatch-arm composition patterns the user wanted (echoing a parsed field back through a chain of concats / JSON-literal scaffolding) without touching `build_result_ok` / `build_option_some` / the bump allocator — those were never the actual cause despite the original gap report. |
| Newtype field access (`Greeting("hi").String`) | `newtype_unwrap_ty` in `src/codegen/wasm/mod.rs`: retypes the on-stack value when the field name matches the underlying type. No-op at the wasm level. Extended to `Ty::I64`/`F64`/`I32` so `ParsePos.Int` (where `ParsePos = Int`) and similar primitive-newtype unwraps also work. |
| Variant payloads in arm bodies (user variants, string payloads) | `bind_arm_payload` + `arm_payload_binding` in `src/codegen/wasm/mod.rs`: extracts `(ptr, len)` from offsets 4/8 of the union struct into adjacent locals, then binds the arm's pattern name in the scope so `Expr::Ident` lookups find it. Pairs with `SavedField::Str0` on the construction side. Also handles `Ty::I64`/`F64`/`I32` payloads now — reads the 4/8-byte value at offset 4 into a scratch local and binds. |
| String primitives (`length`, `byteAt`, `substring`, `eq`) | New built-ins in `compile_builtin_method` in `src/codegen/wasm/mod.rs`. Enable pure-Canon parsers / validators over strings (see `packages/canon/std/src/json.can`). |
| `?` short-circuit on user-defined `Result<X, Y>` (X, Y both String-aliased) | Block 2 of `assign_func_indices` now calls `classify_return`, so user-body validators returning `Result<Json, MalformedJson>` get the same `Ty::NamedPtrStr` shape as extern-backed ones. `build_result_ok` / `build_result_err`'s layout (tag at +0, ptr at +4, len at +8) already matched the extern indirect-return area, so no calling-convention change was needed — just the type label. |
| Self-ctor commutative method-call dispatch for body functions | Block 2 of `assign_func_indices` now mirrors block 1's commutative registration: a `Name = (P) -> R<Name, E> { … }` validator becomes reachable via both `Name(p)` and `p.Name()`. Pairs with a `func_table` dedupe in `compile()` so each function body declares its type exactly once. |
| Receiver/param alias chain in `build_local_scope` | A function with receiver `Json` (where `Json = String`) now registers locals under both `Json` and `String`, so the body can reference the value by either name. Required for body-defined validators that use the underlying type name in the body (e.g. `String.validateOnly()` in a `Self`-renamed `Json` constructor). |
| `build_product_value` clobbered `scope.alloc_ptr` on nested constructors | Products whose fields required nested allocation (an inner product, `Some(…)`, …) silently produced wrong values or trapped: each `LocalGet(alloc_ptr)` between field stores re-read a local the nested constructor had reassigned, and the final result pointer pointed at the *inner* allocation. Fixed differently from `build_union_value`'s scratch-local pass (which caps out at 2 ptr + 1 i64 fields): `build_product_value` now pre-pushes one base-address copy per stored field (plus one for the result) onto the wasm operand stack right after `$alloc` — operand-stack values below a nested expression's own activity are immune to local clobbering, and the approach scales to any field count and nesting depth. `store_payload_at_offset`'s string branch was re-loading `alloc_ptr` too; it now stores through the on-stack address stashed in a new `addr_scratch` local (live only between adjacent instructions, never across a nested `compile_expr`). Pinned by `tests/runtime/product_nested_constructor.can` — pre-fix it printed one right value then trapped on the nested-field read. |
| Value-level product construction (`Foo(a * b * c)`) | `build_product_value` in `src/codegen/wasm/mod.rs`: allocates a heap block sized via the existing `product_field_layout` helper, then for each field pushes the struct base, compiles the field expression, and stores it at the field's byte offset via `store_payload_at_offset`. Returns `Ty::NamedPtr(product_name)`. `compile_constructor`'s product branch routes `Foo(a * b * c)` (single-arg `Expr::ProductValue`) and `Foo(a, b, c)` (positional N-arg) through it; mismatched-arity calls fall through to the legacy side-effect-only path. The paired `load_product_field` helper teaches `Expr::FieldAccess` in `compile_expr` to read back from the matching offset — scalars/named-pointers via one i32/i64 load, strings via two i32 loads for `(ptr, len)`. Pinned by `tests/runtime/product_three_field.can` (Int newtypes) and `tests/runtime/field-access.can` (String newtypes, now actually printing the field). |
| Parameterized type names in expression position (`Option<Content>.(…)`, `wrapper.List<Choice>`) | `Parser::consume_phantom_type_args` in `src/parser/parser.rs`: after a PascalCase identifier in expression position, optionally consume a `<T1, T2, …>` type-argument list and discard it. The generic args are purely a type annotation — runtime values don't carry parameters — so the resulting `Expr::Ident` / `Expr::FieldAccess` looks up the unparameterized name as before. Wired into `parse_primary` (bare-ident case) and `parse_expr` (field-access case). |
| Dynamic HTTP handlers (single-handler convention) | When the program defines a top-level `handleRequest = (String) -> String { … }`, the compiler synthesises a canonical-ABI wrapper (`build_handler_wrapper` in `src/codegen/wasm/mod.rs`) with core signature `(i32, i32) -> i32` matching the callee-allocated indirect-return shape, and `component::wrap` lifts it as an `canon:http-handler/handler@0.1.0` instance carrying `handle-request: func(body: string) -> string`. The runtime (`run_component_async` in `src/runtime.rs`) looks the export up post-instantiation via `Instance::get_export_index` / `Instance::get_func` and stashes the `Func` in `State.handler_func`. `host_builtin_http_server::serve` snapshots the Func via `Accessor::with`, then dispatches each incoming request through `Func::call_concurrent` inside the connection loop. Pinned by `tests/http_handler_test.rs::dynamic_handler_round_trip` — spawns a child `canon run`, sends an HTTP request to the bound port, asserts the echoed body. Multi-route dispatch (handler ID + `__http_dispatch` switch) and inline lambdas remain future work; see `DYNAMIC-HANDLERS.md` slices 2 and 3. |
| SSE / streaming-response Content-Type | The host's dynamic-handler path now honours an optional `Content-Type: <mime>\r\n\r\n` prefix in the handler's return string. When present, the response goes out with that Content-Type; absent the prefix, `text/plain` is used as before. This is the minimum-viable path for `text/event-stream` (and `application/json`, etc.) responses — a single payload per request, no per-event streaming yet. Pinned by `tests/http_handler_test.rs::dynamic_handler_sse_content_type`. **Superseded by `STREAMING.md`** — true multi-event streaming is pull-based via `Stream<T>` flowing into the response body, *not* a push-style `SseSender` capability (the original "future work" sketch here was wrong about the shape). Once the `STREAMING.md` slice 3 lands the prefix hack + `parse_handler_response` get deleted; SSE becomes a four-line composition over `Stream<String>` with no host involvement beyond the standard `wasi:http/types` writable-body. |
| `List<String>.toJsonArray()` builtin | New helper function `fn_list_to_json_array` (`build_list_to_json_array` in `src/codegen/wasm/mod.rs`) slotted alongside `fn_alloc`, `fn_print_*`. Core signature `(list_ptr: i32, list_len: i32) -> (i32, i32)`. Two-pass algorithm: pass 1 sums `2 + sum(elem_len) + max(0, len-1)` to size the output, pass 2 fills it with `[`, comma-separated elements, `]`. The checker recognises `toJsonArray` as a List method returning `Json` (`is_known_method` + `method_return_type` in `src/checker/mod.rs`). Pinned by `tests/runtime/list_to_json_array.can`. Pairs naturally with the `FromJson` primitives already in place — `someList.map(toJson).toJsonArray()` is the canonical pattern (once `list.map(f)` does more than identity, see the `List<T>` iteration gap). |
| Dispatch through newtype-wrapped unions (`MessageContent = Option<Content>` then `value.(None, Some<Content>)`) | Two changes: (1) `collect_symbols` in `src/checker/mod.rs` now records aliases for typedefs whose RHS has generic args (`MessageContent -> Option`) and walks the alias chain when matching dispatch-arm patterns against the scrutinee's type; (2) `build_variant_info` in `src/codegen/wasm/mod.rs` mirrors the same: after registering union variants, it propagates each union's variant set to every alias that transitively resolves to it through `type_defs`. So `union_variants["MessageContent"] == ["None", "Some"]` and `compile_match` dispatches correctly. Pinned by `tests/checker/ok/dispatch_through_newtype_option.can` and `tests/runtime/dispatch_through_newtype.can`. The newtype's `product_fields` registration in `collect_symbols` was also relaxed to include generic-bearing RHS, so `value.Option<Content>` field-access lookups succeed. |
| `FromJson`-style typed-constructor primitives | Two new host functions in `mod host_builtin_json` (`src/runtime.rs`): `field: (string, string) -> result<string, string>` extracts a JSON object field's raw JSON text by name, `to-string: (string) -> result<string, string>` decodes a JSON string literal into its raw contents. Both return `result<string, string>` to hit the existing `IndirectReturnShape::ResultStringString` path. Surfaced in `packages/canon/std/src/json.can` as `field = (Json * String) -> Result<Json, MalformedJson>` and `asString = (Json) -> Result<String, MalformedJson>`. Composes with user `(Json) -> Result<MyType, MalformedJson>` validators via `?`-propagation — the Coulisse pattern `Json(body)?.ChatCompletionRequest()?` works end-to-end. Pinned by `tests/runtime/json_field_extract.can` and `tests/runtime/json_typed_constructor.can`. Array iteration, number extraction, and bool extraction are future extensions of the same shape. |

## Key conventions

- **Alphabetical ordering** is central to the language. If you modify the parser or checker, be aware that sort-order enforcement applies to: product type fields, union variants, function declarations, dispatch arms, and imports. `canon fmt` auto-sorts all of these; the checker errors are the backstop.
- **Indexing is 1-based** everywhere (`byteAt(1)`, `list.get(1)`, positional access `.1`), and `substring(a, b)` is inclusive on both ends. Don't "fix" this to 0-based — it is a deliberate language decision (DESIGN.md § Indexing Is 1-Based).
- The compiler pipeline is: **source → lexer → parser → checker → codegen (wasm core module → Component Model wrapper)**. No external toolchain is invoked — `wasm-encoder` / `wit-component` produce the final `.wasm` in-process, and `canon run` executes it on the embedded wasmtime.
- Standard library is **layered** but ships as a single bundled package, `canon/std`. The package's manifest declares its WIT dependencies under `[imports]`; `canon install` materializes the bindings into `packages/canon/std/bindgen/<ns>/<pkg>/<iface>.can` (one file per interface; each function declared with a bare `extern Wasm` marker). The hand-written wrappers under `packages/canon/std/src/` then `use wasi/…` to consume them, exactly as user code would `use wasi/…` against bindings installed into its own `bindgen/` directory. There is no privileged shape that only the stdlib can use — the bundled-package and project-`bindgen/` lookups are different code paths in the loader but cover the same `use` paths from the source's point of view. Users only ever `use canon/std/…`. The compiler's runtime fulfils the WASI imports through `wasmtime_wasi::p3`.
- The `packages/canon/std/bindgen/` tree is regenerated by `just regen-bindings` (which is just `canon install packages/canon/std`). Don't hand-edit it; bump the vendored WIT and regenerate.
- A bindings file declares its provenance with a single `bindings "<ns>:<pkg>/<iface>@<ver>"` directive at the top, then lists the bound functions as bare function-type aliases (`name = (P) -> R`). The loader (see `apply_bindings_directive` in `src/loader.rs`) rewrites those aliases into FunctionDefs with `extern_wasm.path` set to `<urn>#<fn-kebab>`. This is what `canon install` emits and what every file under `bindgen/` looks like.
- The legacy per-function `extern Wasm("<urn>")` syntax is still parsed for user-written code that wants an explicit URN without using a `bindings` directive. The bindgen no longer emits it; `bindings` is the canonical form.
- Each `bindgen/` directory also contains an `_install.toml` sidecar written by `canon install`: a map from `<rel-path>.can` to the WIT interface URN that file was generated from. It's a derived artifact (committed for `canon/std` so `cargo build` works on a fresh clone, gitignored for user projects). The runtime parser lives in `src/install.rs::parse_install_index`; `build.rs` has its own minimal parser because it can't depend on the crate it's building.
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

greet = (Greeting * Name) -> Greeting {        # function (free, commutative)
    Greeting
}

main = () -> Unit {                            # entry point
    "hello".print()
}

True().(                                       # dispatch (branch on union)
    * (False) -> Unit { "no".print() }
    * (True)  -> Unit { "yes".print() }
)

path.(                                         # literal dispatch (String/Int scrutinee);
    * ("/notes") -> Body { index() }           # the catch-all arm is required, always last
    * (String) -> Body { notFound() }
)

List(1, 2, 3).map((Int) -> Int { Int.mul(2) }) # lambda
```

- No local variables, no `let`, no `if`/`else`, no comments in the language.
- `use Foo` imports from `foo.can` in the same module directory (or the corresponding folder for modules).
- See `DESIGN.md` for the complete specification.
