# notes-api: A JSON Service

[`examples/notes-api`](https://github.com/Almaju/canon/tree/main/examples/notes-api)
— the flagship example: a JSON API compiled to a standard
`wasi:http/service` component. About forty lines, zero server
boilerplate. (The [tutorial](../tutorial/index.md) builds this program
up step by step; this page is the finished artifact.)

```sh
$ canon run examples/notes-api
HTTP handler detected — serving on http://127.0.0.1:8080

$ curl localhost:8080/notes
[{"id":1,"title":"ship canon v1"},{"id":2,"title":"write the docs"}]

$ curl localhost:8080/notes/1
{"id":1,"title":"ship canon v1"}

$ curl -i localhost:8080/other | head -1
HTTP/1.1 404 Not Found
```

## The Source

```canon
use canon/std/http/Body
use canon/std/http/Headers
use canon/std/http/Request
use canon/std/http/Response
use canon/std/http/Status

indexBody = () -> Body {
    Body("[{\"id\":1,\"title\":\"ship canon v1\"},{\"id\":2,\"title\":\"write the docs\"}]")
}

notFound = () -> Body {
    Body("{\"error\":\"not found\"}")
}

noteOne = () -> Body {
    Body("{\"id\":1,\"title\":\"ship canon v1\"}")
}

noteTwo = () -> Body {
    Body("{\"id\":2,\"title\":\"write the docs\"}")
}

serve = (Request) -> Response {
    Request.path().(
        * (None) -> Response { Response(notFound(), Headers(), Status(400)) }
        * (Some<String>) -> Response { String.eq("/notes").( * (False) -> Response { String.eq("/notes/1").( * (False) -> Response { String.eq("/notes/2").( * (False) -> Response { Response(notFound(), Headers(), Status(404)) } * (True) -> Response { Response(noteTwo(), Headers(), Status(200)) }) } * (True) -> Response { Response(noteOne(), Headers(), Status(200)) }) } * (True) -> Response { Response(indexBody(), Headers(), Status(200)) }) }
    )
}
```

## What It Demonstrates

- **The entry-point rule.** The one function returning `Response` is
  the service. No `main`, no port in the program — the host decides how
  to serve it.
- **Helpers return values, not worlds.** Only `serve` may return
  `Response`, so the note bodies are `() -> Body` functions —
  the layering the rule enforces.
- **Request introspection.** `Request.path()` returns
  `Option<String>`; dispatch on `(None, Some<String>)` extracts the
  live path.
- **Routing as dispatch.** No router DSL — route matching is ordinary
  union dispatch over `String.eq` results. Verbose by design: when this
  hurts, the signal is "extract helpers / add a stdlib routing
  combinator", not "add syntax".
- **Per-route status codes.** `Status` is a value; each arm computes
  its own.

## The Compiled Shape

`canon build examples/notes-api` produces a component that imports only
`wasi:*` interfaces and exports `wasi:http/handler#handle` — the same
contract any compliant WASI Preview 3 HTTP host instantiates. Nothing
in the artifact is Canon-specific. See
[Ship a Component](../tutorial/06-ship-it.md) and
[Deploying](../reference/deploying.md).
