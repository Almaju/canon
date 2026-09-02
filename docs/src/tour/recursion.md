# Loops Without Loops

Canon has no loop keyword. Iteration is either an operation on a
collection — `Mapped`, `Filtered`, and `Folded` take a lambda, which is
the same arrow written inline with its full signature (the whole list
vocabulary is on the [Builtins](../reference/builtins.md) page) — or
plain **recursion**, with dispatch supplying the base case.

`Chain` is a union that contains itself, so `Len` calls itself on the
rest of the chain until dispatch reaches the `Stop` arm. Base case,
recursive case, and branch are one construct; there is no counter and
no exit condition to get wrong. Recursive types are boxed for you.

The standard library's `Map` and `Set` are built exactly this way.

```canon,run
Chain = Link + Stop

Len = Int

Link = Next

Next = Chain

Chain => Len {
    Chain -> (
        * Link { Link.Next -> Len -> Sum(1) }
        * Stop { 0 }
    )
}

Unit => Program {
    List(1 * 2 * 3)
        -> Mapped((Int) => Int { Int -> Product(2) })
        -> Length
        -> Print
    Stop()
        -> Next
        -> Link
        -> Next
        -> Link
        -> Len
        -> Print
}
```
