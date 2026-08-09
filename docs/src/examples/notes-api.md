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
        * None => Response { Status(400) -> Response(Headers() * NotFound()) }
        * Some<String> => Response {
            String -> (
                * "/notes" => Response { Status(200) -> Response(Headers() * IndexBody()) }
                * "/notes/1" => Response { Status(200) -> Response(Headers() * NoteOne()) }
                * "/notes/2" => Response { Status(200) -> Response(Headers() * NoteTwo()) }
                * String => Response { Status(404) -> Response(Headers() * NotFound()) }
            )
        }
    )
}
```

## What It Demonstrates

- **Constructors return values, not worlds.** Only the entry may return
  `Response`, so each note body is a constructor for its own `Body`
  newtype (`IndexBody`, `NoteOne`, ...). That is the layering the
  one-world rule enforces.
- **Request introspection.** `Request.path()` returns
  `Option<String>`, so union dispatch on the `Option` wraps literal
  dispatch on the path inside the `Some` arm — nested dispatch composes.
- **Per-route status codes.** `Status` is a value each arm computes for
  itself, piped in with `404 -> Status`.

The route table is [literal dispatch](../tour/literal-dispatch.md) and
the entry is chosen by [its signature](../tour/worlds.md); neither is
special-cased for HTTP.

## The Compiled Shape

`canon build examples/notes-api` produces a component that imports only
`wasi:*` interfaces and exports `wasi:http/handler#handle`: the same
contract any compliant WASI Preview 3 HTTP host instantiates. Nothing
in the artifact is Canon-specific. See
[Deploying](../reference/deploying.md).
