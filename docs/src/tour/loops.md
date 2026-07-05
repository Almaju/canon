# Loops

There are no loop keywords (`while`, `for`). Iteration is expressed
through higher-order methods on collections or recursion.

## Higher-Order Methods

For most collection work, use methods on the collection itself:
`map`, `get`, `length`, `first`, `append`, `concat`.

```canon,run=list-map
Unit => Program {
    List(10, 20, 30)
        -> Mapped((Int) => Int { Int -> Product(2) })
        -> Length
        -> Print
}
```

## Recursion

Anything the collection methods don't cover is plain recursion — a
constructor references itself, and dispatch supplies the base case.
`Summed` is the running total up to a number, so it recurses on
`Summed` of the predecessor:

```canon,run=sum-to
Summed = Int

Int => Summed {
    Int -> Eq(0).(
        * (False) => Summed { Int -> Sum(Int -> Difference(1) -> Summed) }
        * (True) => Summed { 0 }
    )
}

Unit => Program {
    5
        -> Summed
        -> Print
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
