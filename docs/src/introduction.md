# The Canon Programming Language

Canon is a small, maximally opinionated language that compiles directly to
**WebAssembly Components** targeting WASI Preview 3. Every program is a
portable `.wasm` file that runs on any Component Model host: no toolchain
at build time, no runtime of its own to ship.

A complete HTTP service:

```canon
Request => Response {
    Body("hello from canon") -> Response(Status(200) * Headers())
}
```

```sh
$ canon run service.can
HTTP handler detected: serving on http://127.0.0.1:8080 (override with `canon run … --addr <ip:port>`)
canon run --addr 127.0.0.1:8080: listening on http://127.0.0.1:8080

$ curl localhost:8080
hello from canon
```

No framework, no router registration, no port wiring, no `main`. The
compiler sees one function returning `Response`, so the program *is* an
HTTP service: it compiles to a standard `wasi:http/service` component
that any compliant host can serve.

## Three Ideas

**One way to do everything.** Wherever ordering is discretionary, the
compiler enforces alphabetical order: product fields, union variants,
function declarations, dispatch arms. There is no `if`/`else`
*and* `match`; there is dispatch. There is no `while` *and* `for` *and*
recursion; there are collection methods and recursion. Two programmers
writing the same program produce the same bytes.

**Types are the only names.** Canon has no local variables, no `let`, no
comments, no parameter names. A function's inputs are a product of types,
referenced in the body by their type names:

```canon
OtherUser * User => Ord {
    User.Birthday -> Compared(OtherUser.Birthday)
}
```

If code needs explaining, the fix is a better type, not a comment. Names
lie; types don't.

**Having a value is having the capability.** There are no service
singletons and no permission system. Reading a file requires a `File`
value, which you can only get from a `Path`, which you can only build
from a `String`. The type chain *is* the access control:

```canon
Unit => Program {
    Path("./data.json")
        -> File?
        -> Read?
        -> Print
}
```

## What It Looks Like

A CLI program, with branching (dispatch on a union, Canon's only
control-flow construct) and iteration (methods on collections):

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

- Functions are `name = (Components) => Return { body }`; the last
  expression is the return value.
- `Bool` is an ordinary union, `False + True`; dispatch applies the
  value to one handler arm per variant.
- Any component can be the dot-receiver at the call site
  (`a.compare(b)` and `b.compare(a)` are the same call).
- `T()` constructs a value; `value.Field` (no parens) reads a field.
- `?` propagates `Result` errors and `Option` absence.
- Async exists at the ABI, never in the source: no `async`, no
  `.await`. The compiler infers suspension and lifts the component
  accordingly.

## How to Read This Book

The book is split into sections; use the tabs at the top.

- **Getting Started**: install the toolchain, run your first program,
  and read [A Tour of Canon](./guide.md) -- the whole language on one
  page, an afternoon's read.
- **Specification**: the precise rules: lexical structure, the type
  algebra, ordering, the compilation model and ABI. The authoritative
  description of the language.
- **Examples**: a curated set of real programs from the repository --
  their source is pulled in live, so it always matches what compiles.
- **Reference**: the standard library, WASI interfaces, deployment.

## Status

Canon is an **experimental design exploration**. The compiler exists, the
examples run, and the design is stable enough to write about, but every
detail is subject to change. The reference implementation lives in the
same repository as this book; the [language specification](./spec/overview.md)
is the authoritative description of the language.

## On Authorship and AI

The language is human work. Its philosophy, type algebra, ordering
discipline, types-only doctrine, and capabilities-as-values model are the
author's own ideas — not AI-generated. AI was used, under supervision, as
an implementation aid for the *compiler*: a tool for turning
already-decided designs into Rust. Every design decision was made,
reviewed, and owned by a human. The core is handmade; the AI helped build
it, not conceive it.
