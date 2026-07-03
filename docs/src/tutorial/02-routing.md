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

## Matching the Path

Dispatch doesn't stop at unions: it also works **by equality on
`String` and `Int`** scrutinees. Arms are literals, and the final arm is
a mandatory catch-all naming the scrutinee's type — literals can never
be exhaustive, so totality comes from the catch-all. Update `notes.can`:

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
        * (Some<String>) -> Response {
            String.(
                * ("/notes") -> Response { Response(Body("all the notes"), Headers(), Status(200)) }
                * (String) -> Response { Response(notFound(), Headers(), Status(404)) }
            )
        }
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

Read the inner dispatch as a route table: `"/notes"` is a route, and
the `(String)` catch-all is the 404 — "any other path". Inside every
arm the path value stays in scope as `String`, so the catch-all could
echo it back.

Three things worth noticing:

- **The status is computed.** Each arm builds its own `Response` with
  its own `Status` — there's no special mechanism for error responses,
  they're just values from a different branch.
- **`notFound` is a helper, and it returns `Body`, not `Response`.**
  Remember the entry-point rule from chapter 1: only `serve` may return
  `Response`. Helpers return data; the entry wraps data in the world
  type. Try changing `notFound` to return a `Response` — the compiler
  rejects the program as having an ambiguous HTTP entry.
- **Route order isn't yours to choose.** Literal arms follow canonical
  order — alphabetical for strings, ascending for ints — and the
  catch-all comes last. Don't sort by hand; `canon fmt` does it.

If you're wondering why there's no `if`/`else` to compare paths with:
an `if` is a dispatch on `Bool` with better marketing. Canon keeps the
one construct — unions dispatch by variant, `String`/`Int` dispatch by
equality with a catch-all. See [Dispatch](../tour/match.md) in the Tour
for the full argument.

Next: [real JSON routes](./03-json.md).
