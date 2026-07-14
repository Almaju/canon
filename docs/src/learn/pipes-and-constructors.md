# Pipes & Constructors

Three symbols carry the whole language, one job each:

| Symbol | Job |
|---|---|
| `=>` | **declares** — every constructor, lambda, and dispatch arm |
| `->` | **executes** — pipes a value through an operation |
| `.` | **reads** — field access, nothing else |

## Declaring

Every callable is a constructor: `Input => Output { body }`, named
after the type it produces — so it needs no name of its own. The input
is a product of types; the body is a newline-separated sequence of
expressions whose **last expression is the return value**. There are no
semicolons, no `return`, and no local variables: values thread through
the pipe.

```canon
Greeting * Name => Line {
    Greeting -> Joined(Name)
}
```

`Unit` is the name of "no input", so a nullary constructor is
`Unit => X` and its call site is `X()`.

## Executing

At the call site, a value flows left to right through `->`:

```canon,run=learn-pipes
Loud = String

Whisper = String

Whisper => Loud {
    Whisper
        -> Uppercased
        -> Joined("!")
}

Unit => Program {
    Whisper("keep it down")
        -> Loud
        -> Print
    `two plus three is {2 -> Sum(3)}` -> Print
}
```

The backtick string on the last line is a **format string**: `{…}`
holes hold any expression, converted to text and spliced in. Ordinary
`"…"` strings have no holes.

## Calls Are Commutative

A multi-input call can be entered from *any* of its components — the
piped value fills one slot of the input product, the rest ride in the
parens:

```canon
Greeting("hi ") -> Line(Name("ada"))
Name("ada") -> Line(Greeting("hi "))
```

Both are the same call: because the input is a product of **distinct
types**, each argument binds to the slot its type selects, not to a
position — a function is never bound to a single "receiver". One
canonical spelling is enforced, like all formatting: **values flow
through pipes, literals are born in the parens** — `value -> Person(30)`
but `Greeting("hi")`, never `"hi" -> Greeting`.

## Result Newtypes

What do you call an operation that takes a `Map` and returns a `Map`?
In Canon, nothing — functions have no names. Instead the operation
returns a **result newtype** named after what it did:

```canon
Inserted = Map

Map * String * Value => Inserted {
    …
}
```

`map -> Inserted("k" * "v")` reads as what it is, and because a newtype
flows anywhere its base type is expected, chaining is free:
`Map() -> Inserted("a" * "1") -> Removed("a")`. The same idea gives
shared vocabulary its shape — every container declares the same
`Length = Int` and contributes its own arrow to it.

## Lambdas

A one-off operation is the same arrow, written inline with its full
signature:

```canon
List(1 * 2 * 3) -> Mapped((Int) => Int { Int -> Product(2) })
```

There is exactly one function form in the language — top level it
declares a constructor, in expression position it is a lambda, and (as
the next chapter shows) every dispatch arm is one too.

**Precise rules:** [Functions & Traits](../spec/functions.md) and
[Expressions & Dispatch](../spec/expressions.md).

**Next:** [Branching & Loops](./branching-and-loops.md) — the one way
to make a decision.
