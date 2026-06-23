# WASI HTTP Handler — Design

Status: **design in progress, implementation not started**. This
document defines the target architecture for HTTP-handling Canon
programs and supersedes the slicing plan in `DYNAMIC-HANDLERS.md`
(see [Why the previous plan is wrong](#why-the-previous-plan-is-wrong)).

The goal in one sentence: **Canon programs that handle HTTP
requests should compile to standard `wasi:http/handler`-exporting
components, runnable by any compliant host** (`wasmtime serve`,
Fermyon Spin, Cloudflare Workers, browser polyfills, our own
`canon run --addr`). No custom `canon:builtins/*` interfaces on
the wire.

The work fits inside a broader cleanup: replacing magic-named entry
functions (`main`, would-have-been `handle`) with **type-driven
entry-point selection**. See [Entry-point selection](#entry-point-selection).

---

## Target user-facing shape

```ow
use canon/std/http/Headers
use canon/std/http/Request
use canon/std/http/Response
use canon/std/http/Status

home = (Request) -> Response {
    Response(Headers(), Status(200))
}
```

(A real handler dispatches on `Request.method()` or `Request.path()`;
we omit that here for the smallest possible illustration.)

Key properties of this shape:

- **No `main`, no `handle`.** The compiler picks the entry by
  matching return types against a registry of known WASI worlds
  (see below). `home` is the user's chosen name; it's the entry
  because it's the (only) free function returning `Response`.

- **`Request` and `Response` are resource handles** — opaque to
  the guest, owned by the host's resource table. The stdlib wraps
  the resource methods so users see ordinary Canon methods.

- **No `HttpServer` builder, no `.serve()`, no port choice in the
  guest.** Those concerns belong to the host. The user configures
  the listener at the runner: `canon run --addr 127.0.0.1:8080`,
  `wasmtime serve my-app.wasm`, etc.

- **Helpers that return `Response` would conflict with `home`** —
  see [Entry-point selection](#entry-point-selection). To factor
  this handler, helpers must return non-world types (`String`,
  user-defined products, etc.) and `home` does the wrapping.

---

## Entry-point selection

This is bigger than HTTP. Canon's existing `main` is the one
piece of the language that violates "types-are-identity": it's
a magic name borrowed from C tradition. Adding `handle` for HTTP
would compound the wart. The replacement, in one rule:

> **A module is a runnable program when exactly one of its
> top-level free functions has a return type that names a known
> WASI world's primary export shape. That function is the entry.**

The registry of world shapes:

| Return type | World | WASI export |
|---|---|---|
| `Unit`, `ExitCode`, `Result<Unit, _>`, `Result<ExitCode, _>` | `wasi:cli/command` | `wasi:cli/run.run` |
| `Response`, `Result<Response, _>` | `wasi:http/service` | `wasi:http/handler.handle` |

Rules:

1. **Exactly one** top-level function may return a world-shape
   type. Multiple matches in the same module is a compile error
   with the message "ambiguous entry: `foo` and `bar` both return
   `Response`. Refactor one to return a non-world type, or merge."
2. **Mixed worlds** — one function returning `Unit`, another
   returning `Response` — is a compile error. A component exports
   exactly one world.
3. **Zero matches** means the module is a library, not a program.
   It can be `use`d from another module; it cannot be run with
   `canon run`.
4. The entry function's parameters declare the program's
   capability requirements. For a CLI program, valid parameter
   types are host-provided capabilities (`Stdout`, `Filesystem`,
   `Args`, …). For an HTTP service, the parameter is the incoming
   `Request`. The compiler validates that the parameter list is
   consistent with the selected world.

Helpers can return any non-world type — `String`, user-defined
products, etc. The discipline "helpers return data, the entry
returns a `Response`" is the explicit factoring this rule pushes
toward.

**Migration of existing programs.** Today every example program
has `main = (...) -> Unit { ... }`. Under the new rule, the
function need not be called `main` — but `main` is still a valid
name (it just isn't *required*). Existing programs keep compiling.
New programs may use whatever name reads best. The codegen change
is purely additive: scan all top-level functions, pick the one
with a world-shape return, lift it as the world's export. The old
"`main` is the entry, period" rule becomes "`main` is one valid
name for the entry, like any other."

A separate cleanup pass — outside the scope of this doc — can
rename `main` to something more meaningful in each example
(`hello`, `serve`, `program`, …). That pass is purely cosmetic and
can land after the codegen change.

---

## The WIT we target

WASI HTTP 0.3 (the release candidate vendored by
`wasmtime-wasi-http @ 45`). Exact path:
`wasi:http/handler@0.3.0-rc-2026-03-15`, matching the rc we
already vendor for `wasi:cli`, `wasi:clocks`, etc.

```wit
package wasi:http@0.3.0-rc-2026-03-15;

interface types {
  resource request {
    method: func() -> method;
    path-with-query: func() -> option<string>;
    scheme: func() -> option<scheme>;
    headers: func() -> headers;
    consume-body: func() -> result<stream<u8>, error-code>;
  }

  resource response {
    constructor(status-code: status-code, headers: headers);
    body: func() -> writable-body;
  }

  resource headers {
    constructor();
    append: func(name: field-name, value: field-value);
  }

  variant method { get, post, put, delete, head, options, patch, other(string) }
  type status-code = u16;
  variant error-code { ... }
}

interface handler {
  handle: async func(request: request) -> result<response, error-code>;
}

world service {
  import types;
  export handler;
}
```

The guest **imports** `wasi:http/types` (so it can construct
`Response`, `Headers`, write the body) and **exports**
`wasi:http/handler` (so the host can dispatch to it).

The Canon-side names map mechanically: `request` → `Request`,
`response` → `Response`, `headers` → `Headers`, `method` →
`Method`, `status-code` → `Status` (the `-code` suffix drops
because the WIT type is already inside a status-coded interface).

---

## Why the previous plan is wrong

`DYNAMIC-HANDLERS.md` proposes a custom interface
`canon:http-handler/handler@0.1.0` carrying
`handle-request: func(body: string) -> string`. This was a
reasonable bootstrap when the goal was "make handlers work at all
on the existing `canon:builtins/http-server` host bridge."

It's wrong as a long-term shape:

1. **It's not portable.** Only Canon's own runtime knows what
   `canon:http-handler/handler@0.1.0` is. `wasmtime serve` and
   every other host model the world as `wasi:http/handler`.
2. **It's `string → string`.** Real handlers need method, path,
   headers, status codes, streaming bodies — none of which fit in
   a flat string round-trip without re-encoding HTTP inside HTTP.
3. **It assumes a program-driven server.** The whole
   `HttpServer(...).serve()` model is incompatible with "handler
   is a guest export the host calls per request." The latter is
   the standard, so the former goes.

Anything we build on the `canon:http-handler/handler@0.1.0`
foundation has to be thrown away once we adopt
`wasi:http/handler`. We should skip the throwaway step.

---

## What the codegen has to learn

Today's codegen emits a `wasi:cli/command` world: one core
module + one canonical-ABI lift of `run`. The new world is
structurally different. Prerequisites, in roughly ascending
difficulty:

### Prereq A — Type-driven entry detection

The codegen needs to scan top-level functions and pick the one
matching a world-shape return type. When the match is `Response`:

- Skip the `main`-name check (which today is also the `wasi:cli/run`
  entry selector).
- Emit a `wasi:http/service` world instead of `wasi:cli/command`:
  no `wasi:cli/run` export, instead a `wasi:http/handler` export.
- Skip the `wasi:cli/stdout` machinery the command-world
  auto-imports.

The detection lives in
`src/codegen/wasm/mod.rs::assign_func_indices`. The world choice
lives in `src/codegen/wasm/component.rs`. The existing optional
`handleRequest` scaffolding is the wrong shape but proves the
"conditional world export" pattern works.

### Prereq B — Resource handles in the canonical ABI

`request`, `response`, `headers` are all WIT `resource`s. The
codegen must:

- Import their types from `wasi:http/types` (declare them in the
  component type section).
- Lower `Request` (received as a parameter) as an `own<request>`
  handle — a single i32 in the core ABI.
- Lift `Response` (returned to the host) as an `own<response>`
  handle — also a single i32.
- Lower **method calls on resources** as canonical-ABI
  `canon.resource.…` instructions, not as flat function calls.

The CLAUDE.md gap "WIT `resource` / `own<T>` / `borrow<T>` in
`extern Wasm` signatures" tracks this work for *imports* (the guest
calling host-provided resource methods). The *export* side reuses
the same lowering. Bindgen-side scaffolding (`Foo = Handle`
newtypes in `packages/canon/wasi/*`) already exists for the type
declarations.

### Prereq C — Sub-u64 integer widths

`status-code` is `u16`. The CLAUDE.md gap "Sub-u64 WIT integer
widths in `extern Wasm`" tracks this for the import side. The
export side reuses the same lowering. Until this lands, we'd
have to lie about the status code type — unacceptable.

### Prereq D — Async handler export

`handle` is `async func`. The codegen already emits async-stackful
lifts for `wasi:cli/run.run` (it's how `serve_component` works at
all), so the mechanics are present. The new piece is async lift
of a function that *takes* a resource parameter and *returns* a
`result<resource, error-code>`. The error-code variant is itself
resource-heavy (some of its arms carry strings).

### Prereq E — Structured-record / variant returns

`method`, `error-code` are WIT `variant`s. The codegen already
lowers Canon unions; the new piece is making them ABI-compatible
with the WASI `method`/`error-code` shapes specifically. Mostly
mechanical once resources + integers work.

---

## Implementation slicing

Each slice ends with a green `tests/runtime/` test against the
real `wasi:http/handler` export. No transitional WIT shapes.

### Slice 0 — Canon-side API skeleton ✦ this session

**Deliverable:** the curated stdlib types exist as Canon source.
The checker accepts a program that imports and uses them, even
though codegen can't yet build the resulting component.

- `packages/canon/std/src/http/method.can` — `Method` union with
  variants matching the WIT.
- `packages/canon/std/src/http/status.can` — `Status = Int` newtype.
- `packages/canon/std/src/http/headers.can` — `Headers = Handle`,
  empty constructor + `set` for appending fields.
- `packages/canon/std/src/http/request.can` — `Request = Handle`
  with method declarations for `.method()`, `.path()`, `.body()`.
- `packages/canon/std/src/http/response.can` — `Response = Handle`
  with the WIT constructor.
- `tests/checker/ok/wasi_http_handler.can` — checker-only fixture:
  a small handler using the new types.

Both checker fixtures and stdlib files use `extern Wasm` headers
that name the real WASI HTTP paths. The codegen can't build them
yet — runtime tests come in slices 1+ — but the checker accepts
the types and signatures.

### Slice 1a — entry detection (landed)

**Deliverable:** the parser and checker recognise the HTTP entry shape.

Landed:
- `ast::entry_world_of` classifies return types into the world
  registry (Cli / Http / None).
- Parser exemption: a free function returning `Response` or
  `Result<Response, _>` keeps `Request` as a regular parameter
  instead of having it extracted as a receiver.
- Checker: detects HTTP entries, applies the rules:
  - `main` + no HTTP entry → existing CLI path.
  - HTTP entry + no `main` → well-formed type surface, but the
    codegen is slice 1b. The checker emits a clear "codegen not
    yet implemented" diagnostic with a pointer to this doc.
  - `main` + HTTP entry → "mixed worlds" error.
  - Multiple HTTP entries → "ambiguous HTTP entry" error.
  - Neither → "no entry point defined".
- HTTP entry is exempt from the alphabetical-ordering check on
  free functions (same exemption as `main`).
- Fixtures: three new fail-fixtures under `tests/checker/fail/`
  pin each diagnostic; `tests/checker/ok/wasi_http_types_load.can`
  still covers the stdlib-type smoke test.

Left for slice 1b:
- Actually emit a `wasi:http/service` world.
- Resource lowering for `request` / `response`.
- The runtime test (`canon run` an HTTP program end-to-end).

### Slice 1b — minimal `wasi:http/service` world emission

**Deliverable:** the codegen detects a `(Request) -> Response`
free function and emits a `wasi:http/service` world with that
function lifted as `wasi:http/handler.handle`. The handler body
can be trivial — e.g. always return a 200 with empty body — but
the export contract has to match WASI HTTP exactly. `wasmtime
serve` should be able to instantiate the component without
complaint.

Work:
- Read the entry from the AST in `src/codegen/wasm/mod.rs::assign_func_indices`
  using `ast::entry_world_of` (the same helper the checker uses).
  When the entry is an HTTP one, branch to the new world path.
- Switch the world emission in `src/codegen/wasm/component.rs`
  based on the detected entry's return type.
- Emit the `wasi:http/types` import section and the
  `wasi:http/handler` export section.
- Minimal resource lowering for `request` / `response` — enough
  to receive an opaque i32 handle and return an opaque i32 handle.
  No method calls on resources yet.
- Remove the "codegen not yet implemented" diagnostic from
  `check_with_entry` once codegen handles HTTP entries cleanly.
- Move `tests/checker/fail/http_handler_codegen_pending.can` to
  `tests/checker/ok/wasi_http_handler.can` (deleting its `.stderr`).

**Test:** `tests/runtime/wasi_http_handler_smoke.can` — defines a
handler that ignores its `Request` parameter and returns a
hard-coded 200 response. Harness sends one HTTP request, asserts
status 200.

This is the slice that delivers user-visible value first: any
Canon program with a `Response`-returning function can now run
on any WASI HTTP host.

### Slice 2 — request introspection (read path)

**Deliverable:** the handler can actually read the incoming
request. `Request.method()`, `Request.path()`, `Request.body()`
all work.

Work:
- Resource-method codegen for `request`'s methods (`method`,
  `path-with-query`, `consume-body`).
- Variant lowering for the `method` return type.
- Stream → string adapter for the body (synchronous read of the
  whole body into a `String`, like `consume-body` + `stream.read`
  in a loop).

**Test:** `tests/runtime/wasi_http_handler_echo.can` — the handler
reads the body and echoes it. Harness asserts request and response
bodies match.

### Slice 3 — response composition (write path)

**Deliverable:** the handler can build non-trivial responses —
custom status, headers, body.

Work:
- Constructor lowering for `headers` + `response`.
- Resource-method codegen for `response`'s `body()` +
  `writable-body`'s `write` / `close`.
- Stdlib helpers: a convenience constructor on `Response` that
  takes `(Status, String)` and handles the writable-body dance
  internally.

**Test:** `tests/runtime/wasi_http_handler_routing.can` — a minimal
router. Two routes return different status codes and bodies.

### Slice 4 — local runner & cleanup

**Deliverable:**
- `canon run --addr 127.0.0.1:8080` works end-to-end against a
  user's `(Request) -> Response` program.
- The old `canon:builtins/http-server` host bridge is deleted
  from `src/runtime.rs`.
- `packages/canon/std/src/http/http-server.can`,
  `http-response-body.can`, `port.can`, `route-path.can`,
  `http-status.can`, `body.can`, `request.can` (old) are deleted —
  no replacement (the new model has no equivalent; the host owns
  the server).
- `examples/http-server/` is rewritten in the new shape.

The existing `--addr` plumbing in
`runtime.rs::serve_component_async` already does the right thing —
it pre-instantiates the component and dispatches via
`wasmtime-wasi-http`'s `ServicePre`. The work in this slice is
mostly cleanup; the new world export from slices 1–3 is what makes
it actually do something.

### Slice 5 — async bodies & streaming (future)

**Superseded by [`STREAMING.md`](./STREAMING.md).** Reading large
request bodies as streams, writing streamed response bodies (SSE,
NDJSON), and the `Stream<T>` stdlib surface are all designed in that
doc — including the `(Headers * Status * Stream<String>) -> Response`
constructor and the four-line SSE formatter that replaces the current
`Content-Type:`-prefix hack. The slices here become the prerequisites
(handler export + request/response resources) for the work in
`STREAMING.md`.

---

## Open questions for implementation

1. **`Response`-returning helpers in the same module.** The rule
   "exactly one world-shape return type" pushes users to refactor
   helpers to return non-world types. A 404 helper that returns
   `Response` is the canonical case where this bites. Tradeoffs:
   - **Strict** (what's proposed): conflict is a compile error;
     factor the helper to return `String` or a custom union.
   - **Reachability-based**: the entry is the function that isn't
     called from any other top-level function. Subtle (a typo
     turns the wrong function into the entry); not adopted.
   - **Alphabetical-first**: matches the rest of Canon, but
     silent selection by name is fragile; not adopted.

   We're going with strict. Migrate-time pain is real; future
   pain from silent entry selection is worse.

2. **`main` + `Response`-returning function in the same program.**
   Currently `main` is also matched by the `Unit` row of the world
   registry — so this is just a special case of "mixed worlds in
   one module," handled by rule 2.

3. **Outbound HTTP (`wasi:http/client`).** Today the
   `canon:builtins/http` bridge does this. Migrating it to
   `wasi:http/outgoing-handler` is parallel work — same resource +
   sub-u64 prereqs, but on the import side. Not in scope for this
   doc; see CLAUDE.md gap rows for the codegen tracking.

4. **Resource ownership across async boundaries.** The handler is
   `async func(request: own<request>) -> result<own<response>,
   error-code>`. The guest owns the request handle (drops it when
   the body's read, or at the end of the call) and owns the
   response handle until it's returned. Wasmtime's resource table
   handles the refcounting; the codegen just has to emit the right
   `resource.drop` instructions when handles fall out of scope.
   This is a correctness concern, not just bookkeeping — leaking a
   `request` handle per call is a slow memory leak.

5. **Error-code shape.** `wasi:http/types`'s `error-code` is a
   large variant (DNS errors, TLS errors, connection errors, …).
   The stdlib should probably collapse this into a single
   `HttpError = String` newtype carrying a debug-rendered message
   on the Canon side. The full variant is overkill for guests
   that just want "something went wrong, here's the message."

---

## File-by-file change list (slice 0 — this session)

| File | Change |
|---|---|
| `packages/canon/std/src/http/method.can` | **New.** `Method` union with WIT-aligned variants. |
| `packages/canon/std/src/http/status.can` | **New.** `Status = Int` newtype (replaces the redundant `HttpStatus`; old file stays until slice 4). |
| `packages/canon/std/src/http/headers.can` | **New.** `Headers = Handle`, empty constructor, `set`. |
| `packages/canon/std/src/http/response.can` | **New.** `Response = Handle`, WIT constructor binding. |
| `packages/canon/std/src/http/request.can` | **Replace stub.** `Request = Handle` + method/path/body bindings. |
| `tests/checker/ok/wasi_http_types_load.can` | **New.** Loadability smoke test — imports each new type and uses `Status` in a trivial `main` so today's checker accepts it. A proper handler-shaped fixture lands with slice 1, when the entry-point rule is implemented. |
| `DESIGN.md` (§Entry Point) | **Update.** Replace the `main`-is-magic description with the type-driven rule. |
| `CLAUDE.md` (§Known codegen gaps) | **Update.** Replace the "Dynamic HTTP handlers" row with a pointer to this doc. |
| `WASM.md` (§Capability map) | **Update.** Point the `HttpServer` row at this doc. |
| `DYNAMIC-HANDLERS.md` | **Header.** Deprecation notice pointing here. |

Codegen slices 1–4 are deliberately *not* in this PR. Slice 0
isolates the design surface so it can be reviewed without code
churn, and slices 1+ each have their own focused PR.
