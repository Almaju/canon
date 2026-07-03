# Expressions and Dispatch

## Precedence

Type-level, tightest first:

1. `T^N`, `T^*` — postfix repetition / Kleene star
2. `T<...>` — generic application
3. `*` — product
4. `+` — union

So `A + B * C^3` parses as `A + (B * (C^3))`.

Expression-level, tightest first:

1. `.` — call / field access / dispatch
2. `()` — application
3. `?` — postfix propagation
4. `*` — value-level product (only inside a constructor argument)

So `foo.bar()?` is `((foo.bar)())?`.

## Construction vs Observation

Field names and constructors are both PascalCase, so the dot syntax
needs one disambiguation rule, and it is the parentheses:

- `user.Birthday` — **field access**: reads the `Birthday` component.
- `user.Birthday()` — **constructor call**: calls `Birthday` with `user`
  as the receiver, producing a new value.

`()` signals *producing*; its absence signals *observing*. In type
position (signatures, dispatch patterns) a bare PascalCase name is
always the type.

## Function Bodies

A body is a newline-separated sequence of expressions; the **last
expression is the return value**. Non-final expressions are evaluated
and discarded — they exist for effects and for `?` propagation. With no
local variables, the way a value threads through several operations is a
method chain:

```canon
readConfig = (File * Path) -> Result<Config, IoError + ParseError> {
    File.read(Path)?
        .parse()?
        .validate()
}
```

## Dispatch

Dispatch is the language's only branching construct. The scrutinee (a
union value) is the receiver; the arms are its handlers:

```canon
Ord.(
    * (Equal) -> Sign { Zero() }
    * (Greater) -> Sign { Positive() }
    * (Less) -> Sign { Negative() }
)
```

Rules:

- Each arm is a lambda whose single parameter is one variant type; arms
  are separated by `*`. The leading `*` on the first arm is optional.
- Arms must appear in the union's **variant order** (alphabetical), and
  every variant must be handled — there is no wildcard arm.
- Dispatch is an expression; all arms must produce the same type.

Algebraically, dispatch is the isomorphism

```
(A + B + C) -> R  ≅  (A -> R) * (B -> R) * (C -> R)
```

made literal: a sum value applied to a product of handlers.

### Payload Binding

When a variant carries data, the arm body sees the payload under a
name determined by the pattern:

- **Stdlib containers** (`Ok<T>`, `Err<E>`, `Some<T>`): write the type
  argument explicitly; it binds the *unwrapped* value.

  ```canon
  result.(
      * (Err<IoError>) -> String { IoError.message() }
      * (Ok<String>) -> String { String }
  )
  ```

- **User-defined variants** with their own definition (`Branch = Left *
  Right * Value`): write just the variant name; the matched value is in
  scope under that name, fields accessible through it.

Dispatch also follows newtype alias chains: given
`MessageContent = Option<Content>`, a `MessageContent` value dispatches
on `(None, Some<Content>)` directly.

## The `?` Operator

Postfix `?` propagates failure and absence:

- On `Result<T, E>`: if `Err`, the enclosing function returns the error
  immediately; if `Ok`, the expression evaluates to the unwrapped `T`.
- On `Option<T>`: if `None`, the enclosing function returns `None`;
  otherwise unwraps to `T`.

The enclosing function's return type must be able to carry the
short-circuited value (a `Result` whose error slot includes `E`, or an
`Option`). Inline error unions compose at the signature:
`Result<Unit, HttpError + InvalidUrl>` accepts short-circuits from both
`Url(…)?` and `.get()?`.

`Option<T>` and `Result<T, E>` are deliberately distinct: `None` means
*absent*, `Err` means *failed*.

## JSON Literals

JSON object and array literals are first-class expressions producing
`Json` values (requires `use canon/std/Json`):

```canon
label = (Int) -> Json {
    {"answer": Int, "doubled": Int.mul(2), "ok": True()}
}
```

- **Static** members (strings, numbers, `true`/`false`/`null`, nested
  static literals) are baked into a constant at parse time.
- **Interpolated** members are ordinary Canon expressions converted at
  runtime via their `ToJson` instance.

## Operator and Sigil Glossary

| Symbol | Meaning |
|---|---|
| `+` | union (sum) |
| `*` | product (type-level and value-level) |
| `T^N` | fixed repetition |
| `T^*` | unbounded repetition (Kleene star) |
| `<T>` | generic parameter |
| `<T: Tr>` | generic with trait constraint |
| `::<T>` | type argument at a call site (turbofish) |
| `.` | call / field access / dispatch |
| `.( )` | dispatch on a union |
| `?` | propagate `Result` / `Option` failure |
| `"..."` | string literal |
| `{"k": v}` / `[v]` | JSON literal |
