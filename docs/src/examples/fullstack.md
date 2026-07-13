# Fullstack: One Language, Both Sides

[`examples/todo-fullstack`](https://github.com/Almaju/canon/tree/main/examples/todo-fullstack):
a browser frontend and an HTTP backend, **sharing one set of types and
rendering code**, compiled from the same directory. No bundler, no npm,
no serialization framework -- the shared `.can` files *are* the contract.

```sh
# terminal 1 -- the backend (wasi:http/service component)
canon run examples/todo-fullstack/server.can --addr 127.0.0.1:8090

# terminal 2 -- the frontend (web-app bundle, served for the browser)
canon run examples/todo-fullstack/web.can --addr 127.0.0.1:8080
```

Open <http://127.0.0.1:8080>: add todos through the form, toggle and
remove them, then press **"Load todos from the server"** -- the button
fetches `http://127.0.0.1:8090/todos` from the Canon backend and the
frontend decodes it with the *same shared code* that produced it.

## Two entries, one directory

The two entry files live side by side so they can reference the same
sibling files. The **frontend** is the Elm triple over `Todos`:

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
    "<h1>Canon Todos</h1>"
        -> Joined(AddForm() -> String)
        -> Joined(1 -> RenderedItems(Todos) -> Ul)
        -> Joined(LoadButton() -> String)
        -> Div
}

Unit => Init {
    Title("check the canon backend")
        -> AddedTodo(Title("build the canon frontend") -> AddedTodo(Todos("")))
}

Unit => LoadButton {
    Attr("data-fetch=\"http://127.0.0.1:8090/todos\" data-fetch-msg=\"Load:\"")
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
path routing, CORS headers, and `GET /todos` serving the seed list in
the shared encoding:

```canon
CorsHeaders = Headers

Unit => CorsHeaders {
    Headers()
        .set("access-control-allow-origin" * "*")
        .set("content-type" * "text/plain")
}

Request => Response {
    Request.method() -> (
        * "GET" => Response {
            Request.path() -> (
                * None => Response { Body("bad request") -> Response(Status(400) * CorsHeaders()) }
                * Some<String> => Response {
                    String -> (
                        * "/todos" => Response {
                            Status(200) -> Response(CorsHeaders() * Seeded() -> String -> Body)
                        }
                        * String => Response {
                            Body("not found") -> Response(Status(404) * CorsHeaders())
                        }
                    )
                }
            )
        }
        * String => Response { Body("method not allowed") -> Response(Status(405) * CorsHeaders()) }
    )
}
```

## The shared contract

Neither entry defines `Todos` or its operations. Both reference them,
and the loader pulls in the same files for each compile:

- [`todos.can`](https://github.com/Almaju/canon/tree/main/examples/todo-fullstack/todos.can)
  -- the `Todos` wire/state encoding and its operations as result
  newtypes (`AddedTodo`, `ToggledAt`, `RemovedAt`), the list renderer,
  and the pure-Canon string helpers. Compiled into *both* wasm binaries.
- [`line.can`](https://github.com/Almaju/canon/tree/main/examples/todo-fullstack/line.can)
  -- one todo line: the `Flipped` toggle and the `<li>` renderer.
- [`title.can`](https://github.com/Almaju/canon/tree/main/examples/todo-fullstack/title.can)
  -- the `Title` newtype.

There is no `canon.toml` here on purpose: the two entries share the
directory so they can reference the same sibling files. (Package-level
`[deps]` sharing is future work; same-directory sharing is the honest
mechanism available today, which is also why `just examples` does not
run this one.)

## What it demonstrates

- **Shared types across the stack.** `Todos` and its operations are
  written once. The backend serves the encoding; the frontend's `Load:`
  message swaps it straight into the model. No JSON schema, no client
  codegen -- the type file is the protocol.
- **One language, two worlds.** `server.can` compiles to a standard
  `wasi:http/service` component; `web.can` compiles to the browser
  bundle. The entry-point *shape* selects the world -- same compiler,
  same stdlib, no flags.
- **Host-mediated effects.** The frontend stays pure; the fetch happens
  in the JS host via the declarative `data-fetch` attribute, and the
  response arrives as an ordinary message through the `Update` arm.
- **CORS as plain data.** Response headers are constructed with
  `Headers()` and its setters -- just values.
