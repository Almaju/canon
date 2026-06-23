# Streams & Server-Sent Events — Design

Status: **design agreed, implementation not started**. This document
defines how Canon programs produce values *over time* — concretely:
how they construct a `Stream<T>` and hand it to a sink (an HTTP response
body, a file, a socket). Server-Sent Events fall out as a four-line
formatter on top.

This document is a peer of `WASI-HTTP-HANDLER.md`. SSE is not a
language-level concept and not a dedicated stdlib type; it is the
combination of:

1. A `Content-Type: text/event-stream` header.
2. A response body of type `Stream<String>` whose elements are SSE frames.

Both already fall out of decisions that DESIGN.md committed to —
`Stream<T>` as a language primitive and `Response` as a value carrying
its body. The work in this doc is wiring the canonical-ABI lowering for
`Stream<T>` plus a curated `canon/std/Stream` combinator surface.

---

## Target user-facing shape

A clock-driven SSE endpoint:

```ow
use canon/std/Stream
use canon/std/http/Headers
use canon/std/http/Request
use canon/std/http/Response
use canon/std/http/Status
use canon/std/http/eventStream
use canon/std/http/formatSse
use canon/std/time/Duration
use canon/std/time/ticks

home = (Request) -> Response {
    Response(
        eventStream(),
        Status(200),
        ticks(Duration(Seconds(1)))
            .map((Instant) -> String { Instant.toRfc3339() })
            .map(formatSse)
            .take(10)
    )
}
```

Using only slice-0 primitives (no clock source yet) the same shape
looks like:

```ow
use canon/std/Stream
use canon/std/http/Headers
use canon/std/http/Request
use canon/std/http/Response
use canon/std/http/Status
use canon/std/http/eventStream
use canon/std/http/formatSse

home = (Request) -> Response {
    Response(
        eventStream(),
        Status(200),
        List("hello", "world", "done")
            .streamOf()
            .map(formatSse)
    )
}
```

This runs as soon as slice 3 (`Response` constructor accepting a
`Stream<String>`) and the prerequisite WASI-HTTP slices land. Slice 0
proves the type surface compiles.

What's in this snippet, by layer:

- `eventStream = () -> Headers` — convenience constructor in
  `canon/std/http/sse.can` returning `Headers().set("content-type", "text/event-stream")`.
- `formatSse = (String) -> String` — `"data: ".concat(String).concat("\n\n")`. Also in `sse.can`.
- `ticks = (Duration) -> Stream<Instant>` — stream source in
  `canon/std/time`. Yields a new `Instant` per duration. Async by
  virtue of returning `Stream<T>`.
- `.map`, `.take` — pure Canon combinators in `canon/std/Stream`.
- `Response : (Headers * Status * Stream<String>) -> Response` — the
  curated constructor. Internally drives `wasi:http/types`'
  `writable-body` from the supplied stream.

Nothing the user types references SSE-the-protocol beyond the two
helper names. SSE is just the obvious composition of a Content-Type and
a stream.

A non-streaming response in the same shape:

```ow
home = (Request) -> Response {
    Response(Headers(), Status(200), Stream("hello"))
}
```

`Stream(s : String) -> Stream<String>` is the single-element source —
the materialised case from the chat discussion. This replaces the
current `(Headers * Status * String) -> Response` constructor; static
bodies become a one-token wrap.

---

## Design principles

These are the ones we agreed on in chat, restated so the codegen author
has a checklist:

1. **Pull-only.** `Stream<T>` flows *into* a sink (response body, file,
   stdout). No `SseSender` capability, no callback-style push API. The
   response is still a value; the host pulls bytes when it's ready to
   send them.

2. **`Stream<T>` is a handle.** Backed by `wasi:io/streams` at the
   canonical-ABI boundary, exactly like every other WIT resource. The
   compiler emits `resource.drop` at end-of-scope. From the source's
   point of view it's an opaque `Handle` with methods bound by
   `canon/std/Stream`.

3. **Combinators are pure Canon**, hand-written in
   `packages/canon/std/src/stream.can`, layered over
   `canon/wasi/io/streams`. The standard `canon/std` ↔ `canon/wasi`
   split applies — no privileged shape for stream combinators.

4. **Stream sources come from bindings**, never from magic. `ticks`,
   `fileLines`, `socketRecv` are all `extern Wasm` declarations in
   their respective `canon/std` files (wrapping the matching
   `canon/wasi/*` binding). Holding a `Stream<Instant>` *is* the
   capability to iterate ticks — domain-first, same as `File`, `Url`,
   etc.

5. **One `Response` constructor.** `(Headers * Status * Stream<String>)`
   is the only shape. `String` bodies become `Stream("…")`. No
   overload, no special case.

6. **No `async` keyword.** Iterating a `Stream<T>` makes the surrounding
   function suspending; the compiler propagates suspending-ness up the
   call graph and lifts affected functions as `async func(…)` in the
   emitted component. This is already specified in DESIGN.md §Async.

7. **SSE is not a primitive.** No `SseEvent` type, no `SseFrame` union,
   no `[event]`/`[id]`/`[retry]` machinery in the core stdlib. Two
   helpers in `canon/std/http/sse.can` (`eventStream`, `formatSse`)
   cover the 99% case. If anyone needs richer frames later (custom
   `event:` names, `id:` tags), they write their own four-line
   formatter — same composition pattern.

---

## The `canon/std/Stream` surface

Decided shape. Subject to bikeshed on names but not on structure:

```ow
# packages/canon/std/src/stream.can
bindings "canon:builtins/stream@0.1.0"

# `Stream<T>` is a built-in generic type in the checker (see
# BUILTIN_GENERIC_TYPES in src/checker/mod.rs) — do NOT re-declare it
# as `Stream<T> = Handle` here; that's a duplicate-type error. The
# canonical-ABI lowering treats it as a Handle behind the scenes.

# ── Sources ───────────────────────────────────────────────
# Finite stream from a list. Eager — list is fully materialised.
streamOf = <T>(List<T>) -> Stream<T>

# Empty stream. Used as the identity of concat / as a "no events" body.
empty = <T>() -> Stream<T>

# ── Combinators ──────────────────────────────────────────────
map = <A, B>(Stream<A> * (A) -> B) -> Stream<B>
filter = <T>(Stream<T> * (T) -> Bool) -> Stream<T>
take = <T>(Stream<T> * Int) -> Stream<T>
concat = <T>(Stream<T> * Stream<T>) -> Stream<T>

# ── Sinks (consume a stream end-to-end) ────────────────────────
# Used by helpers that need a materialised value. Suspending.
toList = <T>(Stream<T>) -> List<T>
toString = (Stream<String>) -> String
```

Implementation notes:

- The actual stdlib file (`packages/canon/std/src/stream.can`) uses a
  single `bindings "<urn>"` directive at the top + camelCase
  declarations. The loader's `apply_bindings_directive` rewrites each
  camelCase function-type alias into a `FunctionDef` with `extern_wasm`
  populated and the first product field extracted as the receiver. So
  `streamOf = (List<T>) -> Stream<T>` becomes a method on `List`,
  callable as `someList.streamOf()`.
- **PascalCase constructors (`Stream(list)`) are deferred to slice 2.**
  The bindings directive only auto-promotes lowercase names; PascalCase
  body-less aliases stay as function-type aliases, not callable
  constructors. Once slice 2 lands pure-Canon combinators with
  bodies, a curated `Stream = <T>(List<T>) -> Stream<T> { streamOf(…) }`
  wrapper can sit alongside.
- `scan` and `generate` are dropped from the slice-0 surface. `scan` is
  a nice-to-have combinator; `generate` is the from-scratch escape
  hatch and brings a product-type-inside-generic parsing edge case
  (`Option<T * Stream<T>>`) that we'll sort out when we actually need
  it. Neither blocks the SSE end goal.
- `toList` and `toString` are sinks for the rare case where the
  consumer wants the whole stream as a value. They're declared sync
  for slice 0; once codegen wires `Stream<T>` consumption through the
  async-propagation rule, their signatures will widen to
  `Future<List<T>>` / `Future<String>` (and the auto-await transform
  takes care of every call site).

### Checker support (landed in slice 0)

`src/checker/mod.rs::method_return_summary` previously peeled both
`Future<T>` and `Stream<T>` to `T` when computing the summary the
methods-table stores. That was wrong for `Stream<T>`: stream-producing
functions need their return type preserved so downstream combinators
(`.map`, `.take`, `.toString`) resolve. After slice 0 only `Future<T>`
is peeled — `Stream<T>` stays as-is. Stream consumption (the
auto-iteration that makes the surrounding function suspending) is
still handled at call sites via `.each` / `.next` recognition in
`async_analysis::expr_has_async_trigger`, per the `auto_await` module
docstring. Pinned by `tests/checker/ok/stream_compose.can`.

Stream sources outside the core file live with their domain:

| Source | Module | Returns |
|---|---|---|
| `ticks(Duration)` | `canon/std/time` | `Stream<Instant>` |
| `fileLines(File)` | `canon/std/fs` | `Stream<String>` |
| `socketRecv(Socket)` | `canon/std/net` (future) | `Stream<Bytes>` |
| `requestBody(Request)` | `canon/std/http` | `Stream<Bytes>` |

Each is one line in its respective file — an `extern Wasm` declaration
backed by the matching WIT stream-returning function.

---

## Lowering: how `Stream<T>` reaches the wire

The canonical-ABI shape is unchanged from WASI Preview 3:

- A `Stream<T>` value is a single i32 handle in the core ABI.
- The host owns the underlying `wasi:io/streams` resource.
- The guest manipulates it through `[method]…` calls (read, close,
  drop).

The codegen pieces:

1. **Handle lowering for `Stream<T>`.** Extend the `extern Wasm`
   import lowering (the CLAUDE.md gap row "WIT `resource` / `own<T>` /
   `borrow<T>` in `extern Wasm` signatures") to recognise
   `stream<T>` returns and parameters. The canonical-ABI shape is the
   same as any other resource handle.

2. **Async propagation through `Stream<T>`-consuming bodies.** Already
   specified in DESIGN.md §Async. The current codegen doesn't yet
   propagate through stream-iteration — that's part of this work.

3. **Combinator implementation strategy.** Two options, pick one
   during implementation:
   - **Option A (host-backed):** each combinator (`map`, `filter`,
     `take`) is an `extern Wasm` binding in `canon/std/Stream`
     calling a host helper that wraps the input stream with a
     transformer. Lowest-cost at the canonical-ABI boundary, but
     pushes work into the host runtime.
   - **Option B (guest-implemented):** each combinator is pure Canon
     calling `[method]stream.read` in a loop, formatting / filtering /
     counting, and writing into a fresh output stream. Heavier in
     terms of canonical-ABI traffic, but lives entirely in the guest —
     no host extensions, fully portable.

   Recommendation: **Option B for the first cut**. Portability matters;
   `wasmtime serve` shouldn't need to know about Canon's combinators.
   Optimisation passes can fuse adjacent guest combinators later.

4. **`Response` constructor wiring.** The curated
   `(Headers * Status * Stream<String>) -> Response` constructor in
   `canon/std/http/response.can`:
   - Calls the WIT `[constructor]response(status, headers)` to get
     the response handle.
   - Calls `[method]response.body` to get the writable-body handle.
   - Calls `[method]writable-body.write` to get the outgoing
     `stream<u8>`.
   - Pipes the user's `Stream<String>` into the outgoing `stream<u8>`
     (per-element: encode UTF-8 bytes, write to the outgoing stream,
     await flush).
   - Calls `[method]writable-body.finish` when the input stream ends.
   - Returns the response handle.

   The pipe step is the only nontrivial wiring; it's a small
   guest-side function (`Stream<String>` → write loop into
   `stream<u8>`) that ships in `canon/std/http/response.can`. No host
   help required.

---

## Implementation slicing

Each slice ends with a green `tests/runtime/` test. Slices are ordered
by what unblocks user-visible value fastest.

### Slice 0 — stdlib skeleton ✅ landed

Deliverable: the `canon/std/Stream` source file exists with the
declared shape. The checker accepts a program that imports and uses
the new types.

Shipped:

- `packages/canon/std/src/stream.can` — `bindings
  "canon:builtins/stream@0.1.0"` directive with camelCase combinator
  declarations (`streamOf`, `empty`, `map`, `filter`, `take`, `concat`,
  `toList`, `toString`). The bindings directive auto-promotes each
  camelCase alias into a `FunctionDef` with `extern_wasm` populated.
- `packages/canon/std/src/http/sse.can` — `eventStream` and `formatSse`
  helpers. Headers re-export comes from the sibling `headers.can`.
- `tests/checker/ok/stream_compose.can` — exercises
  `.streamOf().map().filter().take().concat().toString().print()`.
- One-line checker fix in `src/checker/mod.rs::method_return_summary`:
  stop peeling `Stream<T>` (was over-aggressive; broke method-chain
  resolution on stream-producing functions). `Future<T>` is still
  peeled because the auto-await rewrite depends on it.

Codegen can't build the resulting component yet — runtime tests come in
slices 1–4.

### Slice 1 — `Stream<T>` codegen for handles

**Slice 1a (prototype, landed) —** `tests/wit_component_stream_prototype.rs`
proves `wit_component::ComponentEncoder` accepts a guest core module
whose signature carries `stream<u8>` (both as a return and as a
parameter). No per-stream lowering on our side; the encoder emits the
entire canonical-ABI type section from embedded WIT metadata. Paired
with the existing `tests/wit_component_prototype.rs` (resources +
variants + sub-u64 ints), this proves the architectural shortcut from
`WASI-HTTP-HANDLER.md` §Slice-1b extends to streams.

**Slice 1b (integration, not landed) —** rewire the main codegen path
(`src/codegen/wasm/mod.rs` + `src/codegen/wasm/component.rs`) so that
programs whose surface mentions `Stream<T>` go through
`ComponentEncoder` instead of the hand-rolled `wasm-encoder` type/import
sections. This is the same integration that unblocks the WASI HTTP
handler. Doing both at once is the recommended sequencing — the type
shape (`stream<u8>`, `own<request>`, `result<own<response>, error-code>`)
is the same machinery on the encoder side.

Work shape for slice 1b:

- Detect when the program uses a feature the hand-rolled path doesn't
  handle (any `extern Wasm` with `Stream<T>` in its signature, any HTTP
  entry, any `Handle`-typed parameter). Today this set is silently
  skipped (`build_extern_component_params` returns `None`, the import
  gets dropped, runtime instantiation fails because the host import
  isn't satisfied).
- For those programs, route through `ComponentEncoder::module(…)`:
  build the core module's `import` / `export` sections with the
  `<iface>#<func>` naming convention the encoder looks for, embed the
  matching WIT via `embed_component_metadata`, run
  `ComponentEncoder::default().module(…).encode()`.
- Leave the existing `wasi:cli/command` world emission alone — the CLI
  programs keep their hand-rolled path until/unless we decide to
  consolidate.

Test: an `extern Wasm` declaration for `wasi:io/streams.read` is
callable from a tiny Canon program and returns a byte buffer the guest
can read.

Strategic note: slice 1a is the smallest landable proof; the work in
slice 1b is what actually unblocks a user-runnable streaming program.

### Slice 2 — `canon/std/Stream` combinators (guest-side)

Deliverable: `map`, `filter`, `take`, `concat`, `empty`, `Stream`
sources all work. Implementation is pure Canon looping over
`[method]stream.read` and writing into a fresh outgoing stream.

Test: `tests/runtime/stream_compose.can` — build a stream from a list,
map it, take the first 3 elements, materialise via `toString`. Assert
the result.

### Slice 3 — `Response` constructor accepting `Stream<String>`

Deliverable: the curated `Response` constructor consumes a
`Stream<String>` and drives the writable-body. Replaces the
`(Headers * Status * String) -> Response` constructor; `String` bodies
go through `Stream("…")` sugar.

Dependencies: WASI-HTTP-HANDLER.md slices 1b–3 (the handler export
itself).

Test: `tests/runtime/wasi_http_handler_streamed_body.can` — handler
returns `Response(Headers(), Status(200), Stream(List("a", "b", "c")))`.
Harness asserts the wire body is `"abc"`.

### Slice 4 — SSE helpers + a real time-driven endpoint

Deliverable:

- `packages/canon/std/src/http/sse.can` — `eventStream` and
  `formatSse`.
- `canon/std/time/ticks` — stream source backed by WASI clocks.

Test: `tests/runtime/wasi_http_handler_sse.can` — handler streams 3
clock ticks formatted as SSE frames. Harness reads the response
incrementally and asserts each `data: …\n\n` frame arrives with
roughly the right delay between them.

This is the slice that delivers the user-visible promise. After it,
"Canon can do real SSE" is true.

### Slice 5 — cleanup

- Delete `parse_handler_response` and the `Content-Type:` prefix hack
  in `src/runtime.rs::host_builtin_http_server`.
- Delete `tests/http_handler_test.rs::dynamic_handler_sse_content_type`.
- Update the closed-gap row "SSE / streaming-response Content-Type" in
  `CLAUDE.md` to point at this doc.
- Update the deprecation pointer in `WASI-HTTP-HANDLER.md` slice 5 to
  link here.

---

## Open questions (deferred, not blocking)

1. **Backpressure.** When the host's outgoing `stream<u8>` blocks, the
   guest's pipe loop should suspend. Canonical-ABI streams already
   model this — `stream.write` returns a future that resolves when the
   write completes. The pipe loop awaits each write naturally through
   the same async-propagation machinery that handles every other
   suspending call. No new design needed; just careful implementation.

2. **Stream of stream-of-T (`Stream<Stream<T>>`).** Useful for "events
   grouped by session" patterns. Falls out of the type system for free;
   `flatten = (Stream<Stream<T>>) -> Stream<T>` is a natural combinator
   if anyone needs it. Not in the slice 2 surface.

3. **Cancellation.** When the client closes the SSE connection, the
   host detects it and drops the outgoing `stream<u8>`. The guest's
   pipe loop sees the next `stream.write` fail and exits. The user's
   `Stream<String>` is dropped, which triggers `resource.drop` on its
   handle — closing any underlying ticker. This is automatic with the
   linear-ownership model from DESIGN.md §Memory Model.

4. **Push sources (channels).** When concurrency primitives land
   (DESIGN.md §Concurrency), a `Channel<T>` adapter turns push
   producers into pull `Stream<T>`s. That work is out of scope here —
   it slots in cleanly once channels exist.

5. **Richer SSE frames.** `formatSse` only emits `data: …\n\n`. If a
   user needs `event:`, `id:`, or `retry:` fields, they write their
   own formatter — same one-liner pattern. We resist baking these into
   stdlib until there's pull from real use cases. If they do bake in,
   the natural shape is a `SseFrame` product with `Option<String>`
   fields for the optional bits and a single `format = (SseFrame) -> String`
   helper.

---

## File-by-file change list (slice 0, optional)

| File | Change |
|---|---|
| `packages/canon/std/src/stream.can` | **New.** Declarations from the surface table above; bodies as `# todo` stubs. |
| `tests/checker/ok/stream_compose.can` | **New.** A small program importing `Stream`, calling `.map`, `.take`, `.concat`. Confirms the checker accepts the type surface. |
| `STREAMING.md` | **New.** This file. |
| `WASI-HTTP-HANDLER.md` | **Update.** Slice 5 ("async bodies & streaming") deprecation-pointers to here. |
| `CLAUDE.md` | **Update.** Recently-closed gap row "SSE / streaming-response Content-Type" gets a deprecation note pointing to this doc; a new open-gap row tracks the slice-1/2 codegen work. |
