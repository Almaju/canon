# Fullstack: One Language, One Command

[`examples/todo-fullstack`](https://github.com/Almaju/canon/tree/main/examples/todo-fullstack):
a browser frontend and an HTTP backend, **sharing one set of types and
rendering code**, compiled and served by one command. No bundler, no
npm, no serialization framework -- the shared `.can` files *are* the
contract.

```sh
canon run examples/todo-fullstack
```

Open <http://127.0.0.1:8080>: add todos through the form, toggle and
remove them, clear the completed ones, then press **"Load todos from
the server"** -- the button fetches `/todos` from the Canon backend
*on the same origin* and the frontend decodes it with the *same shared
code* that produced it. The list also survives a reload: the host
persists it to `localStorage` with no effect in the guest.

## A fullstack package

The directory declares itself by shape: one `src/` file declares the
Elm triple, another declares `Request => Response`, and that makes it
a **fullstack package** -- no reserved filenames (`web.can` and
`server.can` here are just descriptive), no flags, no config. Each
entry is still one program exporting one world -- the frontend a
browser bundle, the backend a `wasi:http/service` component -- but
`canon run` compiles both and serves them from one process: the web
bundle owns `/`, `/index.html`, `/canon-web.js`, and the app's
`.wasm`; every other request dispatches to the server. One origin, so
the frontend's fetch URL is relative and there is no CORS to
configure. `canon build` writes both artifacts into `build/`.

The **frontend** is the Elm triple over `Todos`:

```canon
AddForm = ElAttr

ClearButton = Button

Init = AddedTodo

LoadButton = ElAttr

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
    Div(`<h1>Canon Todos</h1>{AddForm() -> String}{1 -> RenderedItems(Todos) -> Ul}{
        ClearButton() -> String
    }{LoadButton() -> String}`)
}

Unit => Init {
    Title("check the canon backend")
        -> AddedTodo(Title("build the canon frontend") -> AddedTodo(Todos("")))
}

Unit => LoadButton {
    Attr("data-fetch=\"/todos\" data-fetch-msg=\"Load:\"")
        -> ElAttr("Load todos from the server" * Tag("button"))
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
        * "Load" => Todos { String -> Substring(From(6) * String -> Length -> To) -> Todos }
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
`Toggle:N` flips one line, `Delete:N` drops one, `Clear` filters out
the completed, `Load:payload` swaps the server's encoding straight
into the model. The catch-all returns the model unchanged -- no
mutation, no local state; the browser owns the event loop and the
guest is pure constructors piped with `->`.

### Persistence without a `localStorage` import

The guest never touches `localStorage` -- it doesn't need to. The
model is a fold over its message history, so the host persists the
**message log** and replays it through `Update` on the next load,
rebuilding the identical model. A log that stops folding is discarded
rather than allowed to brick the app. See
[The Web Target](../reference/web-target.md).

The **backend** is a single `Request => Response` -- method dispatch,
path routing, and `GET /todos` serving the seed list in the shared
encoding:

```canon
PlainText = Headers

Unit => PlainText {
    Headers().set("content-type" * "text/plain")
}

Request => Response {
    Request.method() -> (
        * "GET" => Response {
            Request.path() -> (
                * None => Response { Body("bad request") -> Response(Status(400) * PlainText()) }
                * Some<String> => Response {
                    String -> (
                        * "/todos" => Response {
                            Status(200) -> Response(PlainText() * Seeded() -> String -> Body)
                        }
                        * String => Response {
                            Body("not found") -> Response(Status(404) * PlainText())
                        }
                    )
                }
            )
        }
        * String => Response { Body("method not allowed") -> Response(Status(405) * PlainText()) }
    )
}
```

## The shared contract

Neither entry defines `Todos` or its operations. Both reference them,
and the loader pulls in the same sibling files for each compile:

- [`src/todos.can`](https://github.com/Almaju/canon/tree/main/examples/todo-fullstack/src/todos.can)
  -- the `Todos` wire/state encoding and its operations as result
  newtypes (`AddedTodo`, `Cleared`, `RemovedAt`, `ToggledAt`), the list
  renderer, and the pure-Canon string helpers. Compiled into *both*
  wasm binaries.
- [`src/line.can`](https://github.com/Almaju/canon/tree/main/examples/todo-fullstack/src/line.can)
  -- one todo line: the `Flipped` toggle and the `<li>` renderer.
- [`src/title.can`](https://github.com/Almaju/canon/tree/main/examples/todo-fullstack/src/title.can)
  -- the `Title` newtype.

## What it demonstrates

`Todos` and its operations are written once and compiled into both
binaries, so the type file *is* the protocol — no JSON schema, no
client codegen. The entry-point shape picks component or browser bundle,
and one `canon run` serves both on one origin, so nothing is
configured, not even CORS. The frontend stays pure: the fetch happens in
the JS host via `data-fetch` and arrives as an ordinary `Update`
message.
