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
{"error":"not found"}
```

## What it demonstrates

- **The entry-point rule**: the one free function returning `Response`
  is the handler — no `main`, no server boilerplate, no port wiring in
  the program itself.
- **Request introspection**: `Request.path()` returns
  `Option<String>`; dispatch on `(None, Some<String>)` extracts the
  live path.
- **Routing as dispatch**: no router DSL — route matching is ordinary
  union dispatch over `String.eq(..)` results. Verbose by design;
  when it hurts, that's the signal for a stdlib routing helper, not
  special syntax.
- **Response composition**: `Response(Body(..) * Headers() *
  Status(..))` — the body rides a real `wasi:http` contents stream,
  the status is set per-route.
- **Helpers return values, not worlds**: only the entry may return
  `Response`, so the note bodies are `() -> Body` functions.

## Compiled shape

`canon build examples/notes-api` produces a component that imports
only `wasi:*` interfaces and exports
`wasi:http/handler@0.3.0-rc-2026-03-15#handle` — the same contract any
compliant WASI HTTP host instantiates. `canon run` hosts it on the
embedded wasmtime.
