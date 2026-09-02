# notes-api

A small JSON API served as a standard **`wasi:http/service`
component** — the flagship example for what a Canon HTTP program looks
like today.

```sh
canon run examples/notes-api                      # serves on 127.0.0.1:8080
canon run examples/notes-api --addr 127.0.0.1:9000
```

```sh
$ curl localhost:8080/notes
[{"id":1,"title":"ship canon v1"},{"id":2,"title":"write the docs"}]

$ curl localhost:8080/notes/1
{"id":1,"title":"ship canon v1"}

$ curl -i localhost:8080/other
HTTP/1.1 404 Not Found
not found

$ curl -i localhost:8080/
HTTP/1.1 303 See Other
location: /notes
```

## What it demonstrates

- **The entry-point rule**: the one free function returning `Response`
  is the handler — no `main`, no server boilerplate, no port wiring in
  the program itself.
- **Request introspection**: `Request.path()` returns
  `Option<String>`; dispatch on `(None, Some<String>)` extracts the
  live path.
- **Routing as dispatch**: no router DSL — `canon/router` (vendored
  under `deps/`, see `canon add`) turns the path into `Segments`, and
  a route is literal dispatch on `First` with its parameter at `At(2)`.
  The parameter parses with the prelude's `String -> Int`, and a bad
  id is the `Err` arm.
- **Response composition**: `JsonResponse`, `NotFound()`, and
  `Redirect` from `canon/router` are ordinary constructors over the
  prelude's `Response(Body * Headers * Status)` — the body rides a real
  `wasi:http` contents stream, the status and content type are set
  per helper.
- **Helpers return values, not worlds**: only the entry is `Request =>
  Response`; the routing lives in `RoutePath => Routed` and the note
  lookup in `Int => Shown`, both `= Response`.

## Compiled shape

`canon build examples/notes-api` produces a component that imports
only `wasi:*` interfaces and exports
`wasi:http/handler@0.3.0-rc-2026-03-15#handle` — the same contract any
compliant WASI HTTP host instantiates. `canon run` hosts it on the
embedded wasmtime.
