# Async Without a Keyword

There is no `async` and no `await`. Whether an arrow suspends is a fact
the compiler can compute, so you never write it down.

`Fetched` performs a network request and this code does not say so.
Asynchrony enters through exactly one door: a host binding whose
interface is asynchronous returns a `Future<T>`. Wherever a `Future<T>`
is used where a `T` is expected, the compiler inserts the await and
lifts every arrow that transitively suspends. So there is
**no function coloring** — sync and async calls are spelled
identically, and making an implementation suspend changes no caller —
and nothing to configure, because the executor is the host's async ABI,
not a library.

Concurrency is two combinators on the values themselves:
`first -> Parallel(second)` awaits both, `first -> Race(second)` takes
the winner and cancels the loser. Cancellation needs no primitive.

```canon
Unit => Program {
    Url("https://example.com")?
        -> Fetched?
        -> Print
}
```
