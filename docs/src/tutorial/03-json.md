# JSON, Without a Framework

A notes API should serve notes, plural, as JSON. In this chapter the
service grows its real routes: `/notes` returns a JSON array, `/notes/1`
returns a single note, everything else gets a JSON error.

## JSON Is a Literal

JSON is part of Canon's syntax. An object or array literal is an
ordinary expression, and it evaluates to the encoded JSON text — a
`String`-shaped value holding exactly the bytes the client will
receive:

```canon
notFound = () -> Body {
    Body({"error":"not found"})
}
```

No serializer, no schema, no middleware — and no escaped quotes. The
literal *is* the wire format. Arrays work the same way, and literals
nest:

```canon
indexBody = () -> Body {
    Body([{"title":"ship canon v1"},{"title":"write the docs"}])
}
```

> Note the spacing: `{"error":"not found"}`, not `{"error": "not
> found"}`. JSON literals are code, so `canon fmt` owns their layout
> like everything else's — compact, no spaces after `:` or `,`.

And there's nothing to import: JSON is part of the prelude, like
`Option` and `Result`. A static literal compiles down to a plain
string; when a program reaches for the JSON *machinery* — the
validating `Json(...)` constructor, or **interpolation** like
`{"answer":Int.mul(2)}`, which converts values through their `ToJson`
instance — the compiler pulls in `canon/std/Json` automatically.

One honest limitation: the `ToJson` instances are currently backed by
host functions that the `wasi:http/service` world can't satisfy, so
interpolation is **CLI-only for now** — use it in a handler and the
build fails with an error naming exactly which imports the HTTP world
can't provide. Inside a handler, keep literals fully static.

## The Full Program

```canon
use canon/std/http/Body
use canon/std/http/Headers
use canon/std/http/Request
use canon/std/http/Response
use canon/std/http/Status

indexBody = () -> Body {
    Body([{"title":"ship canon v1"},{"title":"write the docs"}])
}

notFound = () -> Body {
    Body({"error":"not found"})
}

noteOneBody = () -> Body {
    Body({"title":"ship canon v1"})
}

serve = (Request) -> Response {
    Request.path().(
        * (None) -> Response { Response(notFound() * Headers() * Status(400)) }
        * (Some<String>) -> Response {
            String.(
                * ("/notes") -> Response { Response(indexBody() * Headers() * Status(200)) }
                * ("/notes/1") -> Response { Response(noteOneBody() * Headers() * Status(200)) }
                * (String) -> Response { Response(notFound() * Headers() * Status(404)) }
            )
        }
    )
}
```

```sh
$ canon run notes.can
$ curl localhost:8080/notes
[{"title":"ship canon v1"},{"title":"write the docs"}]
$ curl localhost:8080/notes/1
{"title":"ship canon v1"}
$ curl localhost:8080/other
{"error":"not found"}
```

Note the ordering everywhere: the functions (`indexBody`, `notFound`,
`noteOneBody`, `serve`) are alphabetical, and the literal route arms
(`"/notes"`, `"/notes/1"`) are too, with the catch-all last. Both are
enforced — and both are auto-fixed by `canon fmt`, so you never sort by
hand. This is the [ordering rule](../spec/ordering.md) that runs
through the whole language.

## The Smell We Just Introduced

`{"title":"ship canon v1"}` appears twice — once alone, once inside the
index array. The note *data* and the note *encoding* are tangled
together in literals, in the same file as the routing. And because the
literals are static, adding a third note means editing two places.

In most languages you'd now reach for a `Note` class and a serializer.
Canon's version of that move is a **newtype and a function in their own
module** — which is the [next chapter](./04-modules.md).
