# notes-api: A JSON Service

[`examples/notes-api`](https://github.com/Almaju/canon/tree/main/examples/notes-api):
the flagship backend example, a JSON API compiled to a standard
`wasi:http/service` component. About forty lines, zero server
boilerplate.

```sh
$ canon run examples/notes-api
HTTP handler detected: serving on http://127.0.0.1:8080

$ curl localhost:8080/notes
[{"id":1,"title":"ship canon v1"},{"id":2,"title":"write the docs"}]

$ curl localhost:8080/notes/1
{"id":1,"title":"ship canon v1"}

$ curl -i localhost:8080/other | head -1
HTTP/1.1 404 Not Found
```

## The Source

The whole program is one file, `src/main.can`:

```canon
IndexBody = Body

NotFound = Body

NoteOne = Body

NoteTwo = Body

Unit => IndexBody {
    [{"id":1,"title":"ship canon v1"},{"id":2,"title":"write the docs"}] -> Body
}

Unit => NotFound {
    {"error":"not found"} -> Body
}

Unit => NoteOne {
    {"id":1,"title":"ship canon v1"} -> Body
}

Unit => NoteTwo {
    {"id":2,"title":"write the docs"} -> Body
}

Request => Response {
    Request.path() -> (
        * None => Response { 400 -> Status -> Response(Headers() * NotFound()) }
        * Some<String> => Response {
            String -> (
                * "/notes" => Response { 200 -> Status -> Response(Headers() * IndexBody()) }
                * "/notes/1" => Response { 200 -> Status -> Response(Headers() * NoteOne()) }
                * "/notes/2" => Response { 200 -> Status -> Response(Headers() * NoteTwo()) }
                * String => Response { 404 -> Status -> Response(Headers() * NotFound()) }
            )
        }
    )
}
```

## What It Demonstrates

- **The entry-point rule.** The one arrow returning `Response` is the
  service -- an anonymous `Request => Response`, selected by signature.
  No `main`, no port in the program; the host decides how to serve it.
- **Constructors return values, not worlds.** Only the entry may return
  `Response`, so each note body is a constructor for its own `Body`
  newtype (`IndexBody`, `NoteOne`, ...), built with `Unit => IndexBody`.
  This is the layering the rule enforces.
- **JSON literals.** The bodies are JSON object/array literals,
  ordinary expressions that evaluate to the encoded text, so a static
  body costs no imports and no serializer.
- **Request introspection.** `Request.path()` returns
  `Option<String>`; dispatch on `(None, Some<String>)` extracts the
  live path.
- **Routing as dispatch.** There is no router DSL. The route table is
  **literal dispatch** on the path: one arm per route, alphabetical,
  with the mandatory `String` catch-all as the 404. Nested dispatch
  composes: union dispatch on the `Option`, literal dispatch on the
  payload inside the `Some` arm.
- **Per-route status codes.** `Status` is a value; each arm computes
  its own, piped in with `404 -> Status`.

See the tour's [Serving HTTP](../guide.md#serving-http) for the rule in
prose.

## The Compiled Shape

`canon build examples/notes-api` produces a component that imports only
`wasi:*` interfaces and exports `wasi:http/handler#handle`: the same
contract any compliant WASI Preview 3 HTTP host instantiates. Nothing
in the artifact is Canon-specific. See
[Deploying](../reference/deploying.md).
