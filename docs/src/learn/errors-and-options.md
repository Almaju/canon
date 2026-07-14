# Errors & Options

Failure and absence are values, and they are **different** values:
`Result<T, E>` means the operation *failed* with an `E`;
`Option<T>` means the thing was *not there*. Keeping them apart is how
Canon avoids `null` and exceptions in one move.

## The `?` Operator

Postfix `?` propagates: on a `Result` it short-circuits the error out
of the enclosing function, on an `Option` it short-circuits the
`None` — otherwise it unwraps. Pipelines read top-down, and every
possible failure is visible as a `?` at the exact call that can fail:

```canon
Unit => Program {
    Path("./data.json")
        -> File?
        -> Read?
        -> Print
}
```

To *handle* a failure instead of propagating it, dispatch — `Result`
and `Option` are ordinary unions:

```canon,run=learn-errors
Unit => Result<Program, MalformedInt> {
    Int("42")?
        -> Sum(8)
        -> Print
    Int("4x") -> (
        * Err<MalformedInt> => Unit { "not a number: " -> Joined(MalformedInt) -> Print }
        * Ok<Int> => Unit { Int -> Print }
    )
    Unit() -> Ok
}
```

Note the entry's return type: `?` can only short-circuit into a
signature that can carry the failure, so the program declares
`Result<Program, MalformedInt>` — honesty about failure goes all the
way to the top.

## Inline Error Unions

The error slot is a regular type, so it can be a union written right in
the signature — no error enum to declare per call site:

```canon
File * Path => Result<Bytes, IoError + NotFound + PermissionDenied> {
    File
        -> Read(Path)?
        -> Decoded
}
```

Unions widen along `?`: a callee returning `Result<T, IoError>`
propagates cleanly out of a caller declaring
`Result<U, IoError + ParseError>`, because its errors are a subset.
Alphabetical enforcement makes that check purely syntactic — every
union has exactly one spelling.

Name errors after **what failed** (`InvalidUrl`, `MalformedJson`), not
after who raised them. And remember that
[validated constructors](./types-and-values.md) return `Result` — so
"parse, don't validate" is not a slogan here; it is what constructors
are.

**Precise rules:** [Expressions & Dispatch](../spec/expressions.md).

**Next:** [Capabilities](./capabilities.md) — effects without an
effect system.
