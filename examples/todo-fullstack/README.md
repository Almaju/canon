# todo-fullstack

A browser frontend and an HTTP backend, sharing one set of types and
rendering code, served by one command. This is the flagship fullstack
example: no bundler, no npm, no serialization framework; the shared
`.can` files *are* the contract.

```sh
canon run examples/todo-fullstack
```

Open <http://127.0.0.1:8080>: add todos through the form, toggle and
remove them, then press **“Load todos from the server”** — the button
fetches `/todos` from the Canon backend *on the same origin* and the
frontend decodes it with the same shared code that produced it.

The directory is a **fullstack package**: `src/web.can` +
`src/server.can` in place of `src/main.can`. `canon run` compiles both
and serves them from one process — the web bundle owns `/`,
`/index.html`, `/canon-web.js`, and the app's `.wasm`; every other
request dispatches to the server component. `canon build` writes both
artifacts into `build/`.

## Layout

| File | Role |
|---|---|
| `src/todos.can` | **Shared.** The `Todos` wire/state encoding (newline-separated `flag\|title` lines), its operations as result newtypes (`AddedTodo`, `ToggledAt`, `RemovedAt`), the list renderer, and the pure-Canon string helpers they need. Compiled into *both* wasm binaries. |
| `src/line.can` | **Shared.** One todo line: the `Flipped` toggle and the `<li>` renderer with its `Toggle:`/`Delete:` message buttons. |
| `src/title.can` | **Shared.** The `Title` newtype. |
| `src/seeded.can` | **Shared.** The seed list the server serves. |
| `src/web.can` | **Frontend entry.** The Elm triple: `init`/`update`/`view` over `Todos` as the model. Messages are prefix-parsed strings (`Add:`, `Toggle:N`, `Delete:N`, `Load:payload`). |
| `src/server.can` | **Backend entry.** `Request => Response`: method dispatch (GET-only), path routing, and `GET /todos` serving the seed list in the shared encoding. |

## What it demonstrates

- **Shared types across the stack**: `Todos` and its operations are
  written once. The backend serves the encoding; the frontend's
  `Load:` message swaps it straight into the model. There is no JSON
  schema, no client codegen — the type file is the protocol.
- **One language, two worlds, one command**: `src/server.can` compiles
  to a standard `wasi:http/service` component; `src/web.can` compiles
  to the browser bundle. The entry-point *shape* selects the world —
  same compiler, same stdlib, no flags — and `canon run` serves both
  on one origin, so there is no CORS to configure.
- **Host-mediated effects**: the frontend is pure; the fetch happens
  in the JS host via the declarative `data-fetch` attribute (a
  relative URL, resolved against the shared origin), and the response
  arrives as an ordinary message through `update`.
- **Pure-Canon parsing**: message payloads (`Toggle:3`) and the wire
  encoding are decoded with recursive `Substring`/`ByteAt` functions —
  the same style as the stdlib JSON validator, no host help.
