# Constructors That Check

A type can replace its default constructor with one that validates. Now
`Url("…")` returns a `Result`, every caller is forced to handle the
failure, and only code in the type's own file can touch the raw string.

An invalid `Url` cannot exist anywhere in the program — not because a
rule says so, but because there is no way to make one. This is Canon's
entire encapsulation mechanism: no `private`, no getters, no
constructor conventions to remember. "Parse, don't validate" is not a
slogan here; it is what constructors are.

Name errors after **what failed** — `InvalidUrl`, `MalformedJson` — not
after who raised them.

```canon,run
InvalidUrl = String

Url = String

String => Result<Url, InvalidUrl> {
    String -> Length -> Gt(0) -> (
        * False => Result<Url, InvalidUrl> { String -> InvalidUrl -> Err }
        * True => Result<Url, InvalidUrl> { String -> Ok }
    )
}

Unit => Program {
    Url("https://canon-lang.org") -> (
        * Err<InvalidUrl> => Unit { "rejected" -> Print }
        * Ok<Url> => Unit { Url -> String -> Print }
    )
    Url("") -> (
        * Err<InvalidUrl> => Unit { "rejected" -> Print }
        * Ok<Url> => Unit { Url -> String -> Print }
    )
}
```
