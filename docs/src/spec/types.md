# Types

Every Canon type is built by composing a small algebra over a minimal
core. The algebra has three operators — `+` (union), `*` (product),
`^` (repetition) — and two identities.

## The Core

- **`Unit`** — the type with exactly one value; the multiplicative
  identity (`T * Unit ≡ T`).
- **`Never`** — the type with zero values; the additive identity
  (`T + Never ≡ T`).

Together with `+` and `*` these form a type **semiring**. The boolean
atoms are newtype aliases of `Unit`, and everything scales up from
there:

```canon
Bit = False + True

Bool = False + True

Byte = Bit^8

Bytes = Byte^*

False = Unit

True = Unit
```

Higher-level primitives — `Int`, `Float`, `Hex`, `String` — are defined
from `Byte`/`Bytes`; a small set of built-in operations on them (e.g.
integer arithmetic) is supplied by the compiler, but their *shape* is
still described by the algebra.

## Unions (`+`)

`A + B` is a value of `A` **or** `B`:

```canon
Ord = Equal
  + Greater
  + Less
```

Variants must be listed in [alphabetical order](./ordering.md). There is
no `enum` keyword. Branching on a union is [dispatch](./expressions.md#dispatch).

## Products (`*`)

`A * B` is a value with an `A` **and** a `B`:

```canon
User = Birthday * Username
```

- Components must be in alphabetical order.
- Components must be **distinct types** — `(User * User)` is a compile
  error. Disambiguate with a newtype (`OtherUser = User`).
- A component is accessed by its type name: `user.Birthday`.
- For repeated or anonymous components (from `^N`), access is by
  1-based position: `byte.1`, `byte.2`.

## Newtypes

`A = B` (single named type on the right) declares a **newtype**: a
distinct type that wraps `B`. Formally it is a 1-component product, and
the field-access rule applies uniformly:

```canon
Greeting = String

Greeting("hi").String
```

Rules:

- **Method inheritance.** Methods declared on `B` are callable on `A`
  through the alias chain (`Greeting("hi").print()`). Methods declared
  on `A` shadow inherited ones.
- **Substitutability.** A value of `A` may be passed where `B` is
  expected, without unwrapping. The reverse also holds at construction:
  `A(b)` wraps a `B`.
- **Distinctness.** For product-membership and disambiguation purposes,
  `A` and `B` are different types.
- **Multi-step chains** unwrap one step at a time: with `A = B` and
  `B = C`, reach the bottom via `aValue.B.C`.

## Repetition (`^N`, `^*`)

`T^N` is the N-fold product `T * T * … * T`; `T^*` is the Kleene star —
zero or more `T`s. Both share the `^` operator and complete the
semiring reading: sums, products, exponents.

`List<T>` is not a separate concept: core defines **`List<T> = T^*`** —
the nominal name and the algebraic form are the same type. `Bytes =
Byte^*` therefore has every `List` method (`map`, `first`, `get`, …)
with nothing to declare, and `List(…)` is simply the value-level
constructor for the star. Indexing is **1-based** everywhere
(`list.get(1)` is the first element, `byteAt(1)` the first byte) — one
origin, matching positional product access `.1`.

## Generics

Types may be parameterized with angle brackets — `List<T>`,
`Option<T>`, `Result<T, E>`, `Map<String, Int>`. Constraints name a
trait after `:`:

```canon
showAll = <T: Show>(List<T>) -> Unit {
    ...
}
```

Where a type parameter cannot be inferred from context, the call site
pins it with turbofish: `parse::<List<Int>>(…)`. See
[Functions and Traits](./functions.md).

## Recursive Types

Recursive definitions are legal and **boxed automatically**:

```canon
Branch = Left * Right * Value

Left = Tree

Right = Tree

Tree = Branch + Leaf

Value = Int
```

There is no user-visible `Box<T>`; the compiler chooses the indirection.

## Validated Constructors

By default every type `T` has a total constructor `T(inner)`. A file may
replace it by declaring a **function with the type's own name**:

```canon
Url = String

Url = (String) -> Result<Url, InvalidUrl> {
    ...
}
```

- If a constructor is declared, it *is* the constructor; the implicit
  total one is gone.
- The signature is unconstrained: total (`-> Url`), fallible
  (`-> Result<Url, E>`), or optional (`-> Option<Url>`).
- Call sites keep ordinary constructor syntax — `Url("…")` — but the
  expression's type is the constructor's return type, so a fallible
  constructor forces `?` or dispatch at every use.
- External callers cannot bypass it: only functions declared in the same
  file as the type may touch the raw inner representation. This is the
  language's entire encapsulation story — see
  [visibility](./modules.md#visibility).

## Zero-Data Types

A type with no underlying data (`Unit`, `True`, `False`, a payload-less
variant) has exactly one value, produced with an empty argument list:
`True()`, `None()`. Calling a **data-carrying** constructor with no
arguments (`String()`, `User()`) is a compile error — absence belongs in
`Option<T>`, not in a default value. Factory-style construction uses an
explicit lowercase function (`List.empty()`).

## No Type Inference

Every type is written explicitly: function signatures, lambda
signatures, dispatch arm types. Declared shape and checked shape must
match exactly.

## Dead Code

A **program's** declarations must be reachable from its entry point:
`canon check` walks the reference graph from `main` (or the HTTP
handler) and warns on every unreachable type and function —

```
warning: `unused` is never used — dead code is not allowed to
accumulate; delete it or wire it into the program
```

— a warning at the command line, promoted to a failure in CI.
Libraries are exempt: with no private visibility, every declaration in
a library *is* exported surface, so its dead code shows up downstream,
in the programs that stopped calling it.
