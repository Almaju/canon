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

```canon,run
Unit => Program {
    List(1 * 2 * 3)
        -> Mapped((Int) => Int { Int -> Product(2) })
        -> Length
        -> Print
    True() -> (
        * False { "no" -> Print }
        * True { "yes" -> Print }
    )
}
```

Four commitments explain nearly every design decision — one way to do
everything, types as the only names, having a value is having the
capability, and the artifact is a standard. [The
Philosophy](./philosophy.md) takes each in turn.

Sixteen steps of the [Tour](./tour/hello.md) teach the rest, each one
running in this tab.

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
