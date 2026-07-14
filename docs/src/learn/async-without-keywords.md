# Async

There is no `async` keyword and no `.await`. Whether a function
suspends is a fact the compiler can compute, so you never write it:

```canon
Unit => Program {
    Url("https://example.com")?
        -> Fetched?
        -> Print
}
```

`Fetched` performs a network request. The code does not say so — and
does not need to. Asynchrony enters through exactly one door: a host
binding whose interface is asynchronous returns a `Future<T>`.
Wherever a `Future<T>` is used in a position that expects `T`, the
compiler inserts the await; functions that transitively suspend are
lifted as async at the WebAssembly boundary. Two consequences:

- **No function coloring.** A call is a call; sync and async callees
  are spelled identically, so refactoring an implementation to suspend
  changes no caller.
- **Nothing to configure.** The "executor" is the host's
  implementation of the Component Model's async ABI — not a library
  choice.

`Future<T>` appears only in binding signatures and the occasional type
error; day-to-day code consumes the unwrapped `T`.

## Concurrency Is Two Combinators

Fan-out is expressed on the values, with the same commutative call
shape as everything else:

```canon
first -> Parallel(second)     # await both, results in order
first -> Race(second)         # first to finish wins; the loser is cancelled
```

Cancellation has no primitive — it falls out of composition. `Race`
cancels its losing branch; to abandon a future, stop using it.

## Streams

`Stream<T>` is to `List<T>` what `Future<T>` is to `T`. The stdlib
combinators (`streamOf`, `Mapped`, `filter`, `take`, `Joined`, `toList`,
`toString`) compile and run — streams are eager and list-backed today.
`Stream<T>` beyond that surface is a
[known gap](../reference/codegen-gaps.md) in codegen.

**Precise rules:** [Effects and the Async Model](../spec/effects-and-async.md).

**Next:** [Programs & Modules](./programs-and-modules.md) — how files
become programs.
