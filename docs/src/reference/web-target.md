# The Web Target

Canon programs can be browser frontends. A program that defines the
**Elm-architecture triple** compiles to a self-contained wasm core module
plus a tiny generated JS host — no bundler, no npm, no framework. Combined
with the `wasi:http/service` world for the backend, a fullstack app is two
Canon programs sharing one types package (see `examples/todo-fullstack`).

Canon has no local variables, no mutation, and no capturing closures, so
React's component-local state is unexpressible. The architecture React
approximates, Canon states natively:

```canon
init   = () => Model                 # the whole app state
update = (Model * String) => Model   # a pure fold over messages
view   = (Model) => Html             # a pure render
```

`Model` is any user type. `Html` resolves to `canon/std/web/Html`
automatically. When the triple is present — and no `main` or HTTP entry
competes, which the checker rejects as mixed worlds — the program is a web
app. Detection is by these **names and shapes**, not return type: every
view helper returns `Html`, so "returns `Html`" can't mark the entry.

## What gets emitted

`canon build` writes a three-file bundle; `canon run` serves it on `--addr`
(default `127.0.0.1:8080`):

```
<stem>.wasm      # the compiled app — a plain core module, not a component
canon-web.js     # the JS host, embedded in the compiler binary
index.html       # boots the app into <div id="app">
```

Browsers instantiate core wasm directly, so the web output is **not** a
component; `canon-web.js` plays the role the component wrapper plays for the
CLI and HTTP worlds. The model stays in guest memory between calls — the
host only holds an opaque `i64`. Messages go in as strings, HTML comes out
as a string; no serialization crosses the boundary. `.print()` maps to
`console.log`.

## Events

The host renders `view`'s HTML with `innerHTML` and event-delegates three
declarative attributes:

| Attribute | Trigger | Message sent |
|---|---|---|
| `data-msg="X"` | click | `X` |
| `data-msg-form="X:"` | form submit | `X:` + first input's value (then clears it) |
| `data-msg-input="X:"` | change | `X:` + the control's value |

Payload-carrying messages are plain string composition
(`"Toggle:".concat(id.String())`) decoded in `update` with
`substring`/`byteAt` — the same pure-Canon parsing the JSON validator uses.
`canon/std/web` provides `button` (renders `data-msg`), `elAttr` (arbitrary
attributes), and `text` (HTML-escapes user content).

There is no virtual DOM: `view` returns the whole page and the host swaps it
in. Focus does not survive a re-render, which is why typing flows through
`data-msg-form` (read at submit) rather than per-keystroke updates.

## Persistence

The host can persist app state to `localStorage` with **no guest-side
capability**. Because `Model` is a pure fold over messages, the host never
serializes the model: it records the **message log** and, on the next load,
replays it through `update` to rebuild the identical model.

`canonWebStart(wasmUrl, root, persistKey)` enables this when `persistKey` is
a non-empty string. It reads the saved log on boot (stdout muted during
replay), appends every subsequent message, and discards the log if a saved
message ever fails to fold — a stale or corrupt log can't brick the app. The
generated `index.html` keys persistence by the app's stem, so `canon run` /
`canon build` apps persist by default. `examples/todolist-web` is the worked
example.

## Current limits

- **No extern imports.** The browser host implements only the print stubs.
  Persistence needs no import (it is host-side message replay). A
  `canon:web/host` interface (fetch, timers) is the natural next step.
- **`Msg` is `String`.** A typed `Msg` union with automatic encode/decode is
  future work; literal dispatch keeps the string form readable meanwhile.
