# Routing Is Dispatch

Our service says the same thing on every path. Time to route. Canon has
no route DSL and no router object — routing is **dispatch**, the same
branching construct used everywhere else in the language.

## What the Request Gives Us

`Request.path()` returns an `Option<String>`: the request's path and
query, or `None` if the request has no target. `Option` is a union
(`None + Some<T>`), and the way to branch on a union is to apply the
value to a product of handlers — one arm per variant:

```canon
Request.path().(
    * (None) -> Response { ... }
    * (Some<String>) -> Response { ... }
)
```

Inside the `Some<String>` arm, the unwrapped payload is in scope as
`String` — payloads are bound by their type name, like everything else
in Canon.

## Matching a Path

There is no string pattern matching; there is `String.eq`, which returns
a `Bool` — and `Bool` is a union too, so we dispatch on it. Update
`notes.can`:

```canon
use canon/std/http/Body
use canon/std/http/Headers
use canon/std/http/Request
use canon/std/http/Response
use canon/std/http/Status

notFound = () -> Body {
    Body("not found")
}

serve = (Request) -> Response {
    Request.path().(
        * (None) -> Response { Response(notFound(), Headers(), Status(400)) }
        * (Some<String>) -> Response { String.eq("/notes").( * (False) -> Response { Response(notFound(), Headers(), Status(404)) } * (True) -> Response { Response(Body("all the notes"), Headers(), Status(200)) }) }
    )
}
```

```sh
$ canon run notes.can
$ curl localhost:8080/notes
all the notes
$ curl -i localhost:8080/other | head -1
HTTP/1.1 404 Not Found
```

Two things worth noticing:

- **The status is computed.** Each dispatch arm builds its own
  `Response` with its own `Status` — there's no special mechanism for
  error responses, they're just values from a different branch.
- **`notFound` is a helper, and it returns `Body`, not `Response`.**
  Remember the entry-point rule from chapter 1: only `serve` may return
  `Response`. Helpers return data; the entry wraps data in the world
  type. Try changing `notFound` to return a `Response` — the compiler
  rejects the program as having an ambiguous HTTP entry.

## About That Long Line

The inner dispatch sits on one line because that is how `canon fmt`
lays out a dispatch nested inside an arm — only a body's top-level
dispatch gets the multi-line treatment. Deep nesting is therefore
*visibly* expensive, and that's deliberate pressure: when a route table
outgrows a couple of `eq` checks, the language wants you to extract
helpers and push work into data — which is exactly what the
[next chapter](./03-json.md) starts doing.

If you're wondering why there's no `if`/`else` to tidy this with: an
`if` is a dispatch on `Bool` with better marketing. Canon keeps the one
construct. See [Dispatch](../tour/match.md) in the Tour for the full
argument.
