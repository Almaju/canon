# Types & Values

Every Canon type is composed with two operators: `+` means **or**, `*`
means **and**. That is the whole vocabulary — there is no `enum`, no
`struct`, no `class`, no `interface`.

```canon
Bool = False + True

User = Birthday * Username

Birthday = String
```

- `Bool` is a **union**: a value is a `False` *or* a `True`. Variants
  are listed alphabetically (the compiler checks).
- `User` is a **product**: a value has a `Birthday` *and* a `Username`.
  Fields are alphabetical too, and must be *distinct types* — which is
  where the third form comes in.
- `Birthday` is a **newtype**: a distinct type wrapping `String`. Two
  strings that mean different things get two names, and the names are
  checked wherever the values flow. Newtypes are how Canon
  disambiguates everything other languages use identifiers for.

A product is read by the type of its component — `User.Birthday` — so a
field never needs a name of its own. Generics use angle brackets
(`List<T>`, `Option<T>`, `Result<T, E>`), recursive types are boxed
automatically, and there is no type inference: every signature is
written out.

## Making Values

Values come from **constructors** — there is no `new` keyword and no
literal `true`/`false`. Scalar literals are sugar for construction
(`42` is `Int(42)`, `"hi"` is `String("hi")`), and zero-data types take
empty parens: `True()`, `None()`, `Unit()`.

Here is a product being built, read, and printed — press run:

```canon,run=learn-types
Birthday = String

Greeting = String

User = Birthday * Username

Username = String

User => Greeting {
    "hello, " -> Joined(User.Username)
}

Unit => Program {
    Birthday("1815-12-10")
        -> User(Username("ada"))
        -> Greeting
        -> Print
}
```

The arrow `User => Greeting { … }` declares the `Greeting` constructor:
give it a `User`, get a `Greeting`. Inside the body, `User` names the
input value, and `User.Username` reads its component. Notice how the
`User` is assembled: each argument binds to the field its *type*
selects, never to a position — the `Birthday` pipes in, the `Username`
rides in the parens. (The next chapter,
[Pipes & Constructors](./pipes-and-constructors.md), is all about
these arrows.)

## Conversion Is Construction

Converting a value to type `T` is spelled by constructing a `T`,
because that is what it is:

```canon
String(42)         # "42" — decimal rendering
Int("42")?         # parsing can fail, so it returns a Result
String(Byte(65))   # "A" — wrap in Byte to mean the character reading
```

There is no `parse` / `toString` / `from` / `into` family. When a
conversion is ambiguous, a newtype picks the meaning — `String(42)`
renders digits, `String(Byte(42))` renders the byte as a character.

## Validated Constructors

A type can replace its default constructor with one that checks:

```canon
Url = String

String => Result<Url, InvalidUrl> {
    String -> Parsed
}
```

Now `Url("…")` returns a `Result`, every caller is forced to handle the
failure (with `?` or dispatch), and only code in the type's own file
can touch the raw string — so an invalid `Url` cannot exist anywhere in
a program. This is Canon's entire encapsulation mechanism; there is no
`private` keyword to remember.

**Precise rules:** [Types](../spec/types.md) in the specification.

**Next:** [Pipes & Constructors](./pipes-and-constructors.md) — how
values move.
