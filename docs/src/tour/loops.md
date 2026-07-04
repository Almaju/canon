# Loops

There are no loop keywords (`while`, `for`). Iteration is expressed
through higher-order methods on collections or recursion.

## Higher-Order Methods

For most collection work, use methods on the collection itself:
`map`, `get`, `length`, `first`.

```canon,run=list-map
main = () -> Unit {
    List(10, 20, 30)
        .map((Int) -> Int { Int.mul(2) })
        .length()
        .print()
}
```

## Why No Loop Keywords?

Loop keywords are special forms that exist outside the type-and-method
system. Canon expresses everything through types and methods, so
iteration belongs there too. A `.for()` method on a collection is a
method call like any other.
