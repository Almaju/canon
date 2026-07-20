# Types

Every user-declared Canon type is built by composing a small algebra
over a minimal core. The algebra has three operators, `+` (union), `*`
(product), and `^` (repetition), and two identities.

## The Core

- **`Unit`**: the type with exactly one value; the multiplicative
  identity (`T * Unit == T`).
- **`Never`**: the type with zero values; the additive identity
  (`T + Never == T`).

Together with `+` and `*` these form a type **semiring**, but the
algebra doesn't reach every primitive: `Bool`, `Int`, `Float`, and
`String` are opaque, compiler-supplied types, not composed from `Unit`.
`False` and `True` are `Bool`'s two built-in variants. `Byte` is an
ordinary stdlib newtype of `Int` (`Byte = Int`), used where a value
should read as a one-character `String` (`String(Byte(65))` is `"A"`).

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
  `User(Username("ada") * Birthday("..."))` and
  `User(Birthday("...") * Username("ada"))` build the same value -- position
  never carries meaning. Because the components are distinct types, each
  value's type selects its field; `canon check --fix` canonicalises the written
  order alphabetically when every input carries its type syntactically
  (a tagged construction, a typed reference). Where two fields share an
  underlying type (`Key = String` and `Value = String` in
  `Node = Key * Rest * Value`), tag the values with the newtype --
  `Node(Key("k") * Value("v") * ...)` -- so each still selects its field.
  A bare, untagged `String` carries no such tag and falls back to
  declaration order -- which is why `canon check --fix` never reorders literal
  operands: their position is their identity.

## Newtypes

`A = B` (single named type on the right) declares a **newtype**: a
distinct type that wraps `B`. Formally it is a 1-component product, and
the field-access rule applies uniformly:

```canon
Greeting = String

Greeting("hi").String
```

Rules:

- **Operation inheritance.** Functions whose input product mentions `B`
  accept an `A` through the alias chain (`Greeting("hi") -> Print`).
  A family member declared on `A` shadows the inherited one.
- **Substitutability.** A value of `A` may be passed where `B` is
  expected, without unwrapping. The reverse also holds at construction:
  `A(b)` wraps a `B`.
- **Distinctness.** For product-membership and disambiguation purposes,
  `A` and `B` are different types.
- **Multi-step chains** unwrap one step at a time: with `A = B` and
  `B = C`, reach the bottom via `aValue.B.C`.

## Repetition (`^N`, `^*`)

`T^N` is the N-fold product `T * T * ... * T`, accessed positionally
(`byte.1`, `byte.2`); `T^*` is the Kleene star, zero or more `T`s,
completing the semiring reading: sums, products, exponents.

`List<T>` is itself compiler-supplied, not derived from `T^*` --
`List(...)` is its value-level constructor, with methods like
`Mapped`, `First`, and `At`. Indexing is **1-based** everywhere
(`list -> At(1)` is the first element, `string -> ByteAt(1)` the first
byte): one origin, matching positional product access `.1`.

## Generics

Types may be parameterized with angle brackets: `List<T>`,
`Option<T>`, `Result<T, E>`. Type arguments are the one thing the
compiler fills in from a call site's declared argument types
(`List(1 * 2) -> Mapped(f)` instantiates `T = Int`) — signatures
themselves are always written in full ([No Type
Inference](#no-type-inference) is about signatures, not type
arguments). Constraint syntax (`<T: Show>`) is part of the shape
mechanism and returns with it ([Functions § Shape or Result
Newtype](./functions.md#shape-or-result-newtype)).

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
recursive unions in exactly this shape -- `Map = Empty + Node` with
`Node = Key * Rest * Value` and `Rest = Map` -- and double as reference
code for the pattern.

## Validated Constructors

By default every type `T` has a total constructor `T(inner)`. A file may
replace it by declaring the **anonymous constructor arrow** for the
type:

```canon
Url = String

String => Result<Url, InvalidUrl> {
    ...
}
```

(The named spelling `Url = (String) => …` repeats the name the
signature already carries; `canon check --fix` rewrites it to the arrow.)

- If a constructor is declared, it *is* the constructor; the implicit
  total one is gone.
- The signature is unconstrained: total (`=> Url`), fallible
  (`=> Result<Url, E>`), or optional (`=> Option<Url>`).
- Call sites keep ordinary constructor syntax (`Url("...")`), but the
  expression's type is the constructor's return type, so a fallible
  constructor forces `?` or dispatch at every use.
- External callers cannot bypass it: only functions declared in the same
  file as the type may touch the raw inner representation. This is the
  language's entire encapsulation story; see
  [visibility](./modules.md#visibility).

## Conversions

**Conversion is construction.** There is no `parse` / `toString` /
`from` / `into` family -- converting a value to type `T` is spelled as
constructing a `T`, because it is one:

```canon
String(42)              # "42" -- decimal rendering; String(2.5) and
                        # String(True()) render the same way
Int("42")?              # Result<Int, MalformedInt> -- parsing can fail
Int(2.9)                # 2 -- a Float truncates toward zero
String(Byte(65))        # "A" -- a Byte renders as its character
List("1" * "2") -> Json # [1,2] -- a list of JSON values as a JSON array
```

- Infallible conversions return the target type; the function's name
  *is* its return type, so it cannot lie about what it produces.
- Fallible conversions are [validated
  constructors](#validated-constructors) returning `Result<T, E>` --
  `Int(String)` forces `?` or dispatch exactly like `Url(String)`.
- `T(value)` and `value -> T` are the same declaration (the commutative
  call rule), so what Rust splits into `From` and `Into` is one
  function here.
- Ambiguity is resolved by newtypes: `String(42)` renders decimal
  digits, `String(Byte(42))` is the one-byte string `"*"` -- wrapping
  to mean the other thing is what newtypes are for.

User types opt in the same way the stdlib does: declare the anonymous
constructor arrow from the source type.
`Fahrenheit => Celsius { ... }` enables both `Celsius(f)` and
`f -> Celsius`.

## Zero-Data Types

A type with no underlying data (`Unit`, `True`, `False`, a payload-less
variant) has exactly one value, produced with an empty argument list:
`True()`, `None()`. Calling a **data-carrying** constructor with no
arguments (`String()`, `User()`) is a compile error: absence belongs in
`Option<T>`, not in a default value.

Two escape hatches exist, both deliberate:

- `List()` is the **empty list** -- the type's zero value, and the base
  case that recursive builders grow from via `-> Joined(...)` /
  `-> Appended(...)`.
- A type may declare its own nullary [validated
  constructor](#validated-constructors): `Unit => Map { Empty() }`
  in `canon/std/Map` makes `Map()` the empty map.

## No Type Inference

Every type is written explicitly: function signatures, lambda
signatures, dispatch arm types. Declared shape and checked shape must
match exactly.

## Dead Code

A **program's** declarations must be reachable from its entry point.
`canon check` walks the reference graph from the entry and reports
every unreachable type and function as a hard error:

```
error: `unused` is never used: dead code is not allowed to
accumulate; delete it or wire it into the program
```

Libraries are exempt: with no private visibility, every declaration in
a library *is* exported surface, so its dead code shows up downstream,
in the programs that stopped calling it.
