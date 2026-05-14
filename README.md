# Oneway

Oneway is a new programming language. The reference implementation transpiles to Rust — Oneway inherits Rust's ownership model and zero-cost abstractions, while presenting a much smaller surface area to the programmer.

The guiding rule: wherever ordering is discretionary, the compiler enforces alphabetical order. Components of product types, variants of unions, method declarations, match arms, imports — all alphabetical. Ordering is never a meaningful change.

See [`DESIGN.md`](DESIGN.md) for the language specification.

---

## What It Looks Like

```
Bool = False | True

main = (Stdout) -> Noop {
    List(1, 2, 3)
        .map((Int) -> Int { Int.mul(2) })
        .length()
        .print(Stdout)
}
```

Every function is implemented on a type. There is no `let`, no `if`/`else`, no comments, no local variables. Branching is `match` on a union. Effects are passed in as capabilities (`Stdout`, `Filesystem`, …). Imports are file-based — `use Foo` imports the type declared in `foo.ow` from the current module folder.

---

## Repository Layout

| Path | Description |
|------|-------------|
| [`src/`](src/) | The `oneway` compiler (lexer, parser, checker, codegen) |
| [`examples/`](examples/) | Example `.ow` programs |
| [`editors/`](editors/) | Tree-sitter grammar and Zed extension |
| [`DESIGN.md`](DESIGN.md) | Language specification |

---

## Quick Start

```sh
just run examples/hello.ow      # compile and run a program
just example list               # run examples/list.ow (or examples/list/main.ow)
just examples                   # run every example
just emit examples/hello.ow     # print generated Rust
just ast  examples/hello.ow     # print the AST
just build                      # build the compiler
just test                       # run the test suite
```

---

## Status

Experimental. Phase 18 of the v2 rewrite — lambdas and `List<T>` with `map` / `length` / `first` are in. The compiler is far from complete; the design is the artifact.
