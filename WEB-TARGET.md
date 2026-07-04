# The Web Target

Canon programs can be browser frontends. A program that defines the
**Elm-architecture triple** compiles to a self-contained wasm core
module plus a tiny JS host that drives it — no bundler, no npm, no
framework dependency. Combined with the `wasi:http/service` world for
the backend, a fullstack app is two Canon programs sharing one types
package (see `examples/todo-fullstack`).

## Why Elm architecture, not React

Canon has no local variables, no mutation, and no closures that
capture. React's mental model — component-local state, hooks,
effects — is unexpressible. But the architecture React approximates
is one Canon states natively:

- `Model` — a product/union/newtype, the whole app state
- `update = (Model * String) -> Model` — a pure fold over messages
- `view = (Model) -> Html` — a pure render

Elm's `case msg of` is Canon's literal dispatch. The browser owns the
event loop; the guest is pure functions. This is not a framework
bolted onto the language — the language *is* the framework.

## Entry-point selection

The web world can't key on a return type alone the way HTTP does
(`(Request) -> Response`): every view helper returns `Html`, so
"returns `Html`" can't mean "is the entry". Detection keys on the
conventional **names + shapes** (see `find_web_entry` in
`src/ast.rs`):

```
init   = () -> Model                 # free, zero params
update = (Model * String) -> Model   # method on the model type
view   = (Model) -> Html             # method on the model type
```
`Model` is any user type (product, union, `Int`/`Float`/`String`
newtype). `Html` comes from `use canon/std/web/Html`. When the triple
is present (and no `main` / HTTP entry competes — the checker rejects
mixed worlds), codegen routes through `WasmGen::compile_web`.

## What gets emitted

`canon build` writes a three-file bundle into the build directory;
`canon run` serves the same bundle on `--addr` (default
`127.0.0.1:8080`):

```
<stem>.wasm      # the compiled app (a plain core module)
canon-web.js     # the JS host, embedded in the compiler binary
index.html       # boots the app into <div id="app">
```
Unlike the CLI/HTTP worlds the output is **not** a component —
browsers instantiate core wasm directly, and `canon-web.js` plays the
role the component wrapper plays elsewhere. (A `jco`-style
component-in-the-browser path can layer on later without changing the
source-level story.)

## The ABI (guest ↔ JS host)

```
exports:
  memory
  alloc(size: i32) -> i32                     bump-allocate (JS writes msg bytes here)
  init() -> i64                               opaque model
  update(model: i64, ptr: i32, len: i32) -> i64
  view(model: i64) -> (i32, i32)              UTF-8 HTML (ptr, len)

imports (module "wasi:cli/stdout@0.3.0-rc-2026-03-15"):
  the five print intrinsics — the JS host buffers bytes and forwards
  whole lines to console.log, so `.print()` debugging works in the
  browser console.
```
The model stays **in guest memory** between calls; the host only
holds an opaque i64 (see `WebModelShape` in `src/codegen/wasm/mod.rs`
for how each model representation normalizes to it). No
serialization crosses the boundary — messages go in as strings, HTML
comes out as a string, and that's the entire surface.

The bump allocator grows memory on demand (`build_alloc` emits a
`memory.grow` path) and never frees; every event leaks its
allocations. For apps this is bounded by the browser's memory limits
and irrelevant in practice for example-scale apps; a real allocator
is future work shared with the other worlds.

## Events

The host renders `view`'s HTML with `innerHTML` and event-delegates
three declarative attributes:

| Attribute | Trigger | Message sent |
|---|---|---|
| `data-msg="X"` | click | `X` |
| `data-msg-form="X:"` | form submit | `X:` + first input's value (then clears it) |
| `data-msg-input="X:"` | change | `X:` + the control's value |

Payload-carrying messages are plain string composition
(`"Toggle:".concat(id.toText())`) decoded in `update` with
`substring`/`byteAt` — the same pure-Canon parsing the stdlib's JSON
validator uses. `canon/std/web` provides `button` (renders
`data-msg`), `elAttr` (arbitrary attributes for the form/input
cases), and `text` (HTML-escapes user content).

## Full-page re-render

There is no virtual DOM. `view` returns the whole page and the host
swaps it in. This is honest, simple, and fast enough far beyond
example scale. Two consequences to know about:

- Focus does not survive a re-render, which is why typing flows
  through `data-msg-form` (read at submit) rather than per-keystroke
  model updates.
- Attribute values in `elAttr` are emitted verbatim — escape user
  content with `text()` before interpolating it anywhere.

## Current limits (deliberate MVP cuts)

- **No extern imports.** The browser host implements only the print
  stubs; `new_web` hard-errors on anything else. The natural next
  slice is a `canon:web/host` interface (fetch, localStorage,
  timers) implemented in `canon-web.js` — which is also what turns
  the fullstack example's frontend from static to API-backed.
- **`Msg` is `String`.** A typed `Msg` union with automatic
  encode/decode is a later slice; literal dispatch keeps the string
  form readable meanwhile.
- **Scalar-model method dispatch.** Method lookup on *values* of
  numeric newtypes erases to `Int` (the `Exit(3).exit()` gotcha), so
  helpers inside your program should declare `Int` receivers when the
  model is an `Int` newtype. The entry triple itself is unaffected —
  the wrappers call `update`/`view` by index, not by dispatch.

Pinned by `tests/web_target_test.rs` (full loop under wasmtime — the
same ABI the JS host drives) and `examples/counter-web`.
