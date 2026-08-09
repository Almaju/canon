# One Arrow, Three Worlds

You have now seen the whole language. What is left is how a file
becomes a program.

A **reference is an import**: mentioning a name the file doesn't define
loads the file that does — the project tree first, then bindings,
dependencies, and the bundled standard library. A name that resolves in
two places is a hard error, so there is no shadowing to learn. A
**directory is a package**, `wit/` its external imports, `deps/` its
dependencies. There is no manifest and no reserved filename.

And the **entry point is a signature** — exactly one arrow in a program
may return a world type:

| You write | You get |
|---|---|
| `Unit => Program` | a CLI command |
| `Request => Response` | an HTTP service |
| `Model => Html` + `Unit => Init` + `Model * Msg => Update` | a browser app |

The CLI entry takes nothing, because `wasi:cli/run.run` takes nothing:
the argument vector is *fetched*, not passed, so `Args()` reads `argv`
from any body that wants it. Next:
[install the compiler](../getting-started/installation.md), or read the
[examples](../examples/multifile.md).

```canon,run
Unit => Program {
    "one language, three worlds" -> Print
}
```
