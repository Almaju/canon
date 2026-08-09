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
Birthday = String

User = Birthday * Username

Username = String
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
  When types alone cannot decide -- two untagged values competing for
  fields that share an underlying type -- the construction is a
  **compile error** naming the fields to tag: written order never
  silently carries the meaning. A single untagged value stays legal
  when its field is forced (every other same-base field is already
  bound by an exact match), which is the shape recursive builders use
  (`Node(String * Value)` inside `map.can`, where the piped `Key` and
  the exact `Value` leave one slot).

## Newtypes

`A = B` (single named type on the right) declares a **newtype**: a
distinct type that wraps `B`. Formally it is a 1-component product, and
the field-access rule applies uniformly:

```canon
Greeting = String

Unit => Program {
    Greeting("hi").String -> Print
}
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

Where `T^N` works today is **constructor inputs**: `Int^2 => Ord` binds
two positional `Int` components, reached as `Int.1` / `Int.2` in the
body (a bare `Int` reference is an error -- position is the identity),
and bound positionally at call sites (`3 -> Ord(5)`: the receiver is
`.1`). See [Functions § The Binding Rule](./functions.md#the-binding-rule)
and the stdlib's `Ord` for the reference use. A `T^N` component inside
a *type definition* is accepted structurally but has no value-level
lowering yet.

`List<T>` is itself compiler-supplied, not derived from `T^*` --
`List(...)` is its value-level constructor, with methods like
`Mapped`, `Filtered`, `Taken`, `First`, and `At`. Indexing is **1-based** everywhere
(`list -> At(1)` is the first element, `string -> ByteAt(1)` the first
byte): one origin, matching positional product access `.1`.

## Generics

Types may be parameterized with angle brackets. For the
compiler-supplied types (`List<T>`, `Option<T>`, `Result<T, E>`,
`Future<T>`, `Stream<T>`), type arguments are the one thing the
compiler fills in from a call site's declared argument types
(`List(1 * 2) -> Mapped(f)` instantiates `T = Int`) — signatures
themselves are always written in full ([No Signature
Inference](#no-signature-inference) is about signatures, not type
arguments).

User declarations take parameters the same way, on both typedefs and
constructor arrows:

```canon
Box<T> = T

Same<T> = T

<T>(T) => Same<T> {
    T
}

Unit => Program {
    Box<Int>(42)
        -> String
        -> Print
    Same<String>("echo") -> Print
}
```

- Parameters are PascalCase (a `<` followed by a lowercase letter opens
  an HTML literal), pairwise distinct, and may not shadow a declared
  type.
- **Uses spell their arguments.** Outside a generic declaration, a
  generic name is applied in full — `Box<Int>(42)`,
  `value -> Inserted<String, Int>(…)` — and the argument count must
  match the declaration; a bare reference is an error. There is no
  inference from argument types for user generics yet.
- **A family shares its parameter names.** Inside a generic body, a
  bare reference to a sibling declaration — the zero-data variant, a
  result newtype, the recursive call — resolves each of the sibling's
  parameters through the enclosing declaration's binding by name
  (`Rest<K, V> = Store<K, V>`; an insertion body writes `Store`,
  `Entry`, `Rest` bare). A sibling whose parameter the binding doesn't
  cover must be applied explicitly.
- **Instantiation is expansion.** Each distinct application mints a
  concrete copy of the declaration (and, transitively, of everything
  it references) with the parameters substituted; the copy is ordinary
  Canon, checked in full — a generic body is checked through its
  instantiations, and codegen only ever sees concrete types. Two
  instantiations are two distinct types: `Store<String, Int>` and
  `Store<Int, String>` coexist with separate variants and layouts.

There is no constraint syntax: a parameter is bounded by the
operations its uses require, and each instantiation checks them
concretely.

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

The stdlib's `Map` and `Set` (`canon/Map`, `canon/Set`) are
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
    String -> Length -> Gt(0) -> (
        * False { String -> InvalidUrl -> Err }
        * True { String -> Ok }
    )
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
Unit => Program {
    String(42) -> Print
    Int("42")? -> Print
    Int(2.9) -> Print
    Byte(65)
        -> String
        -> Print
    List("1" * "2")
        -> Json
        -> Print
}
```

This prints `42` (decimal rendering; `String(2.5)` and `String(True())`
render the same way), `42` (parsing returns `Result<Int, MalformedInt>`,
unwrapped by `?`), `2` (a `Float` truncates toward zero), `A` (a `Byte`
renders as its character), and `[1,2]` (a list of JSON values as a JSON
array).

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
  in `canon/Map` makes `Map()` the empty map.

## No Signature Inference

Every signature is written explicitly: function signatures, lambda
signatures, and the dispatch arm types context doesn't already spell
(an arm in return position takes the enclosing declaration's written
return type -- [Expressions § Arm Types](./expressions.md#arm-types) --
which is propagation from a declared signature, not inference).
Declared signature and checked signature must match exactly. The rule
is about *signatures*, not about inference generally -- the compiler
infers plenty below the signature line (generic instantiation,
suspension and await points, boxing, argument-to-component binding,
imports), but never a type the writer should have declared.

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
