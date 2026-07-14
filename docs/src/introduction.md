# The Canon Programming Language

Canon is a small, radically opinionated language that compiles straight
to **WebAssembly**. It has no `if`, no loops, no local variables, no
imports, and no comments — and it is not missing any of them. What is
left is a language where **types do everything**: they name your values,
route your logic, guard your effects, and even decide what kind of
program you are writing.

A complete HTTP service:

```canon
Request => Response {
    Body("hello from canon") -> Response(Status(200) * Headers())
}
```

```sh
$ canon run service.can
HTTP handler detected: serving on http://127.0.0.1:8080

$ curl localhost:8080
hello from canon
```

No framework, no router registration, no port wiring, no `main`. The
compiler sees one arrow returning `Response`, so the program *is* an
HTTP service: a standard `wasi:http/service` component any compliant
host can serve.

Canon also compiles to the browser — which is why the next program has
a **run** button. Press it.

```canon,run=intro
Unit => Program {
    List(1 * 2 * 3)
        -> Mapped((Int) => Int { Int -> Product(2) })
        -> Length
        -> Print
    True() -> (
        * False => Unit { "no" -> Print }
        * True => Unit { "yes" -> Print }
    )
}
```

## Why Canon?

Four commitments explain nearly every design decision. Each gets one
sentence here and a full chapter in [The Philosophy](./philosophy.md).

- **One way to do everything.** Wherever a choice is discretionary —
  ordering, formatting, call spelling — the compiler picks the answer,
  so two programmers writing the same program produce the same bytes.
- **Types are the only names.** No variables, no parameter names, no
  function names: every name in a Canon program is a type, and every
  operation is named after what it produces.
- **Having a value is having the capability.** No permission system and
  no ambient authority: reading a file requires holding a `File`, and
  the type chain that produces one *is* the access control.
- **The artifact is a standard.** Every program compiles to a
  WebAssembly component (or a browser bundle) with no runtime of its
  own to ship — it runs on any Component Model host, sandboxed by
  construction.

## Where to Go Next

- **Skeptical?** [Is Canon for You?](./reference/coming-from.md) maps
  Canon onto the language you already know.
- **Want the ideas?** [The Philosophy](./philosophy.md) is the full
  argument.
- **Want to write code?** [Install](./getting-started/installation.md),
  say [Hello, World](./getting-started/hello-world.md), then walk the
  **Learn** chapters from [Types & Values](./learn/types-and-values.md)
  on — they run in the browser.
- **Looking for something specific?** [How Do I…?](./learn/how-do-i.md).
- **Need the exact rules?** The [Specification](./spec/overview.md).
- **Generating code with a model?** [Canon for AI](./canon-for-ai.md).

## Status

Canon is an **experimental design exploration**. The compiler exists,
the examples run, and the design is stable enough to write about, but
every detail is subject to change. The reference implementation lives in
the same repository as this book.

## On Authorship and AI

The language is human work. Its philosophy, type algebra, ordering
discipline, types-only doctrine, and capabilities-as-values model are the
author's own ideas — not AI-generated. AI was used, under supervision, as
an implementation aid for the *compiler*: a tool for turning
already-decided designs into Rust. Every design decision was made,
reviewed, and owned by a human. The core is handmade; the AI helped build
it, not conceive it.
