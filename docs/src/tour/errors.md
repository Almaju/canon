# Failure Is a Value

Failure and absence are different things, so they are different types:
`Result<T, E>` means the operation *failed* with an `E`, `Option<T>`
means the thing was *not there*. That split is how Canon does without
`null` and without exceptions.

Postfix `?` propagates: it short-circuits the error (or the `None`) out
of the enclosing arrow, and otherwise unwraps. Every failure that can
happen is visible as a `?` at the exact call that can fail.

To *handle* a failure instead of propagating it, dispatch — `Result`
and `Option` are ordinary unions. Note the entry's return type: `?` can
only short-circuit into a signature that can carry the failure, so
honesty about failure goes all the way to the top — and a failure that
reaches the entry prints its message and exits 1.

```canon,run
Unit => Result<Program, MalformedInt> {
    Int("42")?
        -> Sum(8)
        -> Print
    Int("4x") -> (
        * Err<MalformedInt> => Unit { `not a number: {MalformedInt}` -> Print }
        * Ok<Int> => Unit { Int -> Print }
    )
    Unit() -> Ok
}
```
