# Dispatch

There is no `if`/`else` or `match` keyword. All branching is **dispatch**
on a union: the value is the receiver, and the arms go inside `.( )`.

## Basic Form

```canon,run=dispatch
() => Unit {
    True().(
        * (False) => Unit { "no" -> Print }
        * (True) => Unit { "yes" -> Print }
    )
}
```

`Bool` is core's union `False + True`, nothing special-cased; any union
of your own dispatches the same way.

Dispatch is an expression. It can be the final line of a function body or
appear as a sub-expression.

## Arm Order

Dispatch arms follow the union's variant order, which is itself
alphabetical. Every variant must be spelled out:

```canon
Ord = Equal
  + Greater
  + Less

classify = (Ord) => Sign {
    Ord.(
        * (Equal) => Sign { Zero() }
        * (Greater) => Sign { Positive() }
        * (Less) => Sign { Negative() }
    )
}
```

## Matching Constructors with Payloads

Each arm is written as `* (Pattern) => ArmReturnType { body }`. For
union variants that carry a payload, name the payload's type in angle
brackets; inside the arm body the value is referenced by that type
name:

```canon
List(7, 8, 9).first().(
    * (None) => Unit { "empty".print() }
    * (Some<Int>) => Unit { Int.print() }
)
```

## Literal Dispatch

Dispatch also works by **equality on `String` and `Int`** scrutinees:
arms may be literals, and the final arm is a mandatory catch-all naming
the scrutinee's type. Literal arms can never be exhaustive, so totality
comes from the catch-all:

```canon
route = (String) => String {
    String.(
        * ("/notes") => String { "index" }
        * ("/notes/1") => String { "note one" }
        * (String) => String { "not found: " -> Joined(String) }
    )
}
```

Literal arms follow canonical order (alphabetical for strings, ascending
for ints), and `canon fmt` sorts them for you. Inside every arm body the
scrutinee is in scope under its type name, like a bound payload. Newtype
scrutinees work too: a `Path = String` value dispatches with a `(Path)`
catch-all.

## Why No `if`?

`if cond then a else b` is a dispatch on `Bool`. Since you already need
dispatch for unions in general, a second branching construct would be
another way to do the same thing. So there is one.

## Why No `match` Keyword?

Algebraically, a function from a sum is a product of functions:

```
(A + B + C) -> R  ≅  (A -> R) * (B -> R) * (C -> R)
```

Dispatch makes this explicit: the scrutinee is the receiver, the arms are
the handlers. A value applied to its handlers needs no keyword.
