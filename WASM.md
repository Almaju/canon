# Oneway WASM Backend — Status

The Oneway compiler emits a WebAssembly **Component Model** component
targeting **WASI Preview 3**. There is no Rust backend, no `--target` flag,
and no external toolchain step — `oneway build` produces a `.wasm` directly
and `oneway run` executes it through an embedded `wasmtime`.

The lexer / parser / checker / loader are target-agnostic. All codegen lives
in `src/codegen/wasm/` and the runtime in `src/runtime.rs`.

---

## Current pipeline

```
.ow source → Lexer → Parser → AST → Loader (resolves `use`, runs auto-await)
           → Checker → WASM Codegen → core module → wit-component wrap
           → .wasm component → wasmtime (WASI P3 + oneway:* host bridges)
```

| File | Role |
|---|---|
| `src/ast.rs` | `FunctionDef.extern_wasm: Option<ExternWasm>`, `Expr::Await` |
| `src/loader.rs` | Bundled-package registry (from `packages/oneway/{std,wasi}/`); runs `auto_await::transform` |
| `src/checker/mod.rs` | `Future`/`Stream` in `BUILTIN_GENERIC_TYPES`; `method_return_summary` peels one layer |
| `src/checker/auto_await.rs` | Rewrites `Future<T>` method receivers as `Expr::Await` |
| `src/codegen/async_analysis.rs` | Bottom-up fixpoint → `AsyncSet` of suspending functions |
| `src/codegen/wasm/mod.rs` | Core module emission, `extern Wasm` import collection, expr compilation, `emit_async_call` |
| `src/codegen/wasm/component.rs` | Component wrapping, canonical-ABI lowering, WIT world emission |
| `src/runtime.rs` | wasmtime embedding; WASI P3 + `oneway:*` host bridges |

The compiler depends on `wasm-encoder`, `wit-parser`, `wit-component`,
`wasmparser` (0.250) and `wasmtime` / `wasmtime-wasi` (45, `p3` feature).
Pin `wasm-encoder` / `wit-component` / `wasmparser` together — they share
internal types and 0.250 matches the wasmparser bundled by wasmtime 45.

---

## Phase status

| Phase | Status |
|---|---|
| 1 — Infrastructure | ✅ |
| 2 — Hello world | ✅ |
| 3 — Type system | ✅ |
| 4 — WASI stdlib | 🟢 most examples pass; gaps below |
| 5 — Async | 🟢 single-subtask suspension + `parallel`/`race` working end-to-end; `Stream<T>.each` codegen pending |

### Passing examples (`examples/`)

`examples/` is a Cargo-style workspace; each subdirectory is a package.
Passing members: `clock`, `random`, `now`, `read-file`, `fetch-url`,
`multifile`, plus `http-server` (checks, builds, runs to the "Starting
server…" banner; `.serve()` is a stub host-side and exits 0).

### Passing runtime fixtures (`tests/runtime/`)

`arithmetic`, `commutative`, `extern`, `field-access`, `hello`, `list`,
`literals`, `loop`, `match`, `methods`, `newtype_unwrap`, `option`,
`stdlib-string`, `traits`, `tree`, `types`, `variant_payload_extraction`,
`async_echo` (exercises the alloc / async-lower call / status-mask /
sync-completion decode path against `oneway:builtins/http-server#echo`).

---

## `extern Wasm` end-to-end

A declaration like

```
extern Wasm("wasi:random/random@0.3.0-rc-2026-03-15#get-random-u64")
randomInt = () -> Int
```

is lowered as follows:

1. **Path parse** (`parse_extern_path`) → `(component_ns, core_ns, fn_name)`.
   The component namespace keeps `@version`; the core namespace is the
   internal module-import name.
2. **Signature derivation** (`func_wasm_params_for` / `func_wasm_results_for`)
   resolves aliases and treats capability receivers as zero stack slots, so
   `(Random) -> Int` matches WASI's `() -> u64`.
3. **Core import** (`WasmGen::compile`) — one `(import "core_ns" "fn_name")`
   per extern, sorted by `(core_ns, fn_name)`, right after `host.print`.
4. **Component wrap** (`component::wrap`) — groups externs by interface,
   declares one component instance type per interface, lowers each function
   through the canonical ABI with the appropriate `CanonicalOption`s, and
   wires instances into the core module instantiation.
5. **Linker** (`runtime.rs`) — `wasmtime_wasi::p3::add_to_linker` covers
   every `wasi:*` extern; small `oneway:*` host modules cover the bridges.

Externs are **signedness-aware**: `wasi:*` paths use `u32`/`u64` while
`oneway:*` paths use `s32`/`s64`, matching Oneway's `Int = s64` convention.

### Supported call-site shapes

| Shape | Where exercised |
|---|---|
| Flat scalar params + flat scalar return | `random.ow`, `clock.ow`, `extern.ow` |
| String return (indirect via `cabi_realloc`) | `now.ow` |
| String params + string return | `stdlib-string.ow`, `read-file.ow`, `fetch-url.ow` |
| `result<string, string>` return (with `?`) | `read-file.ow`, `fetch-url.ow` |
| `extern Wasm.async` with sync-completion fast path | `tests/runtime/async_echo.ow` |

Anything more exotic (lists, records, `Result<Int, …>`, futures of
non-strings) is silently skipped by `collect_extern_imports` — those call
sites fall through to built-in dispatch.

### Canonical-ABI architecture

The component wrapper instantiates a small **memory provider** core module
first (`build_memory_module`) which exports `memory`, a shared `bump_ptr`
i32 global, and `cabi_realloc(old_ptr, old_size, align, new_size) -> ptr`
(a single-pass bump allocator). The user core module imports the memory
from `env.memory` and shares `bump_ptr` so guest-side `$alloc` and
host-driven `cabi_realloc` work the same heap.

For each `extern Wasm` function, `IndirectReturnShape` describes the return:

- **`None`** (flat scalar): lowered with no options (or just `Memory(0)` if
  any string param is present). Call sites push params and emit
  `Call(idx)` directly.
- **`String`**: `canon lower` adds `Realloc(cabi_realloc)` + `UTF8`. Core
  sig becomes `(…params, i32 ret_area) -> ()`. Call sites alloc 8 bytes,
  pass the pointer, then read `(ptr, len)` back into a `Ty::Str`.
- **`ResultStringString`**: 12-byte return area `[u8 disc, _, _, _, i32
  ptr, i32 len]`. The WIT discriminant (0=ok, 1=err) is the inverse of
  Oneway's alphabetical tagging (Err=0, Ok=1), so the codegen XORs byte 0
  with 1 after the call. Pushed as `Ty::NamedPtrStr("Result")` so `?` /
  match arms can extract the string payload.

`.print` always emits a single trailing `\n` (a one-byte `print_str` of
the newline at `MEM_INT_BUF_END`) so host-returned strings and literals
print identically.

### Async lowering (`extern Wasm.async`)

The canonical-ABI "async lower" rule produces a core wasm sig of
`(flat_params…, ret_ptr?) -> i32`, where `ret_ptr` is appended when the
function has a result and the trailing `i32` packs the subtask status
(low 4 bits = `CallState`, high 28 bits = subtask waitable handle).

In the component, the function type is declared as `async func(…)` via
`ComponentFuncTypeEncoder::async_(true)` and lowered with
`CanonicalOption::Async + Memory + Realloc`.

`WasmGen::emit_async_call` compiles the guest-side sequence:

1. Compile args flat onto the stack (same as a sync call).
2. If there's a result, `$alloc` a ret-area sized via `ret_area_size_for`
   and push its pointer as the trailing core arg.
3. `call $async_import` → packed status word.
4. Mask `status & 0xF`, compare to `2` (Returned). On the
   sync-completion fast path, decode the result from the ret-area.
5. On any other status (`Starting`/`Started`), allocate a single-element
   waitable set, join the subtask, block on `waitable-set.wait`, then
   drop the subtask before the set (otherwise wasmtime's
   `ResourceTableError::HasChildren` trips). When wait returns the host
   has written the result into our ret-area; decode and return.
   Exercised by `tests/runtime/async_slow_echo.ow`.

---

## What's still missing

### Phase 4 gaps

| Gap | Notes |
|---|---|
| **JSON structural derive** | `Json` validation, `ToJson` for primitives, and `{"k": v}` / `[v, ...]` literal syntax with arbitrary-expression interpolation are live (`oneway:builtins/json` host bridge). Still needed: compiler-derived `ToJson` / `FromJson` for user-defined product / union types, including auto-fall-through for newtypes (today `Email = String` needs a hand-written `ToJson` or `.String` unwrap). |
| **More `Result<T, E>` shapes** | `classify_return` only recognises `Result<String, String>`. `Result<Int, String>`, `Result<Unit, IoError>`, … need a small extension to the indirect-return decoder (machinery is generic; only the recognition + readback is hard-coded). |
| **Early-return on `?`** | `?` extracts the Ok/Some payload but doesn't branch on the discriminant. A failing host call surfaces garbage on the Ok path. Fix: load tag after the lowered call, `br_if` to the enclosing function's epilogue on the error case. |
| **Match-arm payload binding for `Result<String, String>`** | Dispatch picks the right arm, but the bound variable in `Ok(s) => … s …` isn't populated. The `bind_arm_payload` mechanism that works for user unions needs to extend to `Ty::NamedPtrStr`. |
| **Record returns** | e.g. `wasi:clocks/wall-clock#now → datetime { seconds, nanoseconds }`. Indirect-return machinery is there; needs a multi-field decoder. |

### Phase 5 gaps

| Gap | Notes |
|---|---|
| **`Stream<T>.each(lambda)` codegen** | The stdlib surface is declared in `packages/oneway/std/src/stream.ow` (`map`/`filter`/`take`/`concat`/`toList`/`toString`), the checker accepts the type, and `async_analysis::expr_has_async_trigger` recognises `.each` / `.next` as async triggers — but `build_extern_component_params` returns `None` on `Stream<T>` params/returns, so the imports are silently dropped and the binary won't link. Real fix is slice 1b in `STREAMING.md`: route Stream-using programs through `wit_component::ComponentEncoder` instead of the hand-rolled `wasm-encoder` type section. |
| **HTTP-server `.serve()`** | Host bridge (`host_builtin_http_server::serve`) is a stub that returns `0` immediately. Real semantics need host-driven invocation of guest handler lambdas — function-table indirect calls or resource-keyed handler tables. |
| **Sync-completion path through `emit_arg_as_nonblocking`** | `compile_parallel` / `compile_race` assume both arms suspend (subtask handle ≠ 0). If an async extern completes synchronously its packed status has CallState = Returned and subtask = 0; calling `waitable.join(0, set)` traps. Today this is fine because the only test arms (`slowEcho`) always suspend, but a robust implementation should branch on `status & 0xF == 2` and treat synchronously-completed arms as immediately `seen`. |

### Stdlib gaps

The "no plain `.rs` preludes" rule has one exception: `std/http-error.rs`
still exists. Either delete it (if unused) or document why it's needed.

---

## Capability → WASI P3 import mapping

| Oneway capability | WASI P3 interface |
|---|---|
| (implicit `print`) | `oneway:host/console` (bridge) |
| `Clock` | `wasi:clocks/monotonic-clock@0.3.0-rc-2026-03-15` |
| `Random` | `wasi:random/random@0.3.0-rc-2026-03-15` |
| `Filesystem` | currently `oneway:builtins/filesystem` (blocking bridge) — WASI `wasi:filesystem/types` async path is Phase-5 work |
| `Network`/`HttpClient` | currently `oneway:builtins/http` (blocking bridge) — `wasi:http/outgoing-handler` is Phase-5 |
| `HttpServer` | exported `wasi:http/handler` (Phase-5); currently stubbed via `oneway:builtins/http-server`. See `WASI-HTTP-HANDLER.md` for the migration plan |

The `oneway:*` interfaces are temporary scaffolds. Each one moves to the
corresponding `wasi:*` interface once its canonical-ABI shape (async,
streams, resources) is implemented.

---

## Testing

`cargo test` is the canonical entry point. Layers:

- `tests/checker/ok/` & `tests/checker/fail/` — checker fixtures, with
  `.stderr` goldens for the failure cases.
- `tests/runtime/` — full-pipeline programs with `.stdout` goldens (run
  through `oneway run` and compared exactly).
- `tests/oneway/*_test.ow` — `() -> TestResult` functions discovered by
  signature.
- `tests/async_test.rs` — unit tests for `async_analysis::analyse`.

Use `just update-fixtures` to regenerate goldens when output changes
intentionally. `examples/` is documentation, not a test layer.

---

## Invariants for future agents

- WASM is the only target. No `--target` flag, no Rust backend, no
  `rustc`/`cargo` shell-out for user programs.
- `extern Rust` is gone from the language. Don't reintroduce it.
- `Future<T>` and `Stream<T>` auto-await at method call sites. The user
  writes neither `async` nor `await`.
- `oneway fmt`, `oneway check`, `oneway inspect`, `oneway lsp` are
  target-agnostic and must keep working regardless of codegen state.
- When the codegen can't lower something, the answer is "not yet
  implemented" — never a fallback to a different backend.
- Every call to `codegen::generate` validates its output with `wasmparser`
  before returning (see `WasmGen::validate`). Don't disable this.
