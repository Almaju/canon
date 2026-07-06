# Coming from TypeScript

TypeScript bolts a structural type system onto JavaScript's runtime.
Canon goes the other way: the types come first, and there is no
JavaScript underneath — programs compile straight to WebAssembly
Components (WASI Preview 3). If you write TypeScript, the biggest
adjustments are that **there are no local variables, no `if`/`else`,
and no comments**, and that a union is branched by *dispatch* rather
than by narrowing. In exchange you get exhaustiveness, canonical
formatting, and effects you can see in a signature.

Canon's nearest mainstream cousin is **Elm**, not JavaScript: pure
functions, tagged unions, a pipe operator, and a managed effect model.
This page maps the TypeScript you know onto that world.

## Cheat Sheet

| TypeScript                                        | Canon                                                             |
|---------------------------------------------------|------------------------------------------------------------------|
| `interface User { birthday: ...; username: ... }` | `User = Birthday * Username`                                     |
| `type Shape = Circle \| Square`                   | `Shape = Circle + Square`                                        |
| `type Bool = false \| true`                       | `Bool = False + True`                                            |
| `type Name = string` (or a branded `string`)      | `Name = String` (a *distinct* newtype, not an alias)            |
| `type List<T> = T[]`                              | `List<T>` (built-in; `= T^*`)                                   |
| `class User { greet(): string { ... } }`          | `Greet = (User) => String { ... }`                              |
| top-level script / `main()`                       | `Unit => Program { ... }` (entry is chosen by signature)        |
| `process.argv` / `process.exit(n)`               | the entry's `Args` (`= List<String>`) and `Exit` (`= Int`)      |
| `interface Show { show(): string }`               | `Show = Unit => String`                                         |
| `class User implements Show { ... }`              | `Show = (User) => String { ... }`                               |
| `Result`-style `{ ok } \| { err }` unions         | `Result<T, E>` (built-in; inline union for `E`)                |
| `T \| null` / `T \| undefined` / `foo?: T`        | `Option<T>` (absent) — distinct from `Result<T, E>` (failed)    |
| `x?.y` / `x ?? y`                                 | `?` operator (propagates absence/failure)                       |
| `switch (shape) { case ...: }`                    | `shape -> ( * Circle => R { ... } * Square => R { ... } )`      |
| `if (cond) { a } else { b }`                      | `cond -> ( * False => R { b } * True => R { a } )`             |
| `const x = expr;`                                 | No equivalent; newtype an intermediate value                    |
| `let` / `var` / reassignment                      | No mutation; method chaining and recursion                      |
| `export`                                          | Everything is public                                            |
| `import { Foo } from "./foo"`                     | Nothing -- referencing `Foo` loads `foo.can`                    |
| `import { z } from "zod"` (third-party)           | Nothing -- stdlib and `deps/` names resolve by reference        |
| `(x: T) => U` (function type)                     | `(T) => U` (also the way a trait/interface is declared)         |
| `async` / `await`                                 | Inferred; no source-level keyword                               |
| `String(x)` / `x.toString()` / `` `${x}` ``       | `String(x)` / `x.String()` -- conversion is construction        |
| `Number(s)` / `parseInt(s)`                       | `Int(s)?` / `s.Int()?` (fallible, so `?` is forced)             |
| `JSON.parse(s)` / `JSON.stringify(x)`             | `Json(s)?` / `x.ToJson()` (`Json` is a prelude type)           |
| `` `<div>${x}</div>` `` (template HTML)           | `<div>{x}</div>` (`Html` literal; auto-escapes strings/ints)   |
| `new Map()` + `.set(k, v)`                        | `Map().Inserted(k, v)` (stdlib; sorted, immutable)             |
| `new Set()` + `.add(x)`                           | `Set().Inserted(x)` (stdlib)                                    |
| `arr.map(f)` / `arr.filter(p)`                    | `arr.Mapped(f)` / method chains on `List<T>`                    |
| `//` and `/* */` comments                         | None -- names and types carry the meaning                       |

## Where TypeScript Is More Forgiving

These are Canon's trade-offs — the things you give up:

- **Structural (duck) typing.** Canon is nominal: two records with the
  same fields are different types unless one is declared a newtype of
  the other. There is no `{ x: number }`-shaped compatibility.
- **`any` / `unknown` escape hatches.** There is no gradual-typing back
  door and no `as` cast. Every value has a precise type.
- **Type inference of signatures.** TypeScript infers most types for
  you; Canon asks you to write them. Every function, lambda, and
  dispatch arm states its types explicitly.
- **`null` and `undefined`.** Absence is modeled with `Option<T>`, and
  it must be handled — there is no billion-dollar mistake to inherit.
- **Local variables, mutation, loops.** No `let`, no `for`/`while`, no
  reassignment. You express iteration with recursion and collection
  methods, and name intermediate values by giving them a newtype.
- **`if`/`else` and `switch` fall-through.** Branching is dispatch on a
  union; it is exhaustive and has no wildcard (literal `String`/`Int`
  dispatch requires an explicit catch-all instead).
- **Comments.** The compiler rejects them. If code needs explaining,
  the intended fix is a clearer type or name.
- **Ecosystem.** npm is enormous; Canon's standard library and package
  set are young. You reach the host through WASI/WIT bindings, not
  Node APIs.

## What Canon Gives You in Return

The strengths — things TypeScript can't offer, or only by convention:

- **Exhaustiveness with no wildcard.** Add a variant to a union and
  every dispatch that forgot to handle it fails to compile. No
  `default:` can silently swallow the new case.
- **One canonical spelling.** Wherever ordering is discretionary — union
  variants, product fields, arguments, declarations, dispatch arms — the
  compiler enforces alphabetical order. `canon fmt` sorts; two authors
  writing the same program produce identical bytes. There is no Prettier
  config to argue about because there is nothing to configure.
- **Effects you can see.** Performing I/O means *holding a value*
  (`File`, `Url`, `Database`) that only a concrete input can produce.
  The capability shows up in the signature, so a function's type tells
  you what it can touch. No ambient `fetch`, no hidden globals.
- **Sound types, no runtime holes.** No `any`, no unsound casts, no
  `strictNullChecks` to remember to turn on — the guarantees are always
  on.
- **`async` with no color.** There is no `async`/`await` keyword and no
  red/blue function split; suspension is inferred and `Future<T>` /
  `Stream<T>` appear only at binding boundaries. Concurrency is
  combinator methods (`.parallel`, `.race`).
- **Dead code is an error.** Anything unreachable from the entry point
  is rejected, so the program you ship has no orphaned branches.
- **Portable artifact.** `canon build` emits a `.wasm` Component that
  runs on any WASI Preview 3 host — no bundler, no `tsc`, no Node
  runtime, no `node_modules`.

## A Small Example

TypeScript:

```typescript
type Shape = { kind: "circle"; r: number } | { kind: "square"; s: number };

function area(shape: Shape): number {
  switch (shape.kind) {
    case "circle": return 3.14 * shape.r * shape.r;
    case "square": return shape.s * shape.s;
  }
}
```

Canon — the union is two named types, and `area` dispatches on it
exhaustively (no `default`, and the arms are in alphabetical order):

```canon
Shape = Circle + Square

Circle = Radius

Square = Side

Shape => Area {
    Shape -> (
        * Circle => Area { Circle -> Squared -> Product(3.14) }
        * Square => Area { Square -> Squared }
    )
}
```

Adding a `Triangle` variant to `Shape` makes `area` stop compiling until
you handle it — the exhaustiveness you'd get in TypeScript only by
carefully typing the `switch` and enabling the right lint.

## When in Doubt

- Skim [A Tour of Canon](../guide.md) for the language end to end.
- The [Types](../spec/types.md) and
  [Expressions & Dispatch](../spec/expressions.md) spec pages cover the
  union/product algebra and how dispatch replaces `switch`.
- [Effects & Async](../spec/effects-and-async.md) explains capabilities
  and the keyword-free async model.
- Look at the [`examples/`](https://github.com/Almaju/canon/tree/main/examples)
  directory — the browser examples use the same Elm-style triple you may
  know from front-end work.
