# Types Are the Vocabulary

Two operators build every type in Canon. `+` means **or**, `*` means
**and**. There is no `enum`, no `struct`, no `class`, no `interface`.

- `Plan = Free + Pro` is a **union** — a value is one *or* the other.
- `User = Birthday * Username` is a **product** — a value carries both.
- `Username = String` is a **newtype**: a distinct type wrapping a
  primitive. Two strings that mean different things get two names, and
  the checker keeps them apart everywhere they flow.

A product's fields must be *distinct types*, because a field is read by
its type — `User.Username` — and never by a name of its own. That is
why newtypes matter: they are what other languages spend identifiers
on. Fields and variants are listed alphabetically; the compiler checks.

```canon,run
Birthday = String

Line = String

User = Birthday * Username

Username = String

User => Line {
    User.Username -> Line
}

Unit => Program {
    Birthday("1815-12-10")
        -> User(Username("ada"))
        -> Line
        -> Print
}
```
