# Fullstack: One Language, Both Sides

[`examples/todo-fullstack`](https://github.com/Almaju/canon/tree/main/examples/todo-fullstack):
a browser frontend and an HTTP backend, **sharing one set of types and
rendering code**, compiled from the same directory. No bundler, no npm,
no serialization framework — the shared `.can` files *are* the contract.

```sh
# terminal 1 — the backend (wasi:http/service component)
canon run examples/todo-fullstack/server.can --addr 127.0.0.1:8090

# terminal 2 — the frontend (web-app bundle, served for the browser)
canon run examples/todo-fullstack/web.can --addr 127.0.0.1:8080
```

Open <http://127.0.0.1:8080>: add todos through the form, toggle and
remove them, then press **"Load todos from the server"** — the button
fetches `http://127.0.0.1:8090/todos` from the Canon backend and the
frontend decodes it with the *same shared code* that produced it.

## Two entries, one directory

The two entry files live side by side so they can reference the same
sibling files. The **frontend** is the Elm triple over `Todos`:

```canon
{{#include ../../../examples/todo-fullstack/web.can}}
```

The **backend** is a single `Request => Response` — method dispatch,
path routing, CORS headers, and `GET /todos` serving the seed list in
the shared encoding:

```canon
{{#include ../../../examples/todo-fullstack/server.can}}
```

## The shared contract

Neither entry defines `Todos` or its operations. Both reference them,
and the loader pulls in the same files for each compile:

- [`todos.can`](https://github.com/Almaju/canon/tree/main/examples/todo-fullstack/todos.can)
  — the `Todos` wire/state encoding and its operations as result
  newtypes (`AddedTodo`, `ToggledAt`, `RemovedAt`), the list renderer,
  and the pure-Canon string helpers. Compiled into *both* wasm binaries.
- [`line.can`](https://github.com/Almaju/canon/tree/main/examples/todo-fullstack/line.can)
  — one todo line: the `Flipped` toggle and the `<li>` renderer.
- [`title.can`](https://github.com/Almaju/canon/tree/main/examples/todo-fullstack/title.can)
  — the `Title` newtype.

There is no `canon.toml` here on purpose: the two entries share the
directory so they can reference the same sibling files. (Package-level
`[deps]` sharing is future work; same-directory sharing is the honest
mechanism available today, which is also why `just examples` does not
run this one.)

## What it demonstrates

- **Shared types across the stack.** `Todos` and its operations are
  written once. The backend serves the encoding; the frontend's `Load:`
  message swaps it straight into the model. No JSON schema, no client
  codegen — the type file is the protocol.
- **One language, two worlds.** `server.can` compiles to a standard
  `wasi:http/service` component; `web.can` compiles to the browser
  bundle. The entry-point *shape* selects the world — same compiler,
  same stdlib, no flags.
- **Host-mediated effects.** The frontend stays pure; the fetch happens
  in the JS host via the declarative `data-fetch` attribute, and the
  response arrives as an ordinary message through the `Update` arm.
- **CORS as plain data.** Response headers are constructed with
  `Headers()` and its setters — just values.
