# todo-fullstack

Two Canon programs — a browser frontend and an HTTP backend — sharing
one set of types and rendering code, compiled from the same directory.
This is the flagship fullstack example: no bundler, no npm, no
serialization framework; the shared `.can` files *are* the contract.

```sh
# terminal 1 — the backend (wasi:http/service component)
canon run examples/todo-fullstack/server.can --addr 127.0.0.1:8090

# terminal 2 — the frontend (web-app bundle, served for the browser)
canon run examples/todo-fullstack/web.can --addr 127.0.0.1:8080
```

Open <http://127.0.0.1:8080>: add todos through the form, toggle and
remove them, then press **“Load todos from the server”** — the button
fetches `http://127.0.0.1:8090/todos` from the Canon backend and the
frontend decodes it with the *same shared code* that produced it.

## Layout

| File | Role |
|---|---|
| `todos.can` | **Shared.** The `Todos` wire/state encoding (newline-separated `flag\|title` lines), its operations (`addTodo`, `toggleAt`, `removeAt`), the list renderer, and the pure-Canon string helpers they need. Compiled into *both* wasm binaries. |
| `line.can` | **Shared.** One todo line: the `flip` toggle and the `<li>` renderer with its `Toggle:`/`Delete:` message buttons. |
| `title.can` | **Shared.** The `Title` newtype. |
| `web.can` | **Frontend entry.** The Elm triple: `init`/`update`/`view` over `Todos` as the model. Messages are prefix-parsed strings (`Add:`, `Toggle:N`, `Delete:N`, `Load:payload`). |
| `server.can` | **Backend entry.** `(Request) -> Response`: method dispatch (GET-only), path routing, CORS + content-type headers, and `GET /todos` serving the seed list in the shared encoding. |

There is no `canon.toml` here on purpose: the two entries live in one
directory so they can `use` the same sibling files. (Package-level
`[deps]` sharing is future work; same-directory sharing is the honest
mechanism available today.)

## What it demonstrates

- **Shared types across the stack**: `Todos` and its operations are
  written once. The backend serves the encoding; the frontend's
  `Load:` message swaps it straight into the model. There is no JSON
  schema, no client codegen — the type file is the protocol.
- **One language, two worlds**: `server.can` compiles to a standard
  `wasi:http/service` component; `web.can` compiles to the browser
  bundle. The entry-point *shape* selects the world — same compiler,
  same stdlib, no flags.
- **Host-mediated effects**: the frontend is pure; the fetch happens
  in the JS host via the declarative `data-fetch` attribute, and the
  response arrives as an ordinary message through `update`.
- **CORS as plain data**: `Headers().set("access-control-allow-origin", "*")`
  — response headers are just values.
- **Pure-Canon parsing**: message payloads (`Toggle:3`) and the wire
  encoding are decoded with recursive `substring`/`byteAt` functions —
  the same style as the stdlib JSON validator, no host help.
