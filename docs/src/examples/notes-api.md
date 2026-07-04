# notes-api: A JSON Service

[`examples/notes-api`](https://github.com/Almaju/canon/tree/main/examples/notes-api):
the flagship example, a JSON API compiled to a standard
`wasi:http/service` component. About forty lines, zero server
boilerplate. The [tutorial](../tutorial/index.md) builds this program
up step by step; this page is the finished artifact.

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

```canon
indexBody = () -> Body {
    Body([{"id":1,"title":"ship canon v1"},{"id":2,"title":"write the docs"}])
}

notFound = () -> Body {
    Body({"error":"not found"})
}

noteOne = () -> Body {
    Body({"id":1,"title":"ship canon v1"})
}

noteTwo = () -> Body {
    Body({"id":2,"title":"write the docs"})
}

serve = (Request) -> Response {
    Request.path().(
        * (None) -> Response { Response(notFound() * Headers() * Status(400)) }
        * (Some<String>) -> Response {
            String.(
                * ("/notes") -> Response { Response(indexBody() * Headers() * Status(200)) }
                * ("/notes/1") -> Response { Response(noteOne() * Headers() * Status(200)) }
                * ("/notes/2") -> Response { Response(noteTwo() * Headers() * Status(200)) }
                * (String) -> Response { Response(notFound() * Headers() * Status(404)) }
            )
        }
    )
}
```

## What It Demonstrates

- **The entry-point rule.** The one function returning `Response` is
  the service. No `main`, no port in the program; the host decides how
  to serve it.
- **Helpers return values, not worlds.** Only `serve` may return
  `Response`, so the note bodies are `() -> Body` functions. This is
  the layering the rule enforces.
- **JSON literals.** The bodies are JSON object/array literals,
  ordinary expressions that evaluate to the encoded text, so a static
  body costs no imports and no serializer.
- **Request introspection.** `Request.path()` returns
  `Option<String>`; dispatch on `(None, Some<String>)` extracts the
  live path.
- **Routing as dispatch.** There is no router DSL. The route table is
  **literal dispatch** on the path: one arm per route, alphabetical,
  with the mandatory `(String)` catch-all as the 404. Nested dispatch
  composes: union dispatch on the `Option`, literal dispatch on the
  payload inside the `Some` arm.
- **Per-route status codes.** `Status` is a value; each arm computes
  its own.

## The Compiled Shape

`canon build examples/notes-api` produces a component that imports only
`wasi:*` interfaces and exports `wasi:http/handler#handle`: the same
contract any compliant WASI Preview 3 HTTP host instantiates. Nothing
in the artifact is Canon-specific. See
[Ship a Component](../tutorial/06-ship-it.md) and
[Deploying](../reference/deploying.md).
