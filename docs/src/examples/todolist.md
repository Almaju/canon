# A Todo List in the Browser

[`examples/todolist-web`](https://github.com/Almaju/canon/tree/main/examples/todolist-web):
a complete, interactive frontend -- add tasks, toggle them done, delete
them, clear the completed ones -- compiled to WebAssembly and running
**entirely in your browser**. No React, no bundler, no npm. The list
survives a reload because the host persists it to `localStorage`.

This very page is proof the model works: the documentation site you are
reading is itself a Canon web app of the same shape. Run the todo list
yourself from a checkout:

```sh
canon run examples/todolist-web        # serves on http://127.0.0.1:8080
```

## The whole app is the Elm triple

A Canon program becomes a web app by defining three anonymous,
type-selected constructors (see
[The Web Target](../reference/web-target.md)): `Todos => Html` (view),
`Unit => Init` (init), and `Todos * String => Update` (update). `Init`
and `Update` are model-alias markers that give init and update distinct
constructor keys. The model here is `Todos`, a newline-separated encoding
of `flag|title` lines; messages are prefix-parsed strings decoded with
the same pure-Canon string primitives the standard library uses
everywhere else. This is the entire entry file, `src/main.can`:

```canon
AddForm = ElAttr

ClearButton = Button

Init = AddedTodo

Prefix = String

Update = Todos

Unit => AddForm {
    Attr("data-msg-form=\"Add:\"")
        -> ElAttr(Attr("placeholder=\"What needs doing?\"") -> ElAttr("" * Tag("input")) * Tag("form"))
}

Unit => ClearButton {
    Msg("Clear") -> Button("Clear completed")
}

Todos => Html {
    Div(`<h1>Canon Todos</h1>{AddForm() -> String}{1 -> RenderedItems(Todos) -> Ul}{ClearButton() -> String}`)
}

Unit => Init {
    Title("toggle a task to mark it done")
        -> AddedTodo(Title("edit this list - it is saved in your browser") -> AddedTodo(Todos("")))
}

Todos * String => Update {
    String -> Substring(From(1) * To(4)) -> Prefix -> (
        * "Add:" => Todos {
            String
                -> Substring(From(5) * String -> Length -> To)
                -> Title
                -> AddedTodo(Todos)
        }
        * "Clea" => Todos { Todos -> Cleared }
        * "Dele" => Todos {
            String
                -> Substring(From(8) * String -> Length -> To)
                -> ParsedNum
                -> RemovedAt(Todos)
        }
        * "Togg" => Todos {
            String
                -> Substring(From(8) * String -> Length -> To)
                -> ParsedNum
                -> ToggledAt(Todos)
        }
        * Prefix => Todos { Todos }
    )
}
```

The `Update` constructor is a literal dispatch on the message's
four-character `Prefix`. Each arm is a pure fold: `Add:` appends,
`Toggle:N` flips one line, `Delete:N` drops one, `Clear` filters out the
completed. The catch-all returns the model unchanged. There is no
mutation and no local state -- the browser owns the event loop; the guest
is pure constructors piped with `->`.

## Persistence without a `localStorage` import

The guest never touches `localStorage`. It doesn't need to. A Canon web
app's model *is* a fold over its message history, so the host persists
the **message log** and replays it through `Update` on the next load --
rebuilding the identical model. That is the whole persistence story: the
generated `index.html` passes a storage key to `canonWebStart`, the host
appends each message to `localStorage` as it is sent, and reads the log
back on boot. If a saved log ever stops folding (say the app's message
grammar changed), the host discards it and starts fresh rather than
breaking. See [The Web Target](../reference/web-target.md).

## The rest of the program

The model operations are shared, ordinary Canon -- the same code would
run in a backend. Each operation is a **result newtype** named after
what it produces, so chaining is free (an `AddedTodo` flows anywhere a
`Todos` is expected). The pieces are split one type per file, as the
module system requires:

- [`src/todos.can`](https://github.com/Almaju/canon/tree/main/examples/todolist-web/src/todos.can)
  -- `Todos` holds the list and its folds (`AddedTodo`, `Cleared`,
  `ToggledAt`, `RemovedAt`, `RenderedItems`) plus the pure-Canon
  `FirstLine` / `RestLines` / `ParsedNum` string helpers.
- [`src/line.can`](https://github.com/Almaju/canon/tree/main/examples/todolist-web/src/line.can)
  -- `Line` renders one item as an `<li>` and its `Flipped` newtype
  toggles the done flag with recursive `ByteAt` / `Substring`
  primitives.
- [`src/title.can`](https://github.com/Almaju/canon/tree/main/examples/todolist-web/src/title.can)
  -- the `Title` newtype.

They read the same way as `main.can`: recursive dispatch over string
encodings, no host help. The same folds reappear, shared, in the
[fullstack example](./fullstack.md).

## What it demonstrates

- **A real frontend with no framework.** The `Init` / `Update` /
  `Todos => Html` triple *is* the app; `canon/std/web` supplies the HTML
  helpers and the declarative event attributes (`data-msg`,
  `data-msg-form`).
- **State that persists, with no effect in the guest.** `localStorage`
  is a host capability layered onto the message log -- the program stays
  pure and would compile unchanged for a server.
- **Dispatch as control flow.** Routing messages and branching on a
  task's done flag are both literal dispatch; there is no `if`.
