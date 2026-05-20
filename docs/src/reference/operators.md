# Operators

## Type-Level Precedence

Tightest first:

1. `T^N`, `T^*` — postfix repetition / Kleene star
2. `T<...>` — generic application
3. `*` — product
4. `+` — union

So `A + B * C^3` parses as `A + (B * (C^3))`.

## Expression-Level Precedence

Tightest first:

1. `.` — method call / field access / dispatch
2. `()` — function application
3. `?` — postfix error propagation
4. `*` — value-level product (only inside a constructor argument)

So `foo.bar()?` is `((foo.bar)())?`.

## Glossary of Operators and Sigils

| Symbol      | Meaning                                  |
|-------------|------------------------------------------|
| `+`         | Union (sum)                              |
| `*`         | Product                                  |
| `T^N`       | Fixed repetition (N copies)              |
| `T^*`       | Unbounded repetition (Kleene star)       |
| `<T>`       | Generic parameter                        |
| `<T: Tr>`   | Generic with trait constraint            |
| `::<T>`     | Type argument at a call site (turbofish) |
| `.`         | Method call / field access / dispatch    |
| `.( )`      | Dispatch on a union (replaces `match`)   |
| `?`         | Propagate `Result` / `Option` failure    |
| `*name`     | Private method (file-local)              |
| `"..."`     | String literal sugar                     |
| `mut`       | Mutable parameter                        |

`::<T>` after a method name pins a generic method's type parameter when
the compiler cannot infer it from context:

```oneway
Json.parse::<List<Int>>("[1, 2, 3]")?
```
