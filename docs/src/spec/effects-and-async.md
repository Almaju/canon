# Effects and the Async Model

Canon has no effect annotations, no capability tokens, and no `async` /
`await` keywords. Both "what can this function touch?" and "can this
function suspend?" are answered by **types and inference**, never by
syntax.

## Effects Are Values

There is no separate capability system; effects emerge from the values a
function requires. A function that reads a file needs a `File`; a `File`
can only be constructed from a `Path`; a `Path` from a `String`. Holding
the value *is* the permission:

```canon
Path("./data.json").File()?.read()?.print()
```

Formally: when a function performs an effect, the value carrying that
effect appears in its signature —

```canon
save = (Database * User) -> Result<Unit, DbError>
```

— and there is no other way to reach the effect. No globals, no
singletons, no ambient authority. The one exception is deliberate:
`print = (String) -> Unit` writes to stdout with no token, lowered
against `wasi:cli/stdout`.

## Suspension Is Inferred

A function is **suspending** if any of the following holds:

1. It is a body-less declaration in a [binding file](./compilation.md#binding-files)
   whose WIT entry is `async func` — the mechanical mapping gives it a
   `Future<T>` return type.
2. Its body consumes a `Future<T>` or iterates a `Stream<T>`.
3. It transitively calls a suspending function.

The compiler computes this set bottom-up over the call graph and lifts
affected functions as `async func(…)` in the emitted component world.
The entry point is lifted **async-stackful**, so suspension anywhere
beneath it yields to the host instead of trapping.

## Auto-Await

Wherever a `Future<T>` value is used in a position that expects `T` —
as a method receiver, as the operand of `?`, or as an argument whose
declared parameter type is `T` — the checker inserts the await:

```canon
main = () -> Unit {
    Url("https://example.com")?
        .get()?
        .body()?
        .print()
}
```

`get()` and `body()` return futures; the user writes a flat chain. The
two keywords other languages build their async story on simply do not
exist in the grammar. `Future<T>` and `Stream<T>` appear in **binding
signatures only** — ordinary code consumes the unwrapped `T`.

The consequences worth stating precisely:

- **No function coloring.** `f(x)` is `f(x)`; the calling convention is
  decided at codegen from the suspending set, not at the source level.
- **No executor choice.** The runtime is the host's implementation of
  WASI Preview 3's async ABI — fixed by the Component Model, not
  selectable by libraries.

## Concurrency

Fan-out is expressed as ordinary stdlib functions over futures
(`use canon/std/concurrent`), not keywords:

```canon
parallel("a".slowEcho(), "b".slowEcho()).toJsonArray().print()
race("a".slowEcho(), "b".slowEcho()).print()
```

```
parallel = <T>(Future<T> * Future<T>) -> Future<List<T>>
race     = <T>(Future<T> * Future<T>) -> Future<T>
```

`parallel` awaits both and returns results in argument order; `race`
returns the first and cancels the loser. The auto-await rule fires when
the composed future is consumed — still no keyword.

**Cancellation** has no primitive. It is a consequence of composition:
`race` cancels its losing branch; dropping a `Stream<T>` mid-iteration
stops it. To abandon a future, stop using it.

## Where Async Is Visible

Three places, all diagnostic:

1. **Binding files** — `Future<T>` in a generated signature is the
   ground truth of "this interface suspends".
2. **Type errors** — pathological cases can surface `Future<T>` in a
   message before auto-await resolves it.
3. **`canon inspect`** — shows which functions the compiler marked
   suspending.

Day-to-day code sees none of it. See the [Async chapter of the
Tour](../tour/async.md) for the design rationale and comparisons.
