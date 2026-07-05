# Growing Into Modules

One file was fine for three routes. The next step is a real package
layout, with the note logic in its own module. Canon's module system
fits in one sentence: *a file declares the type it's named after, and
referencing the type loads the file.*

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

The file is named `note.can`, so it must declare a type named `Note`:

```canon
Note = String

render = (Note) => String {
    "{\"title\":\""
        -> Joined(Note)
        -> Joined("\"}")
}
```

Two ideas in eight lines:

- `Note = String` declares a newtype. A `Note` is stored like a
  `String` but is a distinct type; a function expecting `Note`
  documents itself. Newtypes are Canon's answer to both naming and
  disambiguation: what you write where other languages write a
  comment, a variable name, or a wrapper class.
- `render` is the serializer. It builds the JSON encoding of one note
  by chaining `concat`. A JSON *literal* can't do this job yet:
  interpolating a value (`{"title":Note}`) rides `canon/std/Json`'s
  host-backed `ToJson`, which the HTTP world can't satisfy, so dynamic
  encoding in a handler is honest string concatenation, escapes and
  all. Inside the body the note is referenced as `Note`: parameters go
  by their type name.

## The Entry, Rewritten

`src/main.can` refers to the module and gets out of the data business:

```canon
indexBody = () => Body {
    Body(List(Note("ship canon v1").render(), Note("write the docs").render()) -> Json)
}

notFound = () => Body {
    Body({"error":"not found"})
}

noteOneBody = () => Body {
    Body(Note("ship canon v1").render())
}

serve = (Request) => Response {
    Request.path().(
        * (None) => Response { Response(notFound() * Headers() * Status(400)) }
        * (Some<String>) => Response {
            String.(
                * ("/notes") => Response { Response(indexBody() * Headers() * Status(200)) }
                * ("/notes/1") => Response { Response(noteOneBody() * Headers() * Status(200)) }
                * (String) => Response { Response(notFound() * Headers() * Status(404)) }
            )
        }
    )
}
```

`indexBody` composes the array dynamically now:
`List<String>.Json()` is a compiler builtin that joins
already-encoded JSON fragments into an array, so the static array
literal from chapter 3 gives way to encoding each `Note` once. The
behaviour is identical (same routes, same bytes), but the JSON
encoding now lives in exactly one place, next to the type it encodes:

```sh
$ canon run notes-api
$ curl localhost:8080/notes
[{"title":"ship canon v1"},{"title":"write the docs"}]
```

## Where `Note` Came From

- There is no import statement. `main.can` mentions `Note`, doesn't
  define it, and the loader resolves the reference by convention: a
  file named `note.can` in the project declares `Note`. The type
  arrives **with its methods**, which is why `main.can` can call
  `.render()` without any ceremony. No import lines; no wildcards; no
  `mod` declarations. A folder is a module.
- The same rule fetched `Request`, `Response`, and the rest: a name
  not found in the project's files is looked up in `bindgen/`, then in
  vendored `deps/`, then in the standard library. If a name resolves
  in more than one place, the build fails naming every candidate —
  names are globally unique across a project, its deps, and the
  stdlib, so there is never shadowing to reason about.
- `render` chains `.concat(Note)` where `concat` expects a `String`: a
  newtype flows into its underlying type without unwrapping. The same
  substitutability is why `Body(Note(…).render())` works.

The service is now shaped like a real project: data and encoding in a
module, one thin entry that routes and wraps. The logic is now
testable, which is the [next chapter](./05-testing.md).
