# The Web Target

Canon programs can be browser frontends. A program that defines the
**Elm-architecture triple** compiles to a self-contained wasm core module
plus a tiny generated JS host -- no bundler, no npm, no framework. Combined
with the `wasi:http/service` world for the backend, a fullstack app is two
Canon programs sharing one types package (see `examples/todo-fullstack`).

Canon has no local variables, no mutation, and no capturing closures, so
React's component-local state is unexpressible. The architecture React
approximates, Canon states natively:

```canon
Init = Model                     # marker: the initial model
Update = Model                   # marker: the model after one message

Model => Html { ... }              # view -- a pure render
Unit => Init { ... }               # init -- the whole app state, initially
Model * Msg => Update { ... }      # update -- a pure fold over messages
```

All three are anonymous, type-selected constructors -- no names. `Model`
is any user type, `Msg` the message type (`String` today). `Init` and
`Update` are **model-alias marker newtypes** (`Init = Model`,
`Update = Model`); they exist because `init` and `update` both produce
the model and would otherwise collide on one constructor key -- the
markers give each a distinct type. `Html` resolves to
`canon/std/web/Html` automatically.

Detection is **by shape**: the `view` is the sole `Model => Html` whose
receiver is a user type (a primitive receiver marks a stdlib
conversion like `Escaped` instead); from its model, `init` is the
unique nullary
constructor whose result aliases the model and `update` the unique
two-input constructor whose first input is the model. When the triple is
present -- and no CLI or HTTP entry competes, which the checker rejects as
mixed worlds -- the program is a web app.

## What gets emitted

`canon build` writes a three-file bundle; `canon run` serves it on `--addr`
(default `127.0.0.1:8080`):

```
<stem>.wasm      # the compiled app -- a plain core module, not a component
canon-web.js     # the JS host, embedded in the compiler binary
index.html       # boots the app into <div id="app">
```

Browsers instantiate core wasm directly, so the web output is **not** a
component; `canon-web.js` plays the role the component wrapper plays for the
CLI and HTTP worlds. The model stays in guest memory between calls -- the
host only holds an opaque `i64`. Messages go in as strings, HTML comes out
as a string; no serialization crosses the boundary. `-> Print` maps to
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
(`"Toggle:" -> Joined(Id -> String)`) decoded by the reducer with
`Substring`/`ByteAt` -- the same pure-Canon parsing the JSON validator uses.
`canon/std/web` provides `Button` (renders `data-msg`), `ElAttr` (arbitrary
attributes), and `Escaped` (HTML-escapes user content).

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
message ever fails to fold -- a stale or corrupt log can't brick the app. The
generated `index.html` keys persistence by the app's stem, so `canon run` /
`canon build` apps persist by default. `examples/todolist-web` is the worked
example.

## Current limits

- **No extern imports.** The browser host implements only the print stubs.
  Persistence needs no import (it is host-side message replay). A
  `canon:web/host` interface (fetch, timers) is the natural next step.
- **`Msg` is `String`.** A typed `Msg` union with automatic encode/decode is
  future work; literal dispatch keeps the string form readable meanwhile.
