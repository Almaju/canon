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
AddForm = ElAttr

ClearButton = Button

Prefix = String

Unit => AddForm {
    Attr("data-msg-form=\"Add:\"")
        -> ElAttr(Attr("placeholder=\"What needs doing?\"") -> ElAttr("", Tag("input")), Tag("form"))
}

Unit => ClearButton {
    Msg("Clear") -> Button("Clear completed")
}

init = () => Todos {
    Title("toggle a task to mark it done")
        -> AddedTodo(Title("edit this list - it is saved in your browser") -> AddedTodo(Todos("")))
}

update = (Todos * String) => Todos {
    Prefix(String -> Substring(1, 4)).(
        * ("Add:") => Todos { Title(String -> Substring(5, String -> Length)) -> AddedTodo(Todos) }
        * ("Clea") => Todos { Todos -> Cleared }
        * ("Dele") => Todos { String -> Substring(8, String -> Length) -> ParsedNum -> RemovedAt(Todos) }
        * ("Togg") => Todos { String -> Substring(8, String -> Length) -> ParsedNum -> ToggledAt(Todos) }
        * (Prefix) => Todos { Todos }
    )
}

view = (Todos) => Html {
    "<h1>Canon Todos</h1>"
        -> Joined(AddForm() -> String)
        -> Joined(1 -> RenderedItems(Todos) -> Ul)
        -> Joined(ClearButton() -> String)
        -> Div
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
AddedTodo = Todos

Cleared = Todos

Todos = String

(Title * Todos) => AddedTodo {
    Todos(Todos.String -> Joined("0|") -> Joined(Title.String) -> Joined("\n"))
}

Todos => Cleared {
    Todos.String -> Length -> Eq(0).(
        * (False) => Cleared {
            Todos.String -> ByteAt(1) -> Eq(49).(
                * (False) => Cleared {
                    Todos(Todos.String -> FirstLine -> Joined("\n") -> Joined(Todos(Todos.String -> RestLines) -> Cleared -> String))
                }
                * (True) => Cleared { Todos(Todos.String -> RestLines) -> Cleared }
            )
        }
        * (True) => Cleared { Todos }
    )
}
```

Every operation is named after the value it produces: `AddedTodo`,
`Cleared`, and so on — result newtypes over `Todos`, reached with the
`->` pipe. (`FirstLine`, `RestLines`, `ParsedNum`, `RemovedAt`,
`RenderedItems`, and `ToggledAt` round out the file — see the
[full source](https://github.com/Almaju/canon/tree/main/examples/todolist-web/src).)

`Line` renders one item and toggles its done flag:

```canon
Flipped = Line

Line = String

RenderedItem = Html

Line => Flipped {
    Line -> ByteAt(1) -> Eq(48).(
        * (False) => Flipped { Line("0" -> Joined(Line -> Substring(2, Line -> Length))) }
        * (True) => Flipped { Line("1" -> Joined(Line -> Substring(2, Line -> Length))) }
    )
}

(Int * Line) => RenderedItem {
    Line -> ByteAt(1) -> Eq(49).(
        * (False) => RenderedItem {
            Line
                -> Substring(3, Line -> Length)
                -> Escaped
                -> Joined(" ")
                -> Joined(Msg("Toggle:" -> Joined(Int -> String)) -> Button("done"))
                -> Joined(" ")
                -> Joined(Msg("Delete:" -> Joined(Int -> String)) -> Button("remove"))
                -> Li
        }
        * (True) => RenderedItem {
            "<s>"
                -> Joined(Line -> Substring(3, Line -> Length) -> Escaped)
                -> Joined("</s> ")
                -> Joined(Msg("Toggle:" -> Joined(Int -> String)) -> Button("undo"))
                -> Joined(" ")
                -> Joined(Msg("Delete:" -> Joined(Int -> String)) -> Button("remove"))
                -> Li
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
