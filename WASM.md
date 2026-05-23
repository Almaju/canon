# Oneway WASM P3 Backend — Implementation Plan

## Context and goals

Oneway is a programming language whose compiler is written in Rust. The current compiler transpiles Oneway source to Rust, then shells out to `rustc`/`cargo` to produce a native binary. **This plan replaces that backend entirely.** The new compiler emits a WASM Component Model component directly, targeting WASI Preview 3. No Rust is involved in compiling user programs. The Oneway compiler binary itself remains written in Rust.

**Key decisions (do not revisit):**
- WASM is the only target. There is no `--target` flag. There is no parallel Rust backend.
- The compiler embeds `wasmtime` as a library so `oneway run` requires zero external dependencies.
- `Future<T>` and `Stream<T>` are first-class Oneway types. They auto-await at method call sites — the user never writes `async` or `await`.
- `extern Wasm` and `extern Wasm.async` replace `extern Rust` and `extern Rust.async` in the language syntax and AST.
- The stdlib is rewritten as pure `.ow` files with `extern Wasm` declarations. The `.rs` prelude files are deleted.
- Function coloring is handled by the compiler via bottom-up fixpoint inference. A function is async if it transitively calls an `extern Wasm.async`, consumes a `Future<T>`, or iterates a `Stream<T>`. The user declares capabilities; the compiler does the rest.

This plan resolves the known tradeoff in `DESIGN.md`:

> *"Oneway is permanently coupled to Rust unless a second backend is later added."*

---

## Architecture

### Before

```
.ow source → [Lexer] → [Parser] → [AST] → [Checker] → [Rust Codegen] → .rs → rustc → binary
```

### After

```
.ow source → [Lexer] → [Parser] → [AST] → [Checker] → [WASM Codegen] → .wasm component
                                                                          ├── Core module (wasm-encoder)
                                                                          └── Component wrapper (wit-component)
```

The lexer, parser, AST, and checker are completely unchanged. The target split happens entirely in the codegen layer.

---

## User-facing async model

Given `send = (HttpRequest * Network) -> Future<Json>`, the user writes:

```
main = (Network * Stdout) -> Result<Unit, HttpError> {
    HttpRequest("https://api.example.com/data")
        .send(Network)?
        .print(Stdout)
    Ok(Unit)
}
```

`send(Network)` returns `Future<Json>`. `print` is a method on `Json`, not `Future<Json>`. The checker sees the type mismatch, inserts an implicit `Expr::Await` node, and marks `main` as suspending. The user writes nothing about async. The function becomes `async func` in the emitted WIT world automatically.

In practice, the stdlib hides `Future<T>` entirely behind `extern Wasm.async` declarations, so most users never see `Future<T>` in a signature. It only appears when writing library code that exposes async APIs.

Parallel requests use products of futures — no new syntax:

```
fetchBoth = (Network) -> (Json * Json) {
    (
        HttpRequest("https://api.example.com/a").send(Network) *
        HttpRequest("https://api.example.com/b").send(Network)
    ).join
}
```

---

## Current codebase — what to understand before writing any code

Read these files in full before implementing.

| File | Role |
|---|---|
| `src/ast.rs` | AST node definitions. Key: `FunctionDef` has `extern_rust: Option<ExternRust>` — this gets replaced. |
| `src/loader.rs` | Loads `.ow` files, resolves `use` against stdlib. `LoadResult` carries `cargo_deps` and `rust_preludes` — both deleted. |
| `src/main.rs` | CLI. `cmd_build` and `cmd_run` call `compile_with_rustc`/`compile_with_cargo` — both deleted. |
| `src/codegen/rust.rs` | The entire Rust emitter (~1600 lines). Deleted in full. |
| `src/codegen/mod.rs` | Re-exports `rust.rs`. Will re-export `wasm` module instead. |
| `src/parser/parser.rs` | Parses `extern Rust("...")` and `extern Rust.async("...")`. Updated to `extern Wasm`. |
| `src/checker/` | Target-agnostic. Only change: add `Future<T>` and `Stream<T>` as built-in generic types and insert `Expr::Await` nodes in Phase 5. |

---

## Step 0 — Deletions

Remove these files entirely. Do not migrate; do not preserve.

```
src/codegen/rust.rs
std/clock.ow
std/datetime.ow
std/filesystem.ow
std/http-client.ow
std/http-client.rs
std/http-server.ow
std/http-server.rs
std/json.ow
std/json.rs
std/path.ow
std/path.rs
std/url.ow
std/url.rs
```

---

## Step 1 — Update `Cargo.toml` (the compiler's own)

Add to `[dependencies]`:

```toml
# WASM encoding and Component Model
wasm-encoder  = "*"
wit-parser    = "*"
wit-component = "*"
wasmparser    = "*"   # for output validation

# Embedded runtime
wasmtime      = { version = "*", features = ["component-model", "async"] }
wasmtime-wasi = { version = "*" }
```

All are Bytecode Alliance crates. Pin `wasm-encoder`, `wit-parser`, and `wit-component` to the same minor version — they are released together and have shared internal types.

---

## Step 2 — AST changes (`src/ast.rs`)

Replace `ExternRust` with `ExternWasm`. Grep for `extern_rust` and `ExternRust` and update every reference.

```rust
// Delete:
pub struct ExternRust { pub path: String, pub is_async: bool }

// Add:
pub struct ExternWasm {
    pub path: String,    // e.g. "wasi:filesystem/types@0.3.0-rc-2026-03-15#read-via-stream"
    pub is_async: bool,
}

// In FunctionDef, replace:
pub extern_rust: Option<ExternRust>,
// with:
pub extern_wasm: Option<ExternWasm>,
```

Add a new AST node for the auto-await inserted by the checker in Phase 5:

```rust
// In the Expr enum, add:
Await {
    inner: Box<Expr>,
    span: Span,
},
```

Update `Expr::span()` to handle the new variant.

---

## Step 3 — Parser changes (`src/parser/parser.rs`)

Find where `extern Rust` is parsed. Replace:

```
extern Rust("path")         →  extern Wasm("path")
extern Rust.async("path")  →  extern Wasm.async("path")
```

The AST node produced is now `ExternWasm` instead of `ExternRust`. Logic is otherwise identical.

---

## Step 4 — Loader changes (`src/loader.rs`)

### Remove

- `CargoDep` struct and all references
- `rust_prelude` field from `StdlibEntry`
- `cargo_deps` field from `StdlibEntry`
- `cargo_deps` and `rust_preludes` fields from `LoadResult`
- `LoadCtx.cargo_deps` and `LoadCtx.rust_preludes`
- All code that accumulates or returns those fields
- `auto_include_json_if_needed` — no longer needed without Rust preludes

### `StdlibEntry` becomes

```rust
struct StdlibEntry {
    name: &'static str,
    source: &'static str,
}
```

### `LoadResult` becomes

```rust
pub struct LoadResult {
    pub module: Module,
    pub entry_items_start: usize,
}
```

### New `STDLIB` array

```rust
const STDLIB: &[StdlibEntry] = &[
    StdlibEntry { name: "Clock",      source: include_str!("../std/clock-wasm.ow") },
    StdlibEntry { name: "Filesystem", source: include_str!("../std/filesystem-wasm.ow") },
    StdlibEntry { name: "HttpClient", source: include_str!("../std/http-client-wasm.ow") },
    StdlibEntry { name: "HttpServer", source: include_str!("../std/http-server-wasm.ow") },
    StdlibEntry { name: "Json",       source: include_str!("../std/json-wasm.ow") },
    StdlibEntry { name: "Path",       source: include_str!("../std/path-wasm.ow") },
    StdlibEntry { name: "Random",     source: include_str!("../std/random-wasm.ow") },
    StdlibEntry { name: "Url",        source: include_str!("../std/url-wasm.ow") },
];
```

---

## Step 5 — Main CLI changes (`src/main.rs`)

### Remove

- `compile_with_rustc`
- `compile_with_cargo`
- `combine_source`
- `sanitize_crate_name`
- All imports used only by those functions

### `cmd_build`

```rust
fn cmd_build(args: &[String]) {
    let file_path = require_file(args);
    let loaded = load_or_exit(file_path);
    let errors = checker::check_with_entry(&loaded.module, loaded.entry_items_start);
    if !errors.is_empty() {
        for err in &errors { print_error(file_path, err); }
        eprintln!("\n{} error(s) found.", errors.len());
        process::exit(1);
    }
    let component_bytes = codegen::generate(&loaded.module);
    let build_dir = build_dir_for(file_path);
    let stem = Path::new(file_path).file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let out_path = build_dir.join(format!("{}.wasm", stem));
    fs::create_dir_all(&build_dir).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        process::exit(1);
    });
    fs::write(&out_path, &component_bytes).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        process::exit(1);
    });
    println!("Compiled to: {}", out_path.display());
}
```

### `cmd_run`

```rust
fn cmd_run(args: &[String]) {
    let file_path = require_file(args);
    let program_args: Vec<&str> = args.iter().skip(1).map(|s| s.as_str()).collect();
    let loaded = load_or_exit(file_path);
    let errors = checker::check_with_entry(&loaded.module, loaded.entry_items_start);
    if !errors.is_empty() {
        for err in &errors { print_error(file_path, err); }
        eprintln!("\n{} error(s) found.", errors.len());
        process::exit(1);
    }
    let component_bytes = codegen::generate(&loaded.module);
    runtime::run_component(&component_bytes, &program_args);
}
```

### `cmd_emit`

Update to print WAT (human-readable WASM text format) instead of Rust source. The WASM codegen exposes `generate_wat(module: &Module) -> String`.

---

## Step 6 — New `src/runtime.rs`

Wraps wasmtime. Keeps the runtime concern out of `main.rs`.

```rust
use wasmtime::*;
use wasmtime::component::*;
use wasmtime_wasi::*;

pub fn run_component(bytes: &[u8], args: &[&str]) {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.async_support(true);
    let engine = Engine::new(&config).unwrap();

    let wasi_ctx = WasiCtxBuilder::new()
        .inherit_stdio()
        .args(args)
        .build();

    let mut store = Store::new(&engine, wasi_ctx);

    let component = Component::new(&engine, bytes).unwrap_or_else(|e| {
        eprintln!("error: invalid wasm component: {e}");
        std::process::exit(1);
    });

    let mut linker: Linker<WasiCtx> = Linker::new(&engine);
    wasmtime_wasi::add_to_linker_async(&mut linker).unwrap();

    // Run the component's wasi:cli/run export
    let (command, _) = wasmtime_wasi::bindings::Command::instantiate(
        &mut store, &component, &linker
    ).unwrap();

    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(command.wasi_cli_run().call_run(&mut store))
        .unwrap();

    match result {
        Ok(()) => {}
        Err(()) => std::process::exit(1),
    }
}
```

Note: consult the `wasmtime-wasi` crate docs for the exact current API — the structure above is correct but method names may differ slightly across versions.

Add `pub mod runtime;` to `src/lib.rs`.

---

## Step 7 — Codegen module structure

### `src/codegen/mod.rs`

```rust
mod wasm;
pub use wasm::generate;
pub use wasm::generate_wat;
```

### New directory `src/codegen/wasm/`

```
src/codegen/wasm/
├── mod.rs          orchestration, GeneratedWasm, public API
├── types.rs        Oneway type → WASM value types + Component Model types
├── memory.rs       linear memory layout, bump allocator, string encoding
├── func.rs         compile FunctionDef → WASM function
├── expr.rs         compile Expr → WASM instructions
├── builtin.rs      built-in method implementations (.print, .add, .map, etc.)
└── component.rs    wrap core module → Component Model component, generate WIT world
```

---

## Phase 1 — Skeleton

**Done when:** `cargo build` succeeds. `oneway build hello.ow` runs without panicking on the pipeline — it prints a clear "not yet implemented" error from inside the WASM codegen. All commands that do not invoke the codegen (`check`, `fmt`, `ast`, `tokens`, `lsp`) continue to work correctly.

`src/codegen/wasm/mod.rs` at this stage:

```rust
use crate::ast::Module;

pub fn generate(module: &Module) -> Vec<u8> {
    todo!("WASM codegen not yet implemented")
}

pub fn generate_wat(module: &Module) -> String {
    todo!("WASM WAT emit not yet implemented")
}
```

---

## Phase 2 — Core WASM emission: hello world

**Goal:** `examples/hello.ow` compiles and runs correctly.

### `src/codegen/wasm/types.rs`

Map Oneway types to WASM representations:

| Oneway type | Core WASM | Component Model |
|---|---|---|
| `Int` | `i64` | `s64` |
| `Float` | `f64` | `f64` |
| `Bool` | `i32` (0/1) | `bool` |
| `Unit` | — (empty) | `()` |
| `String` | `(i32 ptr, i32 len)` | `string` |
| `Option<T>` | `i32` tag + T | `option<T>` |
| `Result<T, E>` | `i32` tag + payload | `result<T, E>` |
| `A + B + C` (union) | `i32` tag + payload | `variant { a, b(B), c(C) }` |
| `A * B * C` (product) | multi-value or mem ptr | `tuple<A, B, C>` |
| `List<T>` | `(i32 ptr, i32 len)` | `list<T>` |
| `Future<T>` | `i32` handle | `future<T>` |
| `Stream<T>` | `i32` handle | `stream<T>` |

### `src/codegen/wasm/memory.rs`

Every compiled component includes a bump allocator embedded as globals + a helper function:

```wat
(memory (export "memory") 1)
;; Lower 64KB: stack space. Heap grows upward from 65536.
(global $bump_ptr (mut i32) (i32.const 65536))

(func $alloc (param $size i32) (result i32)
  (local $ptr i32)
  global.get $bump_ptr
  local.set $ptr
  global.get $bump_ptr
  local.get $size
  i32.add
  global.set $bump_ptr
  local.get $ptr)
```

String literals are embedded as data segments at compile time. Dynamic strings use `$alloc`.

### `src/codegen/wasm/component.rs`

Determines the WIT world from which capabilities `main` declares, then uses `wit-component` to wrap the core module into a Component Model component.

**Capability → WASI P3 import mapping:**

| Oneway capability | WASI P3 import |
|---|---|
| `Stdout` | `wasi:cli/stdout@0.3.0-rc-2026-03-15` |
| `Stderr` | `wasi:cli/stderr@0.3.0-rc-2026-03-15` |
| `Stdin` | `wasi:cli/stdin@0.3.0-rc-2026-03-15` |
| `Clock` | `wasi:clocks/wall-clock@0.3.0-rc-2026-03-15` |
| `Random` | `wasi:random/random@0.3.0-rc-2026-03-15` |
| `Filesystem` | `wasi:filesystem/types@0.3.0-rc-2026-03-15` |
| `Network` | `wasi:sockets/tcp@0.3.0-rc-2026-03-15` |
| `HttpClient` | `wasi:http/outgoing-handler@0.3.0-rc-2026-03-15` |
| `HttpServer` | `wasi:http/incoming-handler@0.3.0-rc-2026-03-15` (export, not import) |

Generated WIT world for a command (e.g. `main = (Stdout) -> Unit`):

```wit
package oneway:app@0.1.0;

world app {
  import wasi:cli/stdout@0.3.0-rc-2026-03-15;
  export wasi:cli/run@0.3.0-rc-2026-03-15;
}
```

### Output validation

After every `generate()` call, validate with `wasmparser` before returning bytes:

```rust
fn validate(bytes: &[u8]) {
    let mut validator = wasmparser::Validator::new_with_features(
        wasmparser::WasmFeatures::all()
    );
    validator.validate_all(bytes).unwrap_or_else(|e| {
        eprintln!("internal error: generated invalid wasm: {e}");
        std::process::exit(1);
    });
}
```

**Phase 2 milestone:** `examples/hello.ow`, `examples/arithmetic.ow`, `examples/literals.ow` pass.

---

## Phase 3 — Full Oneway type system

**Goal:** All pure-compute examples pass. No I/O required.

### Products (`A * B * C`)

Products where all fields are scalars: emit as multi-value WASM returns. Products containing strings, lists, or other memory-allocated types: allocate a struct in linear memory with `$alloc`, pass and return as `i32` pointer. Field layout is alphabetical (matches Oneway's ordering rule). Offset of each field is the sum of sizes of preceding fields, padded to alignment.

### Unions (`A + B + C`)

In-memory layout: `(i32 tag, payload)` where payload is the size of the largest variant, padded to 8 bytes. Discriminants are assigned in alphabetical order of variant names.

Dispatch compiles to `block`/`br_table`:

```wat
;; dispatch on Bool (False=0, True=1)
block $end
  block $true
    block $false
      local.get $tag
      br_table $false $true
    end
    ;; False arm body
    br $end
  end
  ;; True arm body
end
```

### `Option<T>` and `Result<T, E>`

These are unions with fixed tag assignments:
- `Option`: `None` = 0, `Some` = 1
- `Result`: `Err` = 0, `Ok` = 1

The `?` operator compiles to: load tag, `br_if` to an early-return block if `Err`, else continue with the `Ok` payload.

### `List<T>`

Represented as `(i32 ptr, i32 len, i32 cap)` in linear memory. Built-in methods: `.map`, `.filter`, `.each`, `.len`. Implement as simple loops over contiguous memory.

### Generics — monomorphization

Maintain a `HashMap<(FunctionKey, Vec<ConcreteType>), wasm_encoder::FunctionIndex>` during codegen. When a generic function is called with concrete types, instantiate it if not already present in the map. This is identical in concept to Rust's approach.

### Closures / lambdas

Represent as a struct `(funcref, env_ptr)` in linear memory. The environment is a heap-allocated record of captured values allocated with `$alloc`. The closure trampoline is a WASM function that receives `env_ptr` as its first parameter.

**Phase 3 milestone:** `examples/match.ow`, `examples/list.ow`, `examples/tree.ow`, `examples/option.ow`, `examples/traits.ow`, `examples/types.ow`, `examples/field-access.ow`, `examples/methods.ow`, `examples/loop.ow`, `examples/commutative.ow` pass.

---

## Phase 4 — WASI P3 stdlib

**Goal:** All I/O examples pass. Every capability is wired up.

Each stdlib file is a pure `.ow` file with `extern Wasm` / `extern Wasm.async` declarations. No Rust. No preludes. The `extern Wasm` path format is `"wasi:namespace/interface@version#function-name"`.

### `std/stdout-wasm.ow`

```
extern Wasm("wasi:cli/stdout@0.3.0-rc-2026-03-15#get-stdout")
getOutputStream = (Unit) -> OutputStream

extern Wasm.async("wasi:io/streams@0.3.0-rc-2026-03-15#output-stream#blocking-write-and-flush")
writeBytes = (OutputStream * List<Byte>) -> Result<Unit, StreamError>

print = (Stdout * String) -> Unit {
    getOutputStream(Unit).writeBytes(String.bytes).unwrap
}
```

### `std/filesystem-wasm.ow`

```
use Path

extern Wasm.async("wasi:filesystem/types@0.3.0-rc-2026-03-15#read-via-stream")
read = (Filesystem * Path) -> Result<String, IoError>
```

### `std/http-client-wasm.ow`

```
extern Wasm.async("wasi:http/outgoing-handler@0.3.0-rc-2026-03-15#handle")
send = (HttpClient * HttpRequest) -> Result<HttpResponse, HttpError>
```

### `std/http-server-wasm.ow`

HTTP server in WASI P3 uses an exported handler rather than an imported server. The component exports `wasi:http/incoming-handler`. The compiler detects `HttpServer` in `main`'s capability parameters and switches the world to include this as an export instead of an import.

### `std/json-wasm.ow`

Implement as a pure-Oneway JSON parser/serializer or bind a WASM-compiled JSON library via component composition. No WASI interface exists for JSON — it is a pure-compute library.

### `std/path-wasm.ow`

```
Path = String
```

Path is a thin wrapper over String in the WASM world. Filesystem operations take a `Path` and interact with WASI preopened directories.

### `std/url-wasm.ow`

URL parsing is pure-compute. Implement in Oneway or bind a compiled WASM component.

### `std/clock-wasm.ow`

```
extern Wasm("wasi:clocks/wall-clock@0.3.0-rc-2026-03-15#now")
now = (Clock) -> Datetime
```

### `std/random-wasm.ow`

```
extern Wasm("wasi:random/random@0.3.0-rc-2026-03-15#get-random-u64")
randomInt = (Random) -> Int
```

**Phase 4 milestone:** `examples/read-file.ow`, `examples/fetch-url.ow`, `examples/now.ow`, `examples/parse-json.ow`, `examples/json-literal.ow`, `examples/extern.ow` pass.

---

## Phase 5 — Async: `Future<T>`, `Stream<T>`, Component Model async

**Goal:** All async examples pass. HTTP server example passes.

### New built-in types in the checker

Add `Future` and `Stream` to the set of recognized built-in generic type names alongside `Option`, `Result`, `List`, `Map`, `Set`. These are valid anywhere a type is expected.

### Auto-await rule

Add to the checker's type inference / unification logic:

> When an expression of type `Future<T>` is used in a position that expects `T` — as a method receiver, a function argument, or an operand of `?` — the checker wraps it in `Expr::Await` and marks the enclosing function as suspending.

This is purely a checker-level transformation. The parser never produces `Expr::Await`. The user never writes it.

```rust
// Pseudocode for the checker rule:
if actual_type == Type::Future(inner) && expected_type == *inner {
    *expr = Expr::Await { inner: Box::new(expr.clone()), span: expr.span() };
    mark_current_function_suspending();
}
```

### Async inference — shared module

Extract the bottom-up fixpoint from `rust.rs` (before it is deleted) into `src/codegen/async_analysis.rs`. The WASM codegen uses this directly.

New trigger conditions beyond what the Rust backend had:
- Function body contains `Expr::Await` (consumes a `Future<T>`)
- Function calls `.each` or `.next` on a `Stream<T>`

The fixpoint loop is identical: seed the async set with direct triggers, then propagate to all callers until stable.

### `Expr::Await` compilation (`src/codegen/wasm/expr.rs`)

`Expr::Await` compiles to the Component Model async call lowering. In the core WASM module this is represented as a suspendable call using `wasm-encoder`'s async instruction support. The Component Model runtime handles stack switching at the host boundary.

### Async world generation (`src/codegen/wasm/component.rs`)

Functions marked as suspending are emitted as `async` in the WIT world. `wit-component` handles the canonical ABI for async functions automatically when wrapping the core module.

### `Stream<T>` — `.each(lambda)` compilation

Compiles to a loop calling the Component Model stream poll until the stream is exhausted:

```wat
;; Stream<T>.each(fn: (T) -> Unit) — structural sketch
loop $stream_loop
  call $stream_next       ;; → (i32 tag, payload): 0 = end, 1 = item
  i32.eqz
  br_if $stream_end       ;; tag 0 = stream closed, exit
  ;; tag 1 = item present: call lambda with payload
  call_indirect $lambda_fn
  br $stream_loop
end
$stream_end:
```

The actual WASI P3 stream poll uses the Component Model's native `stream<T>` suspension — the implementation is encapsulated in `builtin.rs`.

**Phase 5 milestone:** `examples/http-server/` passes. `oneway run` on all examples in `examples/` produces correct output.

---

## Testing strategy

### Existing unit tests

`cargo test` must pass from Phase 1 onward. The lexer, parser, formatter, and checker tests do not touch the codegen and are unaffected by this work.

### WASM example tests

Add a `just examples-wasm` recipe mirroring the existing `just examples`:

```just
examples-wasm:
    #!/usr/bin/env bash
    pass=0; fail=0; skip=0
    for ow in examples/*.ow; do
        stem=$(basename "$ow" .ow)
        expected="examples/expected/$stem.txt"
        if [ ! -f "$expected" ]; then skip=$((skip+1)); continue; fi
        got=$(cargo run --quiet -- run "$ow" 2>&1)
        if [ "$got" = "$(cat "$expected")" ]; then
            pass=$((pass+1))
        else
            fail=$((fail+1))
            echo "FAIL: $stem"
            diff <(echo "$got") "$expected"
        fi
    done
    echo "passed=$pass failed=$fail skipped=$skip"
    [ "$fail" -eq 0 ]
```

Add `examples/expected/*.txt` files with the expected stdout for each example.

### Output validation

Every call to `generate()` runs `wasmparser` validation before returning bytes. This catches codegen bugs immediately with a clear error message rather than a cryptic wasmtime trap at runtime.

---

## Phase completion checklist

| Phase | Done when |
|---|---|
| 1 — Infrastructure | `cargo build` passes. CLI plumbing works. Codegen stubs compile. `fmt`, `check`, `ast`, `tokens`, `lsp` all work. |
| 2 — Hello world | `hello.ow`, `arithmetic.ow`, `literals.ow` pass |
| 3 — Type system | `match.ow`, `list.ow`, `tree.ow`, `option.ow`, `traits.ow`, `types.ow`, `loop.ow`, `field-access.ow`, `methods.ow`, `commutative.ow` pass |
| 4 — WASI stdlib | `read-file.ow`, `fetch-url.ow`, `now.ow`, `parse-json.ow`, `json-literal.ow`, `extern.ow` pass |
| 5 — Async | `http-server/` passes. All examples pass. |

---

## What the implementing agent must NOT do

- Do not keep any code path that generates Rust source or calls `rustc`/`cargo` for user programs.
- Do not add a `--target` flag. WASM is the only target.
- Do not preserve `extern Rust` syntax in the parser. It is gone.
- Do not preserve `CargoDep`, `rust_preludes`, or any field derived from them. They are gone.
- Do not use `tokio`, `axum`, `reqwest`, or any Rust async runtime crate in generated code. Those were for the Rust backend.
- Do not implement `Future<T>` or `Stream<T>` as explicit handles that the user must `.await`. They auto-await at method call sites. The user writes nothing.
- Do not break `oneway fmt`, `oneway check`, `oneway ast`, `oneway tokens`, or `oneway lsp`. These are target-agnostic and must continue working from Phase 1 onward.
- Do not emit the Rust backend path "temporarily" or "as a fallback". It is deleted. If WASM codegen is incomplete, the error is "not yet implemented", not a fallback to Rust.
