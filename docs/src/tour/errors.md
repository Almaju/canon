# Errors

Errors are values, carried by the standard `Result<T, E>` type. The error
slot is a regular type, so it can be a union written inline:

```canon
read = (File * Path) -> Result<Bytes, IoError + NotFound + PermissionDenied> {
    ...
}
```

This is more ergonomic than Rust's approach, where each call site
typically needs a dedicated error enum.

## The `?` Operator

The postfix `?` operator propagates failure. It works on both
`Result<T, E>` and `Option<T>`:

- On `Result<T, E>`: short-circuits with the error, otherwise unwraps to
  `T`.
- On `Option<T>`: short-circuits with `None`, otherwise unwraps to `T`.

```canon
main = () -> Result<Unit, Unit> {
    Ok(42)?.print()
    Some(7).(
        * (None) -> Unit { "absent".print() }
        * (Some<Int>) -> Unit { "present".print() }
    )
    Ok(Unit())
}
```

`Ok(42)?` evaluates to `42` (because the `Result` is `Ok`); if it were
`Err(_)`, the function would return early with that error.

## Option vs Result

`Option<T>` and `Result<T, Empty>` are structurally similar but **kept
distinct**:

- `None` means *absent*.
- `Err(_)` means *failed*.

The semantic difference is worth the duplication. Use `Option` when a
value can legitimately be missing; use `Result` when an operation can
legitimately fail.

## Chaining

Because `?` is postfix, error-propagating pipelines read top-down,
left-to-right:

```canon
readConfig = (File * Path) -> Result<Config, IoError + ParseError> {
    File
        .read(Path)?
        .parse()?
        .validate()
}
```

Each `?` unwraps the success case and lets the chain continue; the first
failure short-circuits the whole function.

## Validated Construction

The same `?` shows up at the construction site for types whose
[constructor is fallible](literals.md#validated-constructors).
A type with a declared constructor that returns `Result<Self, E>`
forces callers to handle the failure mode:

```canon
Url("https://example.com")?.get()?.print()
```

Both `?`s here are doing the same job: unwrapping a `Result` at the
point of use. The first handles `Url` parsing failure (`InvalidUrl`);
the second handles `.get()` failure (`HttpError`). The
function's return type then carries the union:
`Result<Unit, HttpError + InvalidUrl>`.

## Error Naming

Errors are types like any other, and they should be named *semantically*
— by what failed, not by who emitted them. `InvalidUrl`, `MalformedJson`,
`FileNotFound`, `PermissionDenied` carry information; `UrlError`,
`JsonError`, `FsError` don't.

The exception is opaque wrappers around foreign error types: when
binding to a Component Model interface whose error shape is a single
string or an opaque resource, it's pragmatic to keep the wrapper opaque
(e.g., `HttpError = String`) until the underlying error space gets
decomposed into proper Canon variants.
