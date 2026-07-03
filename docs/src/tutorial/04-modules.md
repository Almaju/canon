# Growing Into Modules

One file was fine for three routes. Now we'll give the project a real
package layout and pull the note logic into its own module — meeting
Canon's module system, which is small enough to explain in one sentence:
*a file declares the type it's named after, and `use` imports it.*

## From File to Package

A package is a directory with a manifest and a `src/`:

```text
notes-api/
  canon.toml
  src/
    main.can
    note.can
```

`canon.toml` is minimal:

```toml
name    = "notes-api"
version = "0.1.0"
```

From now on, run the package instead of a file:

```sh
canon run notes-api            # or `canon run .` from inside it
```

## The `Note` Module

`src/note.can` — the file is named `note.can`, so it must declare a type
named `Note`:

```canon
Note = String

render = (Note) -> String {
    "{\"title\":\""
        .concat(Note)
        .concat("\"}")
}
```

Two ideas in eight lines:

- **`Note = String` is a newtype.** A `Note` is stored like a `String`
  but is a distinct type — a function expecting `Note` documents itself.
  Newtypes are Canon's answer to both naming and disambiguation; they're
  what you write where other languages write a comment, a variable name,
  or a wrapper class.
- **`render` is the serializer.** It builds the JSON encoding of one
  note by chaining `concat`. Note the receiver: inside the body, the
  note is referenced as `Note` — components are named by their types.

## The Entry, Rewritten

`src/main.can` imports the module and gets out of the data business:

```canon
use Note
use canon/std/http/Body
use canon/std/http/Headers
use canon/std/http/Request
use canon/std/http/Response
use canon/std/http/Status

indexBody = () -> Body {
    Body(List(Note("ship canon v1").render(), Note("write the docs").render()).toJsonArray())
}

notFound = () -> Body {
    Body("{\"error\":\"not found\"}")
}

noteOneBody = () -> Body {
    Body(Note("ship canon v1").render())
}

serve = (Request) -> Response {
    Request.path().(
        * (None) -> Response { Response(notFound(), Headers(), Status(400)) }
        * (Some<String>) -> Response {
            String.(
                * ("/notes") -> Response { Response(indexBody(), Headers(), Status(200)) }
                * ("/notes/1") -> Response { Response(noteOneBody(), Headers(), Status(200)) }
                * (String) -> Response { Response(notFound(), Headers(), Status(404)) }
            )
        }
    )
}
```

The behaviour is identical to chapter 3 — same routes, same bytes — but
the JSON encoding now lives in exactly one place, next to the type it
encodes:

```sh
$ canon run notes-api
$ curl localhost:8080/notes
[{"title":"ship canon v1"},{"title":"write the docs"}]
```

## What `use` Did

- `use Note` loads `note.can` from the same directory and brings in the
  `Note` type **with its methods** — that's why `main.can` can call
  `.render()` without importing it separately. One import per type; no
  wildcards; no `mod` declarations. A folder is a module.
- `use` lines, like everything else, are alphabetical. `Note` sorts
  before `canon/std/…` — the compiler will tell you if you get it
  wrong.
- Note the substitutability: `render` chains `.concat(Note)` where
  `concat` expects a `String` — a newtype flows into its underlying
  type without unwrapping. Same reason `Body(Note(…).render())` works.

The service is now shaped like a real project: data and encoding in a
module, one thin entry that routes and wraps. Which means the logic is
now *testable* — [next chapter](./05-testing.md).
