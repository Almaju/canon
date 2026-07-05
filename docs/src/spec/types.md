# Types

Every Canon type is built by composing a small algebra over a minimal
core. The algebra has three operators, `+` (union), `*` (product), and
`^` (repetition), and two identities.

## The Core

- **`Unit`**: the type with exactly one value; the multiplicative
  identity (`T * Unit ≡ T`).
- **`Never`**: the type with zero values; the additive identity
  (`T + Never ≡ T`).

Together with `+` and `*` these form a type **semiring**. The boolean
atoms are newtype aliases of `Unit`, and everything scales up from
there:

```canon
Bit = False + True

Bool = False + True

Byte = Bit^8

Bytes = Byte^*

False = Unit

True = Unit
```

The higher-level primitives (`Int`, `Float`, `Hex`, `String`) are
defined from `Byte`/`Bytes`. The compiler supplies a small set of
built-in operations on them (e.g. integer arithmetic), but their shape
is still described by the algebra.

## Unions (`+`)

`A + B` is a value of `A` **or** `B`:

```canon
Ord = Equal
  + Greater
  + Less
```

Variants must be listed in [alphabetical order](./ordering.md). There is
no `enum` keyword. Branching on a union is [dispatch](./expressions.md#dispatch).

## Products (`*`)

`A * B` is a value with an `A` **and** a `B`:

```canon
User = Birthday * Username
```

- Components must be in alphabetical order.
- Components must be **distinct types**: `(User * User)` is a compile
  error. Disambiguate with a newtype (`OtherUser = User`).
- A component is accessed by its type name: `user.Birthday`.
- For repeated or anonymous components (from `^N`), access is by
  1-based position: `byte.1`, `byte.2`.
- **Construction is positionless.** A value binds to the field whose
  type it is, not to the slot it is written in, so
  `User(Username("ada") * Birthday("…"))` and
  `User(Birthday("…") * Username("ada"))` build the same value — position
  never carries meaning. Because the components are distinct types, each
  value's type selects its field; `canon fmt` canonicalises the written
  order alphabetically. Where two fields share an underlying type
  (`Key = String` and `Value = String` in `Node = Key * Rest * Value`),
  tag the values with the newtype — `Node(Key("k") * Value("v") * …)` —
  so each still selects its field. A bare, untagged `String` carries no
  such tag and falls back to declaration order.

## Newtypes

`A = B` (single named type on the right) declares a **newtype**: a
distinct type that wraps `B`. Formally it is a 1-component product, and
the field-access rule applies uniformly:

```canon
Greeting = String

Greeting("hi").String
```

Rules:

- **Method inheritance.** Methods declared on `B` are callable on `A`
  through the alias chain (`Greeting("hi").print()`). Methods declared
  on `A` shadow inherited ones.
- **Substitutability.** A value of `A` may be passed where `B` is
  expected, without unwrapping. The reverse also holds at construction:
  `A(b)` wraps a `B`.
- **Distinctness.** For product-membership and disambiguation purposes,
  `A` and `B` are different types.
- **Multi-step chains** unwrap one step at a time: with `A = B` and
  `B = C`, reach the bottom via `aValue.B.C`.

## Repetition (`^N`, `^*`)

`T^N` is the N-fold product `T * T * … * T`; `T^*` is the Kleene star,
zero or more `T`s. Both share the `^` operator and complete the
semiring reading: sums, products, exponents.

`List<T>` is not a separate concept. Core defines **`List<T> = T^*`**;
the nominal name and the algebraic form are the same type. `Bytes =
Byte^*` therefore has every `List` method (`map`, `first`, `get`, …)
with nothing to declare, and `List(…)` is the value-level constructor
for the star. Indexing is **1-based** everywhere (`list.get(1)` is the
first element, `byteAt(1)` the first byte): one origin, matching
positional product access `.1`.

## Generics

Types may be parameterized with angle brackets: `List<T>`,
`Option<T>`, `Result<T, E>`. Constraints name a trait after `:`:

```canon
showAll = <T: Show>(List<T>) => Unit {
    ...
}
```

Where a type parameter cannot be inferred from context, the call site
pins it with turbofish: `parse::<List<Int>>(…)`. See
[Functions and Traits](./functions.md).

## Recursive Types

Recursive definitions are legal and **boxed automatically**:

```canon
Branch = Left * Right * Value

Left = Tree

Right = Tree

Tree = Branch + Leaf

Value = Int
```

There is no user-visible `Box<T>`; the compiler chooses the indirection.

The stdlib's `Map` and `Set` (`canon/std/Map`, `canon/std/Set`) are
recursive unions in exactly this shape — `Map = Empty + Node` with
`Node = Key * Rest * Value` and `Rest = Map` — and double as reference
code for the pattern.

## Validated Constructors

By default every type `T` has a total constructor `T(inner)`. A file may
replace it by declaring a **function with the type's own name**:

```canon
Url = String

Url = (String) => Result<Url, InvalidUrl> {
    ...
}
```

- If a constructor is declared, it *is* the constructor; the implicit
  total one is gone.
- The signature is unconstrained: total (`=> Url`), fallible
  (`=> Result<Url, E>`), or optional (`=> Option<Url>`).
- Call sites keep ordinary constructor syntax (`Url("…")`), but the
  expression's type is the constructor's return type, so a fallible
  constructor forces `?` or dispatch at every use.
- External callers cannot bypass it: only functions declared in the same
  file as the type may touch the raw inner representation. This is the
  language's entire encapsulation story; see
  [visibility](./modules.md#visibility).

## Conversions

**Conversion is construction.** There is no `parse` / `toString` /
`from` / `into` family — converting a value to type `T` is spelled as
constructing a `T`, because it is one:

```canon
String(42)            # "42" — decimal rendering
42.String()           # the same declaration, method spelling
Int("42")             # Result<Int, MalformedInt> — parsing can fail
"42".Int()?           # method spelling, ?-propagated
String(Byte(65))      # "A" — a Byte renders as its character
List("1", "2").Json() # [1,2] — a list of JSON values as a JSON array
```

- Infallible conversions return the target type; the function's name
  *is* its return type, so it cannot lie about what it produces.
- Fallible conversions are [validated
  constructors](#validated-constructors) returning `Result<T, E>` —
  `Int(String)` forces `?` or dispatch exactly like `Url(String)`.
- `T(value)` and `value.T()` are the same declaration (the commutative
  method-call rule), so what Rust splits into `From` and `Into` is one
  function here.
- Ambiguity is resolved by newtypes: `String(42)` renders decimal
  digits, `String(Byte(42))` is the one-byte string `"*"` — wrapping
  to mean the other thing is what newtypes are for.

User types opt in the same way the stdlib does: declare a function
named after the target type taking the source type.
`Celsius = (Fahrenheit) => Celsius { … }` enables both `Celsius(f)`
and `f.Celsius()`.

## Zero-Data Types

A type with no underlying data (`Unit`, `True`, `False`, a payload-less
variant) has exactly one value, produced with an empty argument list:
`True()`, `None()`. Calling a **data-carrying** constructor with no
arguments (`String()`, `User()`) is a compile error: absence belongs in
`Option<T>`, not in a default value.

Two escape hatches exist, both deliberate:

- `List()` is the **empty list** — the type's zero value, and the base
  case that recursive builders grow from via `.concat(…)` /
  `.append(…)`.
- A type may declare its own zero-arg [validated
  constructor](#validated-constructors): `Map = () => Map { Empty() }`
  in `canon/std/Map` makes `Map()` the empty map.

## No Type Inference

Every type is written explicitly: function signatures, lambda
signatures, dispatch arm types. Declared shape and checked shape must
match exactly.

## Dead Code

A **program's** declarations must be reachable from its entry point.
`canon check` walks the reference graph from `main` (or the HTTP
handler) and warns on every unreachable type and function:

```
warning: `unused` is never used: dead code is not allowed to
accumulate; delete it or wire it into the program
```

The warning is promoted to a failure in CI. Libraries are exempt: with
no private visibility, every declaration in a library *is* exported
surface, so its dead code shows up downstream, in the programs that
stopped calling it.
