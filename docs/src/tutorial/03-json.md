# JSON, Without a Framework

A notes API should serve notes, plural, as JSON. The service grows its
real routes: `/notes` returns a JSON array, `/notes/1` returns a single
note, everything else gets a JSON error.

## JSON Is a Literal

JSON is part of Canon's syntax. An object or array literal is an
ordinary expression, and it evaluates to the encoded JSON text: a
`String`-shaped value holding exactly the bytes the client will
receive:

```canon
() => NotFound {
    Body({"error":"not found"})
}
```

No serializer, no schema, no middleware, no escaped quotes. The
literal *is* the wire format. Arrays work the same way, and literals
nest:

```canon
() => IndexBody {
    Body([{"title":"ship canon v1"},{"title":"write the docs"}])
}
```

> The spacing matters: `{"error":"not found"}`, not `{"error": "not
> found"}`. JSON literals are code, so `canon fmt` owns their layout
> like everything else's: compact, no spaces after `:` or `,`.

There's nothing to import: JSON is part of the prelude, like `Option`
and `Result`. A static literal compiles down to a plain string. When a
program reaches for the JSON *machinery* (the validating `Json(...)`
constructor, or **interpolation** like `{"answer":Int.mul(2)}`, which
converts values through their `ToJson` instance) the compiler pulls in
`canon/std/Json` automatically.

One limitation: the `ToJson` instances are currently backed by host
functions that the `wasi:http/service` world can't satisfy, so
interpolation is **CLI-only for now**. Use it in a handler and the
build fails with an error naming exactly which imports the HTTP world
can't provide. Inside a handler, keep literals fully static.

## The Full Program

```canon
IndexBody = Body

NotFound = Body

NoteOneBody = Body

() => IndexBody {
    Body([{"title":"ship canon v1"},{"title":"write the docs"}])
}

() => NotFound {
    Body({"error":"not found"})
}

() => NoteOneBody {
    Body({"title":"ship canon v1"})
}

Request => Response {
    Request.path().(
        * (None) => Response { Response(NotFound() * Headers() * Status(400)) }
        * (Some<String>) => Response {
            String.(
                * ("/notes") => Response { Response(IndexBody() * Headers() * Status(200)) }
                * ("/notes/1") => Response { Response(NoteOneBody() * Headers() * Status(200)) }
                * (String) => Response { Response(NotFound() * Headers() * Status(404)) }
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

The ordering is everywhere: the newtype constructors (`IndexBody`,
`NotFound`, `NoteOneBody`) are alphabetical — and the anonymous entry
sorts in among them by its produced type, `Response` — and so are the
literal route arms (`"/notes"`, `"/notes/1"`), catch-all last. Both are
enforced, and both are auto-fixed by `canon fmt`, so you never sort by
hand. This is the [ordering rule](../spec/ordering.md) that runs
through the whole language.

## The Smell

`{"title":"ship canon v1"}` appears twice: once alone, once inside the
index array. The note *data* and the note *encoding* are tangled
together in literals, in the same file as the routing. Because the
literals are static, adding a third note means editing two places.

In most languages the next move is a `Note` class and a serializer.
Canon's version of that move is a newtype and a function in their own
module: the [next chapter](./04-modules.md).
