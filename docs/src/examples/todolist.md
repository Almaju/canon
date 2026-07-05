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

```canon
Prefix = String

addForm = () -> Html {
    Attr("data-msg-form=\"Add:\"")
        .elAttr(Attr("placeholder=\"What needs doing?\"").elAttr("", Tag("input")).String, Tag("form"))
}

clearButton = () -> Html {
    Msg("Clear").Button("Clear completed")
}

init = () -> Todos {
    Title("toggle a task to mark it done")
        .addTodo(Title("edit this list - it is saved in your browser").addTodo(Todos("")))
}

update = (Todos * String) -> Todos {
    Prefix(String.substring(1, 4)).(
        * ("Add:") -> Todos { Title(String.substring(5, String.length())).addTodo(Todos) }
        * ("Clea") -> Todos { Todos.clearDone() }
        * ("Dele") -> Todos { String.substring(8, String.length()).parseNum().removeAt(Todos) }
        * ("Togg") -> Todos { String.substring(8, String.length()).parseNum().toggleAt(Todos) }
        * (Prefix) -> Todos { Todos }
    )
}

view = (Todos) -> Html {
    "<h1>Canon Todos</h1>"
        .concat(addForm().String)
        .concat(1.renderItems(Todos).Ul().String)
        .concat(clearButton().String)
        .Div()
}
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
run in a backend. `Todos` holds the list and its folds:

```canon
Todos = String

addTodo = (Title * Todos) -> Todos {
    Todos(Todos.String.concat("0|").concat(Title.String).concat("\n"))
}

clearDone = (Todos) -> Todos {
    Todos.String.length().eq(0).(
        * (False) -> Todos {
            Todos.String.byteAt(1).eq(49).(
                * (False) -> Todos {
                    Todos(Todos.String.firstLine().concat("\n").concat(Todos(Todos.String.restLines()).clearDone().String))
                }
                * (True) -> Todos { Todos(Todos.String.restLines()).clearDone() }
            )
        }
        * (True) -> Todos { Todos }
    )
}
```

(`firstLine`, `restLines`, `parseNum`, `removeAt`, `renderItems`, and
`toggleAt` round out the file — see the
[full source](https://github.com/Almaju/canon/tree/main/examples/todolist-web/src).)

`Line` renders one item and toggles its done flag:

```canon
Line = String

flip = (Line) -> Line {
    Line.byteAt(1).eq(48).(
        * (False) -> Line { Line("0".concat(Line.substring(2, Line.length()))) }
        * (True) -> Line { Line("1".concat(Line.substring(2, Line.length()))) }
    )
}

renderItem = (Int * Line) -> Html {
    Line.byteAt(1).eq(49).(
        * (False) -> Html {
            Line.substring(3, Line.length()).Escaped()
                .concat(" ")
                .concat(Msg("Toggle:".concat(Int.String())).Button("done"))
                .concat(" ")
                .concat(Msg("Delete:".concat(Int.String())).Button("remove"))
                .Li()
        }
        * (True) -> Html {
            "<s>"
                .concat(Line.substring(3, Line.length()).Escaped())
                .concat("</s> ")
                .concat(Msg("Toggle:".concat(Int.String())).Button("undo"))
                .concat(" ")
                .concat(Msg("Delete:".concat(Int.String())).Button("remove"))
                .Li()
        }
    )
}
```

## What it demonstrates

- **A real frontend with no framework.** The `init`/`update`/`view`
  triple *is* the app; `canon/std/web` supplies the HTML helpers and the
  declarative event attributes (`data-msg`, `data-msg-form`).
- **State that persists, with no effect in the guest.** `localStorage`
  is a host capability layered onto the message log — the program stays
  pure and would compile unchanged for a server.
- **Dispatch as control flow.** Routing messages and branching on a
  task's done flag are both literal dispatch; there is no `if`.
