# Programs & Modules

## References Are Imports

There is no import statement. A file is named after the type it
declares (`greeter.can` declares `Greeter`), a module is a folder, and
mentioning a name the file doesn't define loads the file that does —
from the project tree first, then generated bindings, vendored
dependencies, and the bundled standard library. That is why every
example on this site uses `Print`, `Map`, or `Json` with no preamble.

A name that resolves in two places is a hard error naming both
candidates; there is no shadowing and no precedence to learn. And
because structurally identical newtype declarations merge, shared
vocabulary needs no coordination: `map.can` and `set.can` both declare
`Length = Int`, and it is one type.

## Projects Are Directories

There is no manifest. A directory with `src/main.can` is a package
(named after the directory); a directory of packages is a workspace;
WIT files under `wit/` are external imports; vendored packages under
`deps/<ns>/<name>@<version>/` are dependencies, pinned by their
directory name.

```text
my-app/
  src/
    main.can       # entry point — this file makes the directory a package
    invoice.can    # declares Invoice; referenced, never imported
  build/           # compiler output
```

Everything is public. The one encapsulation boundary is the
[validated constructor](./types-and-values.md), which already guards
the only thing worth guarding — a type's invariants.

## Entry Points Are Signatures

Nothing is named `main`. A program is whatever its types say it is —
declare an arrow whose shape matches a **world**, and that arrow is the
entry:

| You write | You get |
|---|---|
| `Args => Exit` (or `Unit => Program`) | a CLI command (`wasi:cli/command`) |
| `Request => Response` | an HTTP service (`wasi:http/service`) |
| `Model => Html` + `Unit => Init` + `Model * Msg => Update` | a browser app (wasm + generated JS host) |

Exactly one arrow may return a world type — helpers return ordinary
values, so good layering is compiler-enforced. Entry *files* are found
the same way: no filename is special — `canon` scans `src/` for the
world-shaped declaration. The
[fullstack example](../examples/fullstack.md) runs one shared codebase
on both sides of the wire this way: one file declares the web triple,
another the HTTP entry, and one `canon run` serves both on one
address. Two details worth knowing early:
`Args` (`= List<String>`) is your argv, handed to the CLI entry the way
`Request` is handed to the HTTP handler; and a fallible entry is just a
`Result` return, so `?` works at the top level too.

**Precise rules:** [Modules & Packages](../spec/modules.md) and
[Functions & Traits](../spec/functions.md); commands in
[The canon CLI](../getting-started/building-and-running.md).

**Next:** [Testing](./testing.md) — a test framework with no
framework.
