# counter-web

The smallest Canon web app — the Elm-architecture counter, compiled
to WebAssembly and driven by the bundled JS host. See
[`WEB-TARGET.md`](../../WEB-TARGET.md) for the whole story.

```sh
canon run examples/counter-web          # serves on 127.0.0.1:8080
canon run examples/counter-web --addr 127.0.0.1:9000
```

Open the printed URL: two buttons and a number. `canon build
examples/counter-web` writes the deployable three-file bundle
(`counter-web.wasm`, `canon-web.js`, `index.html`) instead.

## What it demonstrates

- **The entry-point rule**: defining `init` / `update` / `view` with
  the conventional shapes *is* what makes the program a web app — no
  registration, no framework object, no build config.
- **Messages as literal dispatch**: `update` branches on the message
  string with a literal dispatch; the catch-all arm returns the model
  unchanged.
- **Views as pure composition**: `view` builds the page from
  `canon/std/web` helpers (`h1`, `button`, `span`, `div`) — plain
  string composition under the hood, `data-msg` attributes carry the
  events.
- **No JS written by hand**: `canon-web.js` is generated; the only
  authored file is `src/main.can`.
