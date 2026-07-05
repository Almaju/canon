# Literals

Canon is values-only. There is no `new`, no implicit nullability, no
keywords like `true` or `false`. Every value is built by calling its
type's constructor.

## Constructors

Every type `T` has a constructor `T(_)`. The argument is a value matching
the type's underlying definition:

| Kind             | Constructor                              | Argument                                      |
|------------------|------------------------------------------|-----------------------------------------------|
| Primitive        | `Int(123)`, `Float(1.0)`, `String("hi")` | a literal of the corresponding lexical kind   |
| Hex              | `Hex(0xFF0000)`                          | a hex literal                                 |
| Product `A * B`  | `T(A(...) * B(...))`                     | a value-level product joined with `*`         |
| Union `A + B`    | `T(A(...))` or `T(B(...))`               | a value of any variant                        |
| Newtype          | `T(inner)`                               | a value of the aliased type                   |

## Literal Sugar

A handful of literals desugar to their constructors:

| Literal     | Desugars to       |
|-------------|-------------------|
| `123`       | `Int(123)`        |
| `1.0`       | `Float(1.0)`      |
| `"abc"`     | `String("abc")`   |
| `0xFF0000`  | `Hex(0xFF0000)`   |
| `{"k":v}`, `[v]` | a `Json` value ([JSON literals](../spec/expressions.md#json-literals)) |

Numeric literals exist to avoid boilerplate in arithmetic-heavy code.
String literals exist to avoid the parsing ambiguity of bare `String(...)`
with spaces and punctuation.

## Zero-Data Constructors

A type with no underlying composition (`Unit`, `True`, `False`, a
payload-less union variant) is constructed with empty parens:

```canon
Unit()
True()
None()
```

The `()` unambiguously signals *producing a value*: `value.Field`
(no parens) reads a field, `Type()` constructs. In type position the
bare name is still the type, as in `-> Unit`, and after `.` a bare
PascalCase name is always a field access.

## Constructing a Product

Product constructors take their components joined with value-level `*`:

```canon
User(Birthday("1990") * Username("ahanot"))
Hex(0xFF0000)
```

`*` is overloaded across the two levels: at the type level it forms a
product type, at the value level it forms a product value. The two never
appear in the same context.

## Validated Constructors

By default, a type's constructor is total: `T(inner)` always succeeds and
returns `T`. For types whose construction can fail (a `Url` or an `Email`
parsed from a `String`), the fallibility belongs in the type system as
`Result<T, E>`. Same principle the language already applies to "missing":
`Option<T>`.

A type opts in by declaring a constructor with the **same name as the
type**: a function whose name matches the type it constructs. The body is
ordinary Canon — here, delegating to a host-provided parser bound in a
binding file:

```canon
Url = String

Url = (String) => Result<Url, InvalidUrl> {
    String.parse()
}
```

A declared constructor replaces the implicit total one. The signature is
unconstrained: total (`(String) -> Url`), fallible
(`Result<Url, InvalidUrl>`), or optional (`Option<Url>`). Call sites still
use the ordinary constructor syntax `Url("https://example.com")`, but the
expression's type is now whatever the constructor returns, so a fallible
constructor *forces* `?` (or dispatch) at the call site:

```canon
Url("https://example.com")? -> Fetched?.print()
```

External callers cannot bypass the constructor. The raw inner
representation is only accessible inside the same file as the type.

## Conversions

The constructor spelling covers conversion too — **conversion is
construction**. There is no `parse`, no `toString`, no `from`/`into`
family; converting a value to `T` is spelled `T(value)` (or
`value.T()`, the method form of the same declaration):

```canon,run=conversions
() => Unit {
    String(42) -> Print
    123
        -> String
        -> Joined("!")
        -> Print
}
```

Infallible conversions return the target type — the name cannot lie.
Fallible ones are validated constructors returning `Result<T, E>`:
`Int("42")` from `canon/std/Int` returns `Result<Int, MalformedInt>`,
forcing `?` or dispatch at the call site. When one source type has two
readings, a newtype picks the second one: `String(42)` is `"42"`,
while `String(Byte(42))` is the one-byte string `"*"`.
