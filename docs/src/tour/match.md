# Dispatch

There is no `if`/`else` or `match` keyword. All branching is **dispatch** on
a union — the value is the receiver, and the arms go inside `.( )`.

## Basic Form

```oneway
Bool = False + True

main = (Stdout) -> Unit {
    True.(
        False => "no".print(Stdout),
        True  => "yes".print(Stdout),
    )
}
```

Dispatch is an expression. It can be the final line of a function body or
appear as a sub-expression.

## Arm Order

Dispatch arms follow the union's variant order — which is itself
alphabetical. Every variant must be spelled out:

```oneway
Ord = Equal + Greater + Less

Int.classify = () -> Sign {
    Int.compare(Int(0)).(
        Equal   => Zero,
        Greater => Positive,
        Less    => Negative,
    )
}
```

## Matching Constructors with Payloads

For union variants that carry a payload, bind it with parentheses. Use
`_` inside the parens to ignore the payload:

```oneway
List(7, 8, 9).first().(
    None    => "empty".print(Stdout),
    Some(_) => "non-empty".print(Stdout),
)
```

## Why No `if`?

`if cond then a else b` is a dispatch on `Bool`. Since you already need
dispatch for unions in general, a second branching construct would just be
another way to do the same thing. So there is one.

## Why No `match` Keyword?

Algebraically, a function from a sum is a product of functions:

```
(A + B + C) -> R  ≅  (A -> R) * (B -> R) * (C -> R)
```

Dispatch makes this explicit: the scrutinee is the receiver, the arms are
the handlers. No keyword needed — just a value applied to its handlers.
