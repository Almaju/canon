# The Canon Programming Language

Canon is a small, opinionated language that compiles to a **WebAssembly
Component** targeting WASI Preview 3. Every program is a portable `.wasm`
file that runs on any Component Model host — no Rust toolchain required at
build or run time.

The guiding rule: **wherever ordering is discretionary, the compiler enforces
alphabetical order**. Components of product types, variants of unions, method
declarations, dispatch arms, imports — all alphabetical. Reordering is never a
meaningful change.

## What It Looks Like

```canon
Bool = False + True

main = () -> Unit {
    List(1, 2, 3)
        .map((Int) -> Int { Int.mul(2) })
        .length()
        .print()
}
```

A few things to notice:

- There is **no `let`**, no local variables, no `if`/`else`, no comments.
- Functions are declared as `name = (Type) -> Ret { ... }` — any component can be the dot-receiver at the call site.
- `main` takes no parameters and is lifted as `wasi:cli/run.run` in the emitted component.
- Branching is dispatch on a union (`.( )`).
- `.print` writes a `String` to stdout (lowered against `wasi:cli/stdout`). No capability token needed.
- `T()` constructs a value; `value.Field` (no parens) reads a field.
- Imports are file-based: `use Foo` imports the type declared in `foo.can`; `use std/Foo` pulls from the embedded stdlib.

## Domain-First Design

Canon has no service singletons. Instead of asking permission from a `Filesystem` object, you start with a real value and transform it:

```canon
use std/File
use std/Path

main = () -> Unit {
    Path("./data.json").File()?.read()?.print()
}
```

Having a `File` value *is* the capability to read that file. The type chain enforces access naturally. The same principle applies to HTTP, JSON, time — you start with what you concretely have and transform it toward what you need.

## Status

Canon is an **experimental design exploration**. The compiler exists,
examples run, and the design is stable enough to write about — but every
detail is subject to change.

The reference implementation lives in the same repository as this book. The
authoritative design spec is
[`DESIGN.md`](https://github.com/Almaju/canon/blob/main/DESIGN.md); the
WASM-backend status notes live in
[`WASM.md`](https://github.com/Almaju/canon/blob/main/WASM.md).

## How to Read This Book

- **Getting Started** — install the toolchain and run your first program.
- **A Tour of Canon** — every feature, one short chapter each.
- **Reference** — sort-order rules, operator table, Rust comparison.

The chapters are short on purpose. Read straight through, or skip to whatever
you need.
