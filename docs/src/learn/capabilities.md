# Capabilities

Canon has no effect annotations, no permission system, and no
`IO` monad. Effects emerge from an ordinary fact: **to do the thing,
you must hold the value** — and the only way to get the value is to
construct it.

```canon
Unit => Program {
    Path("./data.json")
        -> File?
        -> Read?
        -> Print
}
```

Constructing the `File` *is* opening it. You cannot read something that
is not a `File`, and you cannot conjure a `File` from anywhere but a
`Path`. The construction chain is the access control. (This program
touches the filesystem, so it has no run button — try it locally with
`canon run`.)

## Signatures Tell the Truth

A function that performs an effect takes the effectful value as an
input, so its signature *is* its effect declaration:

```canon
Database * User => Result<Saved, DbError> {
    …
}
```

No `UserRepository`, no dependency-injection framework, no globals: the
`Database` arrives as an argument or the function cannot exist — and it
is always written, never filled in from surrounding scope. `Print` is
the single deliberate exception: writing a line to stdout requires no
token.

## Evidence

Effects can also *produce* values — receipts that downstream code can
demand:

```canon
Written = Path

Contents * Path => Result<Written, IoError> {
    …
}
```

A write returns a `Written`. A function that takes `(Written)` instead
of `(Path)` now **requires proof the write happened** before it will
run — "do A before B" enforced by the type system, with no ordering
machinery. This is the same result-newtype idea from
[Pipes & Constructors](./pipes-and-constructors.md), pointed at the
outside world.

## The Sandbox Underneath

The discipline survives compilation. A Canon program is a WebAssembly
component whose only powers are the WASI interfaces its host chooses to
satisfy — there is no ambient authority to escalate into. What the
types promise in the source, the Component Model enforces in the
artifact.

**Precise rules:** [Effects and the Async Model](../spec/effects-and-async.md).

**Next:** [Async](./async-without-keywords.md) — suspension without
syntax.
