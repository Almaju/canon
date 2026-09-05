# The Web Target

Canon programs can be browser frontends. A program that defines the
**Elm-architecture triple** compiles to a self-contained wasm core module
plus a tiny generated JS host -- no bundler, no npm, no framework. Combined
with the `wasi:http/service` world for the backend, a fullstack app is two
Canon programs sharing one `src/` tree, compiled and served by one
`canon run` (see [Fullstack packages](#fullstack-packages) below and
`examples/todo-fullstack`).

Canon has no local variables, no mutation, and no capturing closures, so
React's component-local state is unexpressible. The architecture React
approximates, Canon states natively:

```text
Init = Model                     # marker: the initial model

Model => Html { ... }              # view -- a pure render
Unit => Init { ... }               # init -- the whole app state, initially
Model * Msg => Model { ... }       # update -- the model's command, a fold over messages
```

All three are anonymous, type-selected constructors -- no names. `Model`
is any user type, `Msg` the message type (`Msg = String` from
`canon/web`, what the host delivers). `Init` is a **model-alias marker
newtype** (`Init = Model`): `init` produces the model from nothing and
would otherwise be the model's nullary constructor. `update` is the
model's one command ([Functions § Declaration](../spec/functions.md)),
applied to the current model with each message. `Html` resolves to
`canon/web/Html` automatically.

Detection is **by shape**: the `view` is the sole `Model => Html` whose
receiver is a user type (a primitive receiver marks a stdlib
conversion like `Escaped` instead); from its model, `init` is the
unique nullary constructor whose result aliases the model and `update`
the model's unique command. When the triple is present -- and no CLI or
HTTP entry competes, which the checker rejects as mixed worlds -- the
program is a web app.

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
| `data-fetch="URL"` + `data-fetch-msg="X:"` | click | `X:` + the fetched response body -- the host-mediated effect that lets a pure app talk to a backend |

Payload-carrying messages are plain string composition
(`"Toggle:" -> Joined(Id -> String)`) decoded by the reducer with
`Substring`/`ByteAt` -- the same pure-Canon parsing the JSON validator uses.
The prelude provides `Escaped` (HTML-escapes user content); the
[`canon/ui` package](./packages.md#canonui--html-elements) provides the
element vocabulary — `Button` (renders `data-msg`), `Form`, `Input`,
`ElAttr` (arbitrary attributes), and the rest.

There is no virtual DOM: `view` returns the whole page and the host swaps it
in. Focus does not survive a re-render, which is why typing flows through
`data-msg-form` (read at submit) rather than per-keystroke updates.

The swap waits for a pointer interaction to finish. A `data-msg-input`
control fires `change` on blur -- that is, on the *next* control's
mousedown -- so an unguarded re-render would replace the element the
mousedown was headed for and the browser would never fire its `click`.
The message still folds the moment it is sent; only the swap defers, so
editing a field and clicking a button in one gesture applies both, in
order.

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
`canon build` apps persist by default. `examples/todo-fullstack` is the worked
example.

## Fullstack packages

A package whose `src/` declares the web triple in one file and a
`Request => Response` handler in another is a **fullstack package**
([Modules & Packages](../spec/modules.md#no-manifest)). Like the
entries themselves, the entry *files* are anonymous -- discovered by
shape, no reserved filename -- and the shared sibling files are the
contract between the two sides. Each entry still compiles to its own
artifact -- a component exports exactly one world -- but `canon run`
serves both from one process on one address:

```sh
canon run examples/todo-fullstack     # http://127.0.0.1:8080
```

The bundle owns `/`, `/index.html`, `/canon-web.js`, and the app's
`.wasm`; every other request dispatches to the server component.
Frontend and backend share an origin, so a `data-fetch` URL is
relative (`data-fetch="/todos"`) and CORS never comes up. `canon
build` writes the bundle (named after the package) and the server's
`.wasm`/`.wit` (named after its entry file) into one `build/`.

## Current limits

- **No extern imports.** The browser host implements only the print stubs.
  Persistence needs no import (it is host-side message replay). A
  `canon:web/host` interface (fetch, timers) is the natural next step.
- **`Msg` is `String`.** A typed `Msg` union with automatic encode/decode is
  future work; literal dispatch keeps the string form readable meanwhile.
