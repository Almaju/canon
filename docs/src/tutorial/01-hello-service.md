# A Service in Five Lines

Create a file named `notes.can`:

```canon
serve = (Request) -> Response {
    Response(Body("hello from canon") * Headers() * Status(200))
}
```

Run it:

```sh
$ canon run notes.can
HTTP handler detected: serving on http://127.0.0.1:8080
```

And from another terminal:

```sh
$ curl localhost:8080
hello from canon
```

A working HTTP service, with no `main`, no server object, no port
binding, no route registration, and no framework import.

## The Entry-Point Rule

Canon selects a program's entry **by return type, not by name**. A free
function returning `Unit` makes the program a CLI command; a free
function returning `Response` makes it an HTTP service. The compiler
scans the module, finds exactly one function with a world-shaped return
type, and lifts it as the component's export, here
`wasi:http/handler#handle`.

Two consequences you'll run into later:

- **Exactly one** function per program may return `Response`. Helper
  functions must return ordinary values (`Body`, `String`, your own
  types). The restriction is the layering the rule wants: helpers
  return data, the entry returns the world.
- The function's name doesn't matter. We call it `serve`; `greet` or
  `handle` would work identically.

## The Response

`Response` is an ordinary product type (`Body * Headers * Status`), so
its constructor takes a value-level product, the components joined
with `*` in alphabetical order like every Canon product:

```canon
Response(Body("hello from canon") * Headers() * Status(200))
```

`*` is the same operator at both levels: it composes product *types* in
declarations and product *values* at construction sites.

- `Body` is a newtype over `String`: the response body.
- `Headers()` constructs an empty header set.
- `Status` is a newtype over `Int`: the HTTP status code. A status is a
  value, so it can be computed, which is what routing will do in the
  next chapter.

## Formatting

Canon has exactly one code layout, and the compiler holds you to it. If
a file isn't canonically formatted, `canon run` and `canon check` tell
you to run:

```sh
canon fmt notes.can
```

Get in the habit now; the compiler will remind you the moment a line
wraps differently than the formatter would have it.

Next: [routing](./02-routing.md), a language feature you already have.
