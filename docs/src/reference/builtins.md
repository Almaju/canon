# Builtins

A handful of operations are compiler vocabulary rather than stdlib
declarations: the ones that are wasm numerics, linear-memory layout, or
a host boundary (the [minimal-primitives
doctrine](../spec/types-only.md)). They have no prefix form — a builtin
is something a value is piped *into*, `x -> Sum(1)`, never `Sum(x, 1)` —
and because they are not declarations, the [generated API
reference](api/index.html) cannot list them. This page is that list.

Everything else — `And`/`Or`/`Not`, `Ne`/`Le`/`Gt`/`Ge`, `Uppercased`,
`Map`, `Set`, JSON, HTML — is ordinary Canon in the [standard
library](./stdlib.md), written over these.

---

## Effects

| Builtin | Receiver | Result |
|---|---|---|
| `Print` | `String`, `Int`, `Float`, `Bool` | `Unit` — writes the value and a newline to stdout |

```canon
Unit => Program {
    "hello" -> Print
    42 -> Print
    True() -> Print
}
```

## Numbers

| Builtin | Receiver | Argument | Result |
|---|---|---|---|
| `Sum` | `Int`, `Float` | same type | same type |
| `Difference` | `Int`, `Float` | same type | same type |
| `Product` | `Int`, `Float` | same type | same type |
| `Quotient` | `Int`, `Float` | same type | same type (`Int` truncates toward zero) |
| `Remainder` | `Int`, `Float` | same type | same type |
| `Eq` | `Int`, `Float`, `String` | same type | `Bool` |
| `Lt` | `Int`, `Float`, `String` | same type | `Bool` |

The receiver is the left operand: `10 -> Difference(3)` is `7`,
`10 -> Lt(3)` is `False()`. `Eq` and `Lt` are the two comparisons wasm
provides; the other four are one dispatch each in the stdlib
(`Ne`, `Le`, `Gt`, `Ge`), as are `And`, `Or`, `Not` on `Bool`.

```canon
Unit => Program {
    10
        -> Difference(3)
        -> Product(2)
        -> Print
    7
        -> Remainder(4)
        -> Eq(3)
        -> Print
    2.5
        -> Sum(0.25)
        -> Print
}
```

An `Int` newtype keeps its name through arithmetic (`Count -> Sum(1)`
is still a `Count`), which is what lets a constructor family keyed on
that type close a builtin chain.

## Strings

| Builtin | Argument | Result |
|---|---|---|
| `Joined` | `String` | the receiver followed by the argument |
| `Length` | — | `Int`, the byte length |
| `ByteAt` | `Int` (1-based) | `Int`, the byte value |
| `Substring` | `From * To` (1-based, inclusive) | `String` |

```canon
Unit => Program {
    `{"canonical" -> Substring(From(1) * To(5))}!` -> Print
    "abc"
        -> ByteAt(2)
        -> Print
    "abc"
        -> Length
        -> Print
}
```

A `Joined` chain with literal text in it is what a backtick [format
string](../tour/format-strings.md) desugars to; `canon check --fix`
folds one into the other, so `` `{Name}!` `` is the canonical spelling.
Conversion to `String` is construction, not a builtin: `42 -> String`,
`Byte(42) -> String`.

## Lists

`List<T>` holds values of any type — scalars, strings, products,
unions, other lists. Every operation keeps the element type it was
given.

| Builtin | Argument | Result |
|---|---|---|
| `Length` | — | `Int` |
| `First` | — | `Option<T>` |
| `At` | `Int` (1-based) | `Option<T>`; `None` out of range |
| `Skipped` | `Int` | everything after the first *n* elements |
| `Taken` | `Int` | the first *n* elements |
| `Reversed` | — | `List<T>` |
| `Sorted` | — | `List<T>` in ascending `Lt` order; `T` is `Int`, `Float`, or `String` |
| `Appended` | `T` | `List<T>` with the element at the end |
| `Joined` | `List<T>` | the receiver followed by the argument |
| `Mapped` | `(T) => U { … }` | `List<U>` |
| `Filtered` | `(T) => Bool { … }` | `List<T>` |
| `Folded` | `init * (Acc * T) => Acc { … }` | `Acc` |

`Mapped`, `Filtered`, and `Folded` take an inline lambda, and Canon
lambdas are non-capturing — the language has no local variables, so
the body sees only its parameters, each referenced by its type name.

```canon
Unit => Program {
    List(3 * 1 * 2) -> Reversed -> Mapped((Int) => Int { Int -> Product(10) }) -> At(1) -> (
        * None { "empty" -> Print }
        * Some<Int> { Int -> Print }
    )
}
```

`Folded` is the reduction. Its argument is the initial accumulator and
a lambda over a product of the accumulator and one element; the
lambda's return type names the accumulator, which is how the two
components are told apart. An accumulator of the same underlying type
as the elements is a newtype, exactly as a product with two `Int`
fields would need:

```canon
Total = Int

Unit => Program {
    List(1 * 2 * 3 * 4)
        -> Folded(Total(0) * (Int * Total) => Total { Total -> Sum(Int) })
        -> Print
}
```

`First` and `At` hand an element back through `Option<T>`, so it
reaches the value level through `?` or a `Some<T>` arm — including a
product's fields:

```canon
Name = String

Player = Name * Score

Players = List<Player>

Score = Int

Unit => Players {
    List(Name("ada") -> Player(Score(3)) * Name("bob") -> Player(Score(5)))
}

Unit => Result<Program, Unit> {
    Players() -> Filtered((Player) => Bool { Player.Score -> Gt(4) }) -> First?.Name -> Print
    Unit() -> Ok
}
```

`-> Json` on a `List<Json>` encodes it as a JSON array; see [the
stdlib page](./stdlib.md).

## Concurrency

| Builtin | Receiver | Argument | Result |
|---|---|---|---|
| `Parallel` | `Future<A>` | `Future<B>` | both results, as a list |
| `Race` | `Future<A>` | `Future<A>` | whichever finishes first |

Futures come from async bindings and never need naming — see
[Effects and Async](../spec/effects-and-async.md). Both combinators are
methods on the first future, `a -> Parallel(b)`; there is no bare call
form.
