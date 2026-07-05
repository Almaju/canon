# A Todo List in the Browser

[`examples/todolist-web`](https://github.com/Almaju/canon/tree/main/examples/todolist-web):
a complete, interactive frontend — add tasks, toggle them done, delete
them, clear the completed ones — compiled to WebAssembly and running
**entirely in your browser**. No React, no bundler, no npm. The list
survives a reload because the host persists it to `localStorage`.

<iframe
  src="../runner/web/todolist/index.html"
  title="Canon todo list — live preview"
  style="width:100%;height:440px;border:1px solid var(--sidebar-active,#ccc);border-radius:8px;background:#fff;"
  loading="lazy"></iframe>

*The preview above is the real compiled program. Add a few tasks, then
refresh the page — they are still there. (Live previews need a browser
with [JSPI](https://github.com/WebAssembly/js-promise-integration)-free
core-wasm support — every modern browser — and the built site; a raw
local `mdbook serve` won't have the compiled bundle.)*

Run it yourself from a checkout:

```sh
canon run examples/todolist-web        # serves on http://127.0.0.1:8080
```

## The whole app is the Elm triple

A Canon program becomes a web app by defining three functions with the
conventional shapes — `init`, `update`, `view` (see
[The Web Target](../reference/web-target.md)).
The model here is `Todos`, a newline-separated encoding of `flag|title`
lines; messages are prefix-parsed strings decoded with the same
pure-Canon string primitives the standard library uses everywhere else.
This is the entire entry file, `src/main.can`:

```canon
{{#include ../../../examples/todolist-web/src/main.can}}
```

`update` is a literal dispatch on the message's four-character prefix.
Each arm is a pure fold: `Add:` appends, `Toggle:N` flips one line,
`Delete:N` drops one, `Clear` filters out the completed. The catch-all
returns the model unchanged. There is no mutation and no local state —
the browser owns the event loop; the guest is pure functions.

## Persistence without a `localStorage` import

The guest never touches `localStorage`. It doesn't need to. A Canon web
app's model *is* a fold over its message history, so the host persists
the **message log** and replays it through `update` on the next load —
rebuilding the identical model. That is the whole persistence story: the
generated `index.html` passes a storage key to `canonWebStart`, the host
appends each message to `localStorage` as it is sent, and reads the log
back on boot. If a saved log ever stops folding (say the app's message
grammar changed), the host discards it and starts fresh rather than
breaking. See [The Web Target](../reference/web-target.md).

## The rest of the program

The model operations are shared, ordinary Canon — the same code would
run in a backend. The pieces are split one type per file, as the module
system requires:

- [`src/todos.can`](https://github.com/Almaju/canon/tree/main/examples/todolist-web/src/todos.can)
  — `Todos` holds the list and its folds (`addTodo`, `clearDone`,
  `toggleAt`, `removeAt`, `renderItems`) plus the pure-Canon
  `firstLine` / `restLines` / `parseNum` string helpers.
- [`src/line.can`](https://github.com/Almaju/canon/tree/main/examples/todolist-web/src/line.can)
  — `Line` renders one item as an `<li>` and toggles its done flag with
  recursive `byteAt` / `substring` primitives.
- [`src/title.can`](https://github.com/Almaju/canon/tree/main/examples/todolist-web/src/title.can)
  — the `Title` newtype.

They read the same way as `main.can`: recursive dispatch over string
encodings, no host help. The same folds reappear, shared, in the
[fullstack example](./fullstack.md).

## What it demonstrates

- **A real frontend with no framework.** The `init`/`update`/`view`
  triple *is* the app; `canon/std/web` supplies the HTML helpers and the
  declarative event attributes (`data-msg`, `data-msg-form`).
- **State that persists, with no effect in the guest.** `localStorage`
  is a host capability layered onto the message log — the program stays
  pure and would compile unchanged for a server.
- **Dispatch as control flow.** Routing messages and branching on a
  task's done flag are both literal dispatch; there is no `if`.
