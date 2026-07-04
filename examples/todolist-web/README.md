# todolist-web

An interactive todo list — a Canon **web app** (the Elm-architecture
`init`/`update`/`view` triple) compiled to WebAssembly and driven by the
bundled JS host. Add tasks, toggle them done, delete them, clear the
completed ones. The list persists across reloads via `localStorage`,
with no `localStorage` import in the guest.

```sh
canon run examples/todolist-web          # serves on 127.0.0.1:8080
canon run examples/todolist-web --addr 127.0.0.1:9000
```

Open the printed URL, add a few tasks, then refresh — they are still
there. `canon build examples/todolist-web` writes the deployable
three-file bundle (`todolist-web.wasm`, `canon-web.js`, `index.html`)
instead. A live, embedded preview runs in the book:
[A Todo List in the Browser](../../docs/src/examples/todolist.md).

## Layout

| File | Role |
|---|---|
| `src/main.can` | **Entry.** The Elm triple over `Todos` as the model. `update` is a literal dispatch on the message prefix (`Add:`, `Toggle:N`, `Delete:N`, `Clear`); `view` renders the whole page. |
| `src/todos.can` | The `Todos` state encoding (newline-separated `flag\|title` lines) and its folds: `addTodo`, `clearDone`, `removeAt`, `toggleAt`, plus the pure-Canon string helpers they need. |
| `src/line.can` | One todo line: the `flip` toggle and the `<li>` renderer with its `Toggle:`/`Delete:` message buttons. |
| `src/title.can` | The `Title` newtype. |

## What it demonstrates

- **A real frontend with no framework.** Defining `init`/`update`/`view`
  with the conventional shapes *is* what makes the program a web app —
  no registration, no build config. `canon/std/web` supplies the HTML
  helpers and the declarative event attributes.
- **`localStorage` with a pure guest.** The model is a fold over the
  message history, so the host persists the *message log* and replays it
  on load. The program stays effect-free and would compile unchanged for
  a backend. See [The Web Target](../../docs/src/reference/web-target.md).
- **Dispatch as control flow.** Message routing and the done/undone
  branch are both literal dispatch — there is no `if`.
