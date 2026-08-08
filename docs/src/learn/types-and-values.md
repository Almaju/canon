# Types & Values

Every Canon type is composed with two operators: `+` means **or**, `*`
means **and**. That is the whole vocabulary — there is no `enum`, no
`struct`, no `class`, no `interface`.

```canon
Birthday = String

Plan = Free + Pro

User = Birthday * Username

Username = String
```

- `Plan` is a **union**: a value is a `Free` *or* a `Pro`.
- `User` is a **product**: a value has a `Birthday` *and* a `Username`.
  Its fields must be *distinct types* — which is where the third form
  comes in.
- `Birthday` is a **newtype**: a distinct type wrapping `String`. Two
  strings that mean different things get two names, checked wherever
  the values flow — newtypes disambiguate everything other languages
  use identifiers for.

(Variants and fields are listed alphabetically; the compiler checks.)

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
    `hello, {User.Username}`
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
selects, never to a position.

## Conversion Is Construction

Converting a value to type `T` is spelled by constructing a `T`,
because that is what it is:

```canon
Unit => Program {
    String(42) -> Print
    Int("42")? -> Print
    Byte(65)
        -> String
        -> Print
}
```

This prints `42`, then `42` again, then `A`. There is no `parse` /
`toString` / `from` / `into` family; the `?` after `Int("42")` is there
because parsing can fail, so it returns a `Result`. When a conversion
is ambiguous, a newtype picks the meaning — `String(42)` renders
digits, wrapping in `Byte` renders the byte as a character.

## Validated Constructors

A type can replace its default constructor with one that checks:

```canon
Url = String

String => Result<Url, InvalidUrl> {
    String -> Length -> Gt(0) -> (
        * False => Result<Url, InvalidUrl> { String -> InvalidUrl -> Err }
        * True => Result<Url, InvalidUrl> { String -> Ok }
    )
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
