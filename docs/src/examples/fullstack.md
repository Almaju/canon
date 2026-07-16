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
remove them, then press **"Load todos from the server"** -- the button
fetches `/todos` from the Canon backend *on the same origin* and the
frontend decodes it with the *same shared code* that produced it.

## A fullstack package

The directory declares itself, structurally: `src/web.can` +
`src/server.can` in place of `src/main.can` make it a **fullstack
package**. Each entry is still one program exporting one world -- the
frontend a browser bundle, the backend a `wasi:http/service`
component -- but `canon run` compiles both and serves them from one
process: the web bundle owns `/`, `/index.html`, `/canon-web.js`, and
the app's `.wasm`; every other request dispatches to the server. One
origin, so the frontend's fetch URL is relative and there is no CORS
to configure. `canon build` writes both artifacts into `build/`.

The **frontend** is the Elm triple over `Todos`:

```canon
AddForm = ElAttr

Init = AddedTodo

LoadButton = ElAttr

Prefix = String

Update = Todos

Unit => AddForm {
    Attr("data-msg-form=\"Add:\"")
        -> ElAttr(Attr("placeholder=\"What needs doing?\"") -> ElAttr("" * Tag("input")) * Tag("form"))
}

Todos => Html {
    Div(`<h1>Canon Todos</h1>{AddForm() -> String}{1 -> RenderedItems(Todos) -> Ul}{
        LoadButton() -> String
    }`)
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
  newtypes (`AddedTodo`, `ToggledAt`, `RemovedAt`), the list renderer,
  and the pure-Canon string helpers. Compiled into *both* wasm binaries.
- [`src/line.can`](https://github.com/Almaju/canon/tree/main/examples/todo-fullstack/src/line.can)
  -- one todo line: the `Flipped` toggle and the `<li>` renderer.
- [`src/title.can`](https://github.com/Almaju/canon/tree/main/examples/todo-fullstack/src/title.can)
  -- the `Title` newtype.

## What it demonstrates

**Shared types across the stack** — `Todos` and its operations are
written once, and the type file is the protocol (no JSON schema, no
client codegen). **One language, two worlds, one command** — the
entry-point shape selects component vs browser bundle; `canon run`
serves both on one origin, so nothing is configured, not even CORS.
The frontend stays pure (the fetch happens in the JS host via
`data-fetch`, arriving as an ordinary `Update` message), and response
headers are just values.
