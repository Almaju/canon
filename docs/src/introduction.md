# The Oneway Programming Language

Oneway is a small, opinionated language that transpiles to Rust. It inherits
Rust's ownership model and zero-cost abstractions while presenting a much
smaller surface area to the programmer.

The guiding rule: **wherever ordering is discretionary, the compiler enforces
alphabetical order**. Components of product types, variants of unions, method
declarations, dispatch arms, imports — all alphabetical. Reordering is never a
meaningful change.

## What It Looks Like

```oneway
Bool = False | True

main = (Stdout) -> Noop {
    List(1, 2, 3)
        .map((Int) -> Int { Int.mul(2) })
        .length()
        .print(Stdout)
}
```

A few things to notice:

- There is **no `let`**, no local variables, no `if`/`else`, no comments.
- Every function is implemented on a type: `Type.name = (params) -> Ret { ... }`.
- The exception is `main`, the program's entry point.
- Branching is dispatch on a union (`.( )`).
- Side effects are passed in as **capabilities** (`Stdout`, `Filesystem`, …).
- Imports are file-based: `use Foo` imports the type declared in `foo.ow`.

## Status

Oneway is an **experimental design exploration**. The compiler exists,
examples run, and the design is stable enough to write about — but every
detail is subject to change.

The reference implementation lives in the same repository as this book. The
authoritative design spec is
[`DESIGN.md`](https://github.com/Almaju/oneway/blob/main/DESIGN.md).

## How to Read This Book

- **Getting Started** — install the toolchain and run your first program.
- **A Tour of Oneway** — every feature, one short chapter each.
- **Reference** — sort-order rules, operator table, Rust comparison.

The chapters are short on purpose. Read straight through, or skip to whatever
you need.
