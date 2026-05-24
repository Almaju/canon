# Dynamic HTTP Handlers — Design

> **SUPERSEDED.** This document targets a custom
> `oneway:http-handler/handler@0.1.0` interface built on top of the
> `oneway:builtins/http-server` host bridge. That architecture is no
> longer the plan — see `WASI-HTTP-HANDLER.md` for the current
> direction (standard `wasi:http/handler` export, runnable on any
> compliant host). The slicing below remains useful as a reference
> for the callback-ABI mechanics, but the user-facing API and the
> on-the-wire interface are different.

---

Status: **not yet implemented**. This document captures the architecture
for Gap 1 (dynamic handlers) and Gap 6 (SSE streaming) from the Coulisse
port report (see `CLAUDE.md` § "Known codegen gaps"), so that the next
session — or contributor — can hit the ground running.

The piece in front of every other HTTP-related feature is the **callback
ABI**: a way for the host's request loop to invoke a guest-defined
function per request. Once that exists, multi-route dispatch, state
threading, and SSE all reduce to small variations on the same theme.

---

## User-visible shape (what we're trying to enable)

```
handleChat = (Request) -> HttpResponseBody {
    Request
        .body()
        .ChatCompletionRequest()?
        .respond()
}

main = (Network) -> Unit {
    HttpServer(Port(8421))
        .post(HttpStatus(200), RoutePath("/v1/chat/completions"), handleChat)
        .serve(Network)
}
```

The fourth argument to `post` is a **function reference** — not a body
string. The host invokes it per matching request, marshalling the
request body in and the response body out.

The full Coulisse-port shape adds state threading:

```
post = <S>(HttpRouter<S> * HttpStatus * RoutePath * (Request * S) -> HttpResponseBody) -> HttpRouter<S>
```

For the MVP we'll punt on `<S>` and ship the `(Request) -> HttpResponseBody`
form; state threading is a layer of polish on top of the same machinery.

---

## Why this is hard

Three independent moving parts have to line up:

1. **Component-model exports.** The guest currently exports exactly one
   function (`run`, lifted as `wasi:cli/run.run`). Per-route handlers
   need to be **additional exports** so the host can call them by name.
2. **Reentrancy.** The host's `serve()` is itself running as a guest
   `extern Wasm.async` call. From inside `serve()`, the host needs to
   call back into the guest. wasmtime supports this through
   `Accessor::with` + the component instance, but it's a careful dance.
3. **Lambda lifting (optional).** The user wants to write a lambda
   inline. The MVP can require a named function (no inline lambdas);
   inline lambda support comes from a separate lambda-lifting pass
   that turns `(Request) -> R { … }` into a top-level function before
   codegen sees it.

---

## Architecture: three slices

We split the work into three slices that each end in a green test:

### Slice 1 — single named handler, single route (the "hello, request" test)

Smallest deliverable that proves the round-trip works end-to-end.

**Surface:**

```
handleRequest = (String) -> String {
    "echoed: ".concat(String)
}

main = (Network) -> Unit {
    HttpServer(Port(8421))
        .withHandler(handleRequest)
        .serve(Network)
}
```

Only one handler. No routing — every request hits it. Just proves the
ABI works.

**Implementation:**

1. **Compiler — function-name-as-value support** (`src/checker/mod.rs`).
   `handleRequest` in an expression position currently parses as
   `Expr::Ident`. The checker rejects it because it's not a known
   value. Teach `check_expr`'s `Ident` branch to accept it when
   `symbols.methods` contains an entry under any receiver with that
   method name (i.e. it's the name of a defined function). The Oneway
   type carried by the ident is the function type
   `(String) -> String`.

2. **Compiler — handler registration** (new pass in `src/codegen/wasm/mod.rs`).
   A second pre-pass scans every call to `withHandler` (or `post` / `get` later)
   and records the handler-function's name. The function gets a stable
   numeric ID assigned in declaration order. Store these in a new
   `WasmGen.handler_funcs: Vec<String>` field.

3. **Compiler — new component export** (`src/codegen/wasm/component.rs`).
   Emit a `handle-request` component export alongside `run`. Lifting
   shape: `(string) -> string`. The core function is the existing
   user-compiled function for `handleRequest`; we just expose it
   under a stable name.

   The wasm-encoder calls needed:

   ```rust
   // In the `comp_insts.export_items` for wasi:cli/run, add:
   ("handle-request", ComponentExportKind::Func, handle_request_component_fn)
   ```

   where `handle_request_component_fn` is built via `lift` from the
   user-function's core index, mirroring how `run_component_fn` is
   built (`component.rs` ~line 470).

4. **Stdlib — `withHandler`** (`packages/oneway/std/src/http/http-server.ow`).
   New method:

   ```
   extern Wasm("oneway:builtins/http-server@0.1.0#with-handler")
   withHandler = (HttpServer * String) -> HttpServer
   ```

   For the MVP we pass the handler **name** as a String. The runtime
   side ignores the name and just calls the single `handle-request`
   export. (Generalising to multiple handlers comes in slice 2.)

5. **Runtime — call into the guest export** (`src/runtime.rs`,
   `mod host_builtin_http_server`). Modify `run_server` to receive
   the component instance via `Accessor::with`. On each accepted
   connection:

   ```rust
   acc.with(|mut access| {
       let handler = access
           .instance()
           .get_typed_func::<(String,), (String,)>(&mut access.store, "handle-request")?;
       let (body,) = handler.call_async(&mut access.store, (request_body,)).await?;
       // build HTTP/1.1 response with `body` as the response body
   })
   ```

   The exact accessor/instance plumbing varies by wasmtime version — see
   `wasmtime-45.0.0/src/runtime/component/concurrent.rs` for the
   `Accessor::with` shape we already use in `serve`.

**Test (`tests/runtime/http_handler_echo.ow`):**

```
handleRequest = (String) -> String {
    "echoed: ".concat(String)
}

main = (Network) -> Unit {
    HttpServer(Port(0))                                  -- 0 → pick a free port
        .withHandler(handleRequest)
        .serveOnce(Network)                              -- serves one request then exits
}
```

`serveOnce` is a test-only variant that returns after the first
request — saves us from killing the server in the harness. The
harness sends a single HTTP request to the bound port and asserts the
response body. Both the binding port and the request need a small
host-side fixture (which can live alongside `tests/runtime_fixtures.rs`).

### Slice 2 — multi-route dispatch (the "routing works" test)

Once one handler round-trips, the next step is to register multiple
handlers and dispatch by `(method, path)`.

**Surface:**

```
chatHandler = (Request) -> HttpResponseBody { … }
statusHandler = (Request) -> HttpResponseBody { … }

main = (Network) -> Unit {
    HttpServer(Port(0))
        .post(HttpStatus(200), RoutePath("/chat"), chatHandler)
        .get(HttpStatus(200), RoutePath("/status"), statusHandler)
        .serve(Network)
}
```

**Implementation deltas vs slice 1:**

1. **Compiler** — instead of one export, synthesise a single dispatcher
   export `__http_dispatch: (i32 handler_id, string body) -> string`
   whose body is a switch over the handler IDs assigned in the
   pre-pass. Each arm calls the matching user function.

   ```wat
   (func $__http_dispatch (param $id i32) (param $body_ptr i32) (param $body_len i32)
                          (result i32 i32)
     local.get $id
     i32.const 0
     i32.eq
     if (result i32 i32)
       local.get $body_ptr local.get $body_len
       call $chatHandler
     else
       local.get $id
       i32.const 1
       i32.eq
       if (result i32 i32)
         local.get $body_ptr local.get $body_len
         call $statusHandler
       else
         ;; unknown id: return ""
         i32.const 0 i32.const 0
       end
     end)
   ```

   This is straightforward wasm. The handler IDs are stable across the
   `WasmGen.handler_funcs` Vec.

2. **Stdlib** — `post` / `get` now take a handler value (function-typed)
   as their 4th arg. Their core extern signature stays `… String` —
   the compiler encodes the handler ID as a string at the call site,
   so no extern-signature change.

   The actual rewrite happens in `compile_constructor` (or wherever
   the `post(server, status, path, handler)` call is compiled): when
   the 4th arg is a function-name `Expr::Ident`, replace it with
   `StringLit(handler_id.to_string())` at codegen time.

3. **Runtime** — the route table now stores `(method, path) →
   handler_id`. On a request, the host calls
   `__http_dispatch(handler_id, body)` and returns the result.

**Test (`tests/runtime/http_handler_routing.ow`):** two routes, one
returns "ROOT", another returns "STATUS". Harness hits both endpoints
and asserts.

### Slice 3 — state threading + SSE

This is Gap 6, plus the state-parameter polish from Gap 1.

**State threading** (Gap 1's full shape): the user's handler signature
is `(Request * S) -> HttpResponseBody`, where `S` is an arbitrary
state value. The same dispatcher pattern as slice 2, but each arm also
receives the state pointer. The state is allocated once at
`HttpServer(...)` construction and stored alongside the route table.

**SSE** (Gap 6): the `SseSender` type is a handle pointing at a
host-owned TCP write half. The host registers an SSE handler via
`sse = (HttpRouter * RoutePath * (Request * S * SseSender) -> Unit)
  -> HttpRouter`, and the dispatcher arm for an SSE handler receives a
third parameter — the sender handle.

The handle is just an opaque integer at the wasm level (a slot index
into a host-owned `Vec<WriteHalf>`). `send = (SseData * SseSender) ->
Unit` is a separate `extern Wasm` host function that does
`writer.write_all(b"data: …\n\n").await`.

The dispatch path for SSE handlers differs from POST/GET handlers in
**not** returning a response body — the host has already started
streaming over the SSE socket, so the dispatcher arm returns `Unit`
and the host doesn't try to write an HTTP response after it.

A separate dispatcher export `__http_dispatch_sse` keeps the typing
honest (different return shape).

---

## Open questions for the next session

1. **Lambda lifting.** Slice 1 + 2 require named handlers. Inline
   lambdas (`post(…, (Request) -> HttpResponseBody { … })`) need a
   lifting pass that turns the lambda into a synthetic top-level
   function before codegen sees it. The lifting pass goes between the
   loader and the checker. Closure capture is the hard sub-problem —
   for the http handler case, *most* lambdas capture nothing
   (everything they need comes via parameters), so a no-capture-only
   restriction is acceptable for the MVP.

2. **Wasmtime reentrancy semantics.** Calling
   `instance.get_typed_func("handle-request").call_async(...)` from
   inside an `async fn serve(_accessor: &Accessor<…>, …)` body needs
   verification: does `Accessor::with` permit the nested call, or do
   we need to drop the accessor first and re-acquire? The docstring
   on `Accessor::with` says "panics if called recursively with any
   other accessor already in scope" — `with` itself, not
   `call_async`. The wasmtime async machinery should already support
   guest→host→guest call stacks (that's the whole point of the
   component model's reentrant calling convention), but it's worth
   building a one-call smoke test before committing to the design.

3. **Error path.** What does the host do when the guest handler
   panics / traps? Slice 1 can return 500 verbatim; slice 2 onwards
   should log the trap and continue serving other routes (don't tear
   down the whole server on one bad handler).

4. **Multiple `HttpServer` builders in one program.** The handler-ID
   assignment is per-compilation-unit, not per-server. If two
   `HttpServer(...)` chains exist in the same `main`, they share the
   ID space but the server-handle differentiates them (each
   `withHandler` call appends to that server's route table). Should
   just work; mention it explicitly in the test that exercises it.

5. **WIT signature for the dispatcher.** The synthesised export needs
   a WIT-level type. For slice 2 it's:

   ```wit
   __http-dispatch: func(handler-id: s32, body: string) -> string;
   ```

   This is exported, not imported, so it lives in the component's
   *export* section (currently empty except for `run`). Adding an
   export means another instance entry in the
   `comp_insts.export_items` call in `component.rs` plus the matching
   `comp_exports.export(...)` of that instance — same shape as the
   existing `wasi:cli/run` export, scaled to a single function
   instead of an instance.

---

## File-by-file change list (slice 1)

| File | Change |
|---|---|
| `src/codegen/wasm/mod.rs` | Add `handler_funcs: Vec<String>` field; new pre-pass `collect_handler_funcs`; reserve a stable export name |
| `src/codegen/wasm/component.rs` | Emit `handle-request` export alongside `run`; lift from the user function's core fn-idx |
| `src/runtime.rs` | `mod host_builtin_http_server`: thread `Accessor::with` into `run_server`; per-connection, look up `handle-request` export and call it |
| `packages/oneway/std/src/http/http-server.ow` | Add `withHandler = (HttpServer * String) -> HttpServer` extern |
| `tests/runtime/http_handler_echo.ow` + `.stdout` | End-to-end test: bind, send request, assert echoed body |
| `tests/common/mod.rs` | New helper: `serve_once_and_request(port: u16, body: &str) -> String` (test harness only) |

Estimate: **one focused session** for slice 1 alone, given that the
extern-import lowering machinery already proves all the canonical-ABI
pieces work; the new code is mostly *export* lowering, which is the
mirror image.

Slice 2 is **half a session** on top of slice 1 — the dispatcher
function is ~80 lines of wasm-encoder code plus the route-table
extension on the host side.

Slice 3 (state + SSE) is **one session** on top of slice 2 — most of
the work is the SSE writer task lifecycle on the host, not the
guest-side ABI.
