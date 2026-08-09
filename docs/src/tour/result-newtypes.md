# Naming What Happened

What do you call an operation that takes a `Map` and returns a `Map`?
In Canon, nothing — functions have no names. The operation returns a
**result newtype** named after what it did: `Inserted = Map`,
`Removed = Map`.

So `-> Inserted("a" * "1")` reads as what it is, and because a newtype
flows anywhere its base type is expected, chaining is free. This is the
whole convention behind the standard library, and it is also why shared
vocabulary needs no coordination: `Map`, `Set`, `String`, and `List`
each declare the same `Length = Int` and contribute their own arrow to
it.

An arrow may not construct a type that appears in its own input, which
is what forces the result-newtype name instead of an endomorphism.

```canon,run
Unit => Program {
    Map()
        -> Inserted("b" * "2")
        -> Inserted("a" * "1")
        -> Removed("b")
        -> Keys
        -> Json
        -> Print
}
```
