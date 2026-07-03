# JSON, Without a Framework

A notes API should serve notes, plural, as JSON. In this chapter the
service grows its real routes: `/notes` returns a JSON array, `/notes/1`
returns a single note, everything else gets a JSON error.

## JSON Bodies Are Strings

In an HTTP handler, a JSON body is exactly what it is on the wire: a
`String` with escaped quotes.

```canon
notFound = () -> Body {
    Body("{\"error\":\"not found\"}")
}
```

No serializer, no schema, no middleware — you are looking at the bytes
the client will receive.

> The stdlib does have a JSON module — `canon/std/Json` gives you a
> validating `Json` constructor, JSON literal syntax like
> `{"answer": Int}`, and a `ToJson` trait — but it's currently backed by
> host functions that the `wasi:http/service` world can't satisfy, so it
> is **CLI-only for now**. Import it in a handler program and the
> compiler tells you exactly that. Plain strings are today's honest
> answer for HTTP, and they carry us surprisingly far.

## Composing an Array

The index route needs `[note, note]`. `List<String>.toJsonArray()` joins
already-encoded JSON fragments into an array — it's a compiler builtin,
so it works fine inside a handler:

```canon
List("{\"title\":\"ship canon v1\"}", "{\"title\":\"write the docs\"}")
    .toJsonArray()
```

produces:

```json
[{"title":"ship canon v1"},{"title":"write the docs"}]
```

## The Full Program

```canon
use canon/std/http/Body
use canon/std/http/Headers
use canon/std/http/Request
use canon/std/http/Response
use canon/std/http/Status

indexBody = () -> Body {
    Body(List("{\"title\":\"ship canon v1\"}", "{\"title\":\"write the docs\"}").toJsonArray())
}

notFound = () -> Body {
    Body("{\"error\":\"not found\"}")
}

noteOneBody = () -> Body {
    Body("{\"title\":\"ship canon v1\"}")
}

serve = (Request) -> Response {
    Request.path().(
        * (None) -> Response { Response(notFound(), Headers(), Status(400)) }
        * (Some<String>) -> Response { String.eq("/notes").( * (False) -> Response { String.eq("/notes/1").( * (False) -> Response { Response(notFound(), Headers(), Status(404)) } * (True) -> Response { Response(noteOneBody(), Headers(), Status(200)) }) } * (True) -> Response { Response(indexBody(), Headers(), Status(200)) }) }
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

Note the declaration order: `indexBody`, `notFound`, `noteOneBody`,
`serve` — functions in a file must be alphabetical, and the compiler
enforces it. (Reorder two of them and see.) This is the
[ordering rule](../spec/ordering.md) that runs through the whole
language.

## The Smell We Just Introduced

`"{\"title\":\"ship canon v1\"}"` appears twice — once alone, once
inside the index list. The note *data* and the note *encoding* are
tangled together in string literals, in the same file as the routing.

In most languages you'd now reach for a `Note` class and a serializer.
Canon's version of that move is a **newtype and a function in their own
module** — which is the [next chapter](./04-modules.md).
