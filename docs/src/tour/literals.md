# Literals

Canon is values-only. There is no `new`, no implicit nullability, no
keywords like `true` or `false`. Every value is built by calling its
type's constructor.

## Constructors

Every type `T` has a constructor `T(_)`. The argument is a value matching
the type's underlying definition:

| Kind             | Constructor                              | Argument is…                                  |
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

Numeric literals exist to avoid boilerplate in arithmetic-heavy code.
String literals exist to avoid the parsing ambiguity of bare `String(...)`
with spaces and punctuation.

## Singleton Values

A singleton type — one with no underlying composition — has one value,
referenced by writing the type name:

```canon
Unit      // the sole value of type Unit
On        // the sole value of type On
```

## No Empty Constructors

`String()`, `Int()`, `User()` — calling any constructor with zero
arguments is a compile-time error. If a value can legitimately be
"missing", that absence belongs in the type as `Option<T>`. Otherwise the
type requires its data.

For factory-style construction (an empty list, etc.), use an explicit
method like `List.empty` or `String.empty`.

## Constructing a Product

The argument to a product's constructor is its components joined with
value-level `*`:

```canon
user = User(Birthday(...) * Username("ahanot"))
red  = Hex(0xFF0000)
```

`*` is overloaded across the two levels: at the type level it forms a
product type, at the value level it forms a product value. The two never
appear in the same context.

## Validated Constructors

By default, a type's constructor is total: `T(inner)` always succeeds and
returns `T`. For types whose construction can fail — a `Url` parsed from
a `String`, an `Email` from a `String`, etc. — the fallibility belongs in
the type system as `Result<T, E>`. Same principle the language already
applies to "missing": `Option<T>`.

A type opts into this by declaring a constructor with the **same name as the type** — a function whose name matches the type it constructs. The body can be ordinary Canon, or it can be an `extern Wasm` binding to a host-provided parser:

```canon
Url = String

extern Wasm("canon:builtins/url@0.1.0#parse")
Url = (String) -> Result<Url, InvalidUrl>
```

When a type declares a constructor, that *is* the constructor — the
implicit total constructor is replaced. The signature is unconstrained:
total (`(String) -> Url`), fallible (`Result<Url, InvalidUrl>`), or
optional (`Option<Url>`). Call sites still use the ordinary constructor
syntax `Url("https://example.com")`, but the expression's type is now
whatever the constructor returns, so a fallible constructor *forces* `?` (or
dispatch) at the call site:

```canon
Url("https://example.com")?.get()?.print()
```

External callers cannot bypass the constructor. The raw inner
representation is only accessible inside the same file as the type.
