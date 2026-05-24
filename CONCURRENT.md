# Concurrency Combinators — Design

> **Status: shipped.** `parallel(a, b)` and `race(a, b)` work end-to-end.
> Runtime fixtures: `tests/runtime/parallel_two_echoes.ow` and
> `tests/runtime/race_two_echoes.ow`. This doc documents the architecture
> for future maintainers; the slice plan that was here originally ended
> up not being needed — see "Why this didn't need slice 1" below.

## Surface (final, shipped)

`packages/oneway/std/src/concurrent.ow`:

```
bindings "oneway:builtins/concurrent@0.1.0"

parallel = <T>(Future<T> * Future<T>) -> Future<List<T>>
race     = <T>(Future<T> * Future<T>) -> Future<T>
```

`parallel` is `Promise.all`-shaped: both futures must produce the same
payload type, results come back in arg-order as a `List<T>`. `race` is
`Promise.race`: returns the first to finish, cancels the loser. Both are
recognised by name in the codegen — no host bridge.

Usage:

```oneway
use oneway/std/concurrent

main = () -> Unit {
    parallel("hello".slowEcho(), "world".slowEcho())
        .toJsonArray()
        .print()
    # Prints: [hello,world]
}
```

## Architecture

### Step 1 — Bindings are filtered out of extern collection

`collect_extern_imports` (in `src/codegen/wasm/mod.rs`) skips any extern
whose component namespace starts with `oneway:builtins/concurrent`:

```rust
if component_ns.starts_with("oneway:builtins/concurrent") {
    continue;
}
```

So no wasm import is emitted and the linker doesn't go looking for a
host implementation. The bindings declaration still exists in the AST
so the loader / auto-await / checker can reason about the type
signature, but at the wasm level the names are entirely synthetic.

### Step 2 — Checker recognises the combinators

`check_expr`'s `Expr::Constructor` arm accepts `parallel(…)` and
`race(…)` calls of arity 2 unconditionally (the bindings declaration's
PascalCase first parameter `Future<T>` would otherwise force them to be
looked up as methods on `Future`, which the `Constructor(…)` call shape
doesn't support). `expr_type_name_in_scope` reports `List` for `parallel`
and propagates the inner type of the first arg for `race`, so subsequent
method chains (`.toJsonArray()`, `.print()`) type-check normally.

See the `CONCURRENT_COMBINATORS` const in `src/checker/mod.rs`.

### Step 3 — Codegen dispatches to inline builders

`compile_constructor` in `src/codegen/wasm/mod.rs` matches `parallel`
and `race` by name (before the user-defined-types fallback) and
dispatches to `compile_parallel` / `compile_race`.

### Step 4 — Each arg is compiled non-blocking

`emit_arg_as_nonblocking(arg, scope, f, subtask_local, retarea_local)`
is the new codegen helper that:

1. Identifies the callee FuncInfo (looks up the arg's MethodCall or
   Constructor target in `func_table`).
2. Validates it's an `extern Wasm.async` import.
3. Pushes the arg's receiver + sub-args onto the stack.
4. Allocs a ret-area (sized by the callee's `result_ty`) and
   `LocalTee`s into `retarea_local`.
5. Calls the async-lowered import → packed status word on the stack.
6. Extracts the subtask handle (`status >> 4`) into `subtask_local`.

This is the **non-blocking dual** of the existing `emit_async_call`,
which immediately enters a wait block. The split lets the multi-subtask
case start N calls before waiting on any of them.

### Step 5 — Wait-on-many sequence

`compile_parallel` then emits:

```
set = waitable-set.new()
waitable.join(subtask_a, set)
waitable.join(subtask_b, set)
event_area = alloc(8)
seen_a = 0
seen_b = 0
block $break
  loop $continue
    waitable-set.wait(set, event_area)  ;; drop returned event-code
    handle = i32_load(event_area + 0)
    if handle == subtask_a: seen_a = 1
    if handle == subtask_b: seen_b = 1
    if seen_a & seen_b: br $break
    br $continue
  end
end
subtask.drop(subtask_a)
subtask.drop(subtask_b)
waitable-set.drop(set)
```

The event-payload layout (`handle` at +0, `code` at +4) comes from
wasmtime's `waitable_check` implementation. The two-subtasks-drop
**must** happen before the set-drop because the subtasks are children
of the set (see `ResourceTableError::HasChildren` in wasmtime).

### Step 6 — Result construction

For `parallel`: alloc a 2-element `List<T>`, copy `(ptr, len)` pairs
from each ret-area to the appropriate slot, return `(list_ptr, 2)` as
`Ty::List`. Only `Ty::Str` / `Ty::NamedStr` element shapes are decoded
today; `Ty::I64` / `Ty::F64` have a code path but no test coverage.

For `race`: one `waitable-set.wait` instead of a loop. The winner is
identified by comparing the event handle against `subtask_a`; the loser
is cancelled via `canon.subtask.cancel(false)`. The winner's ret-area
is selected via `Select` and decoded onto the stack.

### Step 7 — `canon.subtask.cancel` intrinsic

A new entry in the `oneway:async/waitable` synthetic instance:

- `component.rs::wrap` emits `canon.subtask_cancel(false)` after
  `canon.task_return`. The `async_ = false` flag means the cancel
  blocks until acknowledged, which is permitted because `run` is
  lifted async-stackful.
- `mod.rs` imports it from `oneway:async/waitable.subtask-cancel`
  with core type `(i32) -> (i32)` (subtask handle → new state code).
- `fn_subtask_cancel` is added to `WasmGen` and points at the new
  import index (`base_waitable + 6`).

The `base_defined` index bumps by 1 to make room. The `run_core_fn` and
`handler_core_fn` indices in `component.rs::wrap` shift accordingly
(from `12 + N` and `13 + N` to `13 + N` and `14 + N`).

## Why this didn't need slice 1

The original plan in this doc proposed routing through
`wit_component::ComponentEncoder` so the encoder could lower `future<T>`
parameters at the import boundary. **That turned out not to be needed**
because the final implementation never treats `Future<T>` as a
first-class canonical-ABI value at the import boundary.

Instead:

- The args to `parallel(a, b)` are themselves async-extern call
  expressions in the source (e.g. `"hello".slowEcho()`).
- The codegen recognises this shape and compiles each arg as a
  *non-blocking* async call via `emit_arg_as_nonblocking`, capturing
  the subtask handle + ret-area into named locals.
- The wait-on-many sequence runs entirely inside the calling function.
- No synthetic `future<T>` canonical-ABI handle ever crosses an import
  boundary.

Routing through `ComponentEncoder` becomes the cheaper / more general
solution if/when a future codegen wants to:

- Pass `Future<T>` values through method-return positions (e.g.
  `fetchA()` stored in a local, then passed to multiple combinators).
- Accept `Future<T>` as a parameter to user-defined functions.
- Implement `Stream<T>` consumption (`STREAMING.md` slice 1b).

None of those are needed for the canonical `parallel(call, call)` /
`race(call, call)` patterns, which always inline the async call at the
combinator site.

## Open follow-ups

These are documented in `WASM.md` § "Phase 5 gaps":

1. **Sync-completion arms** — if one of the args completes synchronously
   (CallState = Returned, subtask handle = 0), `waitable.join(0, set)`
   traps. Today this is fine because every realistic async extern (HTTP,
   filesystem, timer) yields at least once, but a robust implementation
   should branch on the status word and mark synchronously-completed
   arms as immediately `seen`.

2. **N-arity parallel / race** — currently fixed at exactly 2 args.
   Extending to 3+ is mechanical (more scratch locals + iteration over a
   variadic arg list) but no user has asked for it.

3. **`List<Future<T>>` -> `Future<List<T>>` shape** — the natural
   continuation from #2 when you want to fan out over a runtime-sized
   list. Requires `Future<T>` reification (the slice-1 work that this
   implementation avoided), so it lands later if at all.

4. **`Stream<T>` propagation through `Race`** — open question: when one
   branch of `race` is a stream, does cancellation drain the stream or
   abort it? Spec-level question, defer until we have a use case.

## Why this matters

With these combinators landed, Oneway has a complete async surface:

- **Sequential**: every method call on a `Future<T>` auto-awaits.
- **Parallel**: `parallel(call, call)` runs two futures concurrently and
  returns a `List<T>` of results.
- **Racing**: `race(call, call)` returns the first to finish, cancels
  the loser.

All without an `async` keyword in source. The user writes flat method
chains; the compiler inserts the right canonical-ABI choreography
underneath. That's the philosophy in `docs/src/tour/async.md`.
