# Serving HTTP

A Canon program becomes an HTTP service by declaring **one free
function that returns `Response`**. There is no server object, no
router registration, no port in the program — the function *is* the
service, and the host decides how to run it.

```canon
use canon/std/http/Body
use canon/std/http/Headers
use canon/std/http/Request
use canon/std/http/Response
use canon/std/http/Status

greet = (Request) -> Response {
    Response(Body("hello from canon"), Headers(), Status(200))
}
```

```sh
$ canon run greet.can
HTTP handler detected — serving on http://127.0.0.1:8080

$ curl localhost:8080
hello from canon
```

`canon run … --addr <ip:port>` picks a different address.

## The entry-point rule

The same rule that makes `main` a CLI program makes this an HTTP
program: **the compiler selects the entry by return type**. A free
function returning `Unit` is a CLI command; a free function returning
`Response` is an HTTP handler. Exactly one function per program may
return a world type — helpers must return ordinary values:

```canon
notFound = () -> Body {
    Body("{\"error\":\"not found\"}")
}
```

A second `Response`-returning function is a compile error ("ambiguous
HTTP entry"), and mixing `main` with a handler is too — a component
exports exactly one world.

## Routing is dispatch

`Request.path()` returns `Option<String>` — the live path and query.
There is no route DSL; route matching is the same union dispatch used
everywhere else in Canon:

```canon
serve = (Request) -> Response {
    Request.path().(
        * (None) -> Response { Response(notFound(), Headers(), Status(400)) }
        * (Some<String>) -> Response {
            String.eq("/notes").(
                * (False) -> Response { Response(notFound(), Headers(), Status(404)) }
                * (True) -> Response { Response(indexBody(), Headers(), Status(200)) }
            )
        }
    )
}
```

## Response composition

`Response(Body * Headers * Status)` — fields in alphabetical order,
like every Canon product:

- **`Body`** is a `String` newtype. The compiled component streams it
  through a real `wasi:http` contents stream with a correct
  `Content-Length`.
- **`Headers()`** constructs an empty header set. (Setting individual
  headers is not wired up yet.)
- **`Status`** is an `Int` newtype and can be computed: dispatch to a
  `Status(404)` arm, or build it from any expression.

## The capstone: notes-api

[`examples/notes-api`](https://github.com/Almaju/canon/tree/main/examples/notes-api)
puts all of it together — a JSON API with an index route, per-item
routes, and a 404 fallback, in about forty lines with zero server
boilerplate:

```sh
$ canon run examples/notes-api
$ curl localhost:8080/notes
[{"id":1,"title":"ship canon v1"},{"id":2,"title":"write the docs"}]
$ curl localhost:8080/notes/1
{"id":1,"title":"ship canon v1"}
$ curl -i localhost:8080/other | head -1
HTTP/1.1 404 Not Found
```

Read its `src/main.can` top to bottom: body helpers first
(alphabetical), then the single `serve` entry that routes by
dispatching on `Request.path()`.

## What you actually built

`canon build` on an HTTP program emits a standard **`wasi:http/service`
component**: it imports only `wasi:*` interfaces and exports
`wasi:http/handler@0.3.0-rc-2026-03-15#handle` — the same contract any
compliant WASI HTTP host instantiates. `canon run` hosts it on the
embedded wasmtime, but the `.wasm` itself is not tied to Canon's
runtime in any way.

Not wired up yet (tracked in `WASI-HTTP-HANDLER.md`): reading the
request method, headers, and body; setting response headers; and
streaming response bodies (`STREAMING.md`).
