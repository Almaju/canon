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
Unit => Program {
    Path("./data.json")
        -> File
        -> Read?
        -> String
        -> Print
}
```

When a function performs an effect, the value carrying that effect
appears in its signature:

```canon
Database = Int

DbError = String

Saved = Unit

User = String

Database * User => Result<Saved, DbError> {
    Unit() -> Ok
}
```

There is no other way to reach the effect. No globals, no singletons,
no ambient authority. The one exception is deliberate:
`Print = (String) => Unit` writes to stdout with no token, lowered
against `wasi:cli/stdout`.

## Dependencies Thread Explicitly

A value carrying an effect appears in the signature of every function
that touches it, **and at every call site that passes it on**:

```canon
Main = Query

Query = String

SqliteConnection = Int

SqliteConnection => Main {
    SqliteConnection -> Query
}

SqliteConnection => Query {
    "SELECT 1"
}
```

The signature declares the requirement; the pipe shows the flow. There
is no inferred filling of an omitted argument from the enclosing scope —
an earlier design supplied a missing dependency automatically whenever
exactly one in-scope value matched, but that made the call site's
spelling optional (`Query()` and `SqliteConnection -> Query` were the
same program), and the rule "wherever a choice is discretionary, the
compiler removes it" cuts against optional spellings hardest of all.
Since the inference could never be canonicalised by `canon check --fix` (the
formatter is purely syntactic and cannot see scope), the pipe is the one
spelling: a call names every value it consumes, and an omitted argument
is a plain missing-argument error.

## Suspension Is Inferred

A function is **suspending** if any of the following holds:

1. It is a body-less declaration in a [binding file](./compilation.md#binding-files)
   whose WIT entry is `async func`; the mechanical mapping gives it a
   `Future<T>` return type.
2. Its body consumes a `Future<T>` or iterates a `Stream<T>`.
3. It transitively calls a suspending function.

The compiler computes this set bottom-up over the call graph and lifts
affected functions as `async func(...)` in the emitted component world.
The entry point is lifted **async-stackful**, so suspension anywhere
beneath it yields to the host instead of trapping.

## Auto-Await

Wherever a `Future<T>` value is used in a position that expects `T`
(as a method receiver, as the operand of `?`, or as an argument whose
declared parameter type is `T`), the checker inserts the await:

```canon
Unit => Program {
    Url("https://example.com")?
        -> Fetched?
        -> Print
}
```

`Url` and `Fetched` return futures; the user writes a flat chain. The
two keywords other languages build their async story on do not exist in
the grammar. `Future<T>` appears in **binding signatures only**;
ordinary code consumes the unwrapped `T`.

Two precise consequences:

- **No function coloring.** `f(x)` is `f(x)`; the calling convention is
  decided at codegen from the suspending set, not at the source level.
- **No executor choice.** The runtime is the host's implementation of
  WASI Preview 3's async ABI, fixed by the Component Model, not
  selectable by libraries.

## Streams

A `Stream<String>` is a value that yields its chunks one pull at a time.
The chunks come from wherever the stream does: a host stream yields what
each host read returns — a chunk is not a line, and may end inside a
multi-byte character — and a list yields its elements.

```canon
Unit => Result<Program, IoError> {
    Stdin()?
        -> Mapped((String) => Uppercased { String -> Uppercased })
        -> Printed?
    Unit() -> Ok
}
```

Producers: `Stdin()` and `file -> Read` (both `Result`s whose `Ok` is
the stream), `Request.body()` in an HTTP handler, and `list -> Stream`
over a `List<String>`. Consumers:
`-> First` pulls one chunk as an `Option<String>`; `-> Folded(init *
lambda)` pulls every chunk into an accumulator, as a list's `Folded`
does; `-> String` drains the rest into one string; `-> Printed?` writes
each chunk to standard output as it is pulled, so the filter above
never holds more than one chunk. Transforms:
`-> Mapped(lambda)` applies the lambda to each chunk as it is pulled
and `-> Taken(n)` stops after `n` chunks. Nothing is read until a
consumer pulls, and a stream nothing pulls from is never read; the
host's handles are dropped when the stream ends. `Stream<T>` for any
other `T` is a checker error — see the [codegen
gaps](../reference/codegen-gaps.md).

## Concurrency

Fan-out is expressed as combinators over the futures themselves, not
keywords: the same pipe shape as every other Canon call:

```
Parallel = <T>(Future<T> * Future<T>) => Future<List<T>>
Race     = <T>(Future<T> * Future<T>) => Future<T>
```

`a -> Parallel(b)` awaits both and returns results in receiver-then-argument
order; `a -> Race(b)` returns the first and cancels the loser. There is no
bare call form: `Parallel(a * b)` is a compile error. The auto-await
rule fires when the composed future is consumed, still with no keyword.

(The runtime fixtures exercise these through `Slept`, the binding to
`wasi:clocks`' asynchronous `wait-for`; `Parallel`/`Race` themselves
are the language surface.)

**Cancellation** has no primitive. It is a consequence of composition:
`Race` cancels its losing branch; a stream nothing pulls from is never
read. To abandon a future, stop using it.

## Where Async Is Visible

Three places, all diagnostic:

1. **Binding files**: `Future<T>` in a generated signature is the
   ground truth of "this interface suspends".
2. **Type errors**: pathological cases can surface `Future<T>` in a
   message before auto-await resolves it.
3. **`canon inspect`**: shows which functions the compiler marked
   suspending.

Day-to-day code sees none of it. See the [Async
chapter](../tour/async.md) for the working
introduction.
