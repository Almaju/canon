# Branching & Loops

Canon has one branching construct and zero loop keywords. This chapter
is both of them.

## Dispatch

To branch, pipe a union value into a group of handlers — one arm per
variant, every variant covered, in the union's (alphabetical) order,
with no wildcard:

```canon
True() -> (
    * False => Unit { "no" -> Print }
    * True => Unit { "yes" -> Print }
)
```

Each arm is a lambda for one variant; the whole dispatch is an
expression, so all arms produce the same type. When a variant carries
data, the arm names the payload type and the body sees it under that
name: `* Some<Int> => Unit { Int -> Print }`. Exhaustiveness is the
point — add a variant to a union and every dispatch that forgot it
stops compiling.

## Literal Dispatch

Strings and integers dispatch by equality. Literal arms can never cover
every value, so the final arm is a **mandatory catch-all** naming the
scrutinee's type — this is Canon's route table, switch statement, and
parser all at once:

```canon
Route -> (
    * "/notes" => Body { Index() }
    * String => Body { NotFound() }
)
```

## Loops Without Loops

Iteration is either an operation on a collection —

```canon
List(1 * 2 * 3) -> Mapped((Int) => Int { Int -> Product(2) })
```

— or plain recursion, with dispatch supplying the base case. A
recursive union plus a recursive constructor replaces the loop, the
counter, and the exit condition. Here is a linked chain measuring its
own length; run it:

```canon,run=learn-branching
Chain = Link + Stop

Len = Int

Link = Next

Next = Chain

Chain => Len {
    Chain -> (
        * Link => Len { Link.Next -> Len -> Sum(1) }
        * Stop => Len { 0 }
    )
}

Unit => Program {
    Stop()
        -> Next
        -> Link
        -> Next
        -> Link
        -> Len
        -> Print
}
```

`Len` calls itself on the rest of the chain until dispatch hits the
`Stop` arm — base case, recursive case, and branch are one construct.
The standard library's `Map` and `Set` are built exactly this way.

**Precise rules:** [Expressions & Dispatch](../spec/expressions.md).

**Next:** [Errors & Options](./errors-and-options.md) — what happens
when things fail.
