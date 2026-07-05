# Loops

There are no loop keywords (`while`, `for`). Iteration is expressed
through higher-order methods on collections or recursion.

## Higher-Order Methods

For most collection work, use methods on the collection itself:
`map`, `get`, `length`, `first`, `append`, `concat`.

```canon,run=list-map
main = () => Unit {
    List(10, 20, 30)
        .map((Int) => Int { Int.mul(2) })
        .length()
        .print()
}
```

## Recursion

Anything the collection methods don't cover is plain recursion —
functions call themselves, and dispatch supplies the base case:

```canon,run=sum-to
sumTo = (Int) => Int {
    Int.eq(0).(
        * (False) => Int { Int.add(Int.sub(1).sumTo()) }
        * (True) => Int { 0 }
    )
}

main = () => Unit {
    5
        .sumTo()
        .print()
}
```

The stdlib's `Map` and `Set` are built this way — recursive unions
walked by recursive functions — as is the JSON validator; all three
make good reference code.

## Why No Loop Keywords?

Loop keywords are special forms that exist outside the type-and-method
system. Canon expresses everything through types and methods, so
iteration belongs there too. A `.for()` method on a collection is a
method call like any other.
