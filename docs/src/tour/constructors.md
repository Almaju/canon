# Conversion Is Construction

Values come from **constructors**. There is no `new`, no literal
`true`/`false`, and no `parse` / `toString` / `from` / `into` family:
converting a value to a `T` is spelled by constructing a `T`, because
that is what it is.

Scalar literals are sugar for construction — `42` is `Int(42)`, `"hi"`
is `String("hi")` — and zero-data types take empty parens: `True()`,
`None()`, `Unit()`.

When a conversion is ambiguous, a newtype picks the meaning.
`String(42)` renders the digits; wrapping in `Byte` first renders the
character. Parsing the other way can fail, so `Int("42")` returns a
`Result` — which is step 11.

```canon,run
Unit => Program {
    String(42) -> Print
    Byte(65)
        -> String
        -> Print
}
```
