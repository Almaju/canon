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
Path("./data.json")
    -> File?
    -> Read?
    -> Print
```

When a function performs an effect, the value carrying that effect
appears in its signature:

```canon
Saved = (Database * User) => Result<Unit, DbError>
```

There is no other way to reach the effect. No globals, no singletons,
no ambient authority. The one exception is deliberate:
`Print = (String) => Unit` writes to stdout with no token, lowered
against `wasi:cli/stdout`.

## Dependencies Thread Explicitly

A value carrying an effect appears in the signature of every function
that touches it, **and at every call site that passes it on**:

```canon
SqliteConnection => Main {
    SqliteConnection -> Query
}

SqliteConnection => Query {
    …
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
2. Its body consumes a `Future<T>`.
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
the grammar. `Future<T>` and `Stream<T>` appear in **binding signatures
only**; ordinary code consumes the unwrapped `T`.

Two precise consequences:

- **No function coloring.** `f(x)` is `f(x)`; the calling convention is
  decided at codegen from the suspending set, not at the source level.
- **No executor choice.** The runtime is the host's implementation of
  WASI Preview 3's async ABI, fixed by the Component Model, not
  selectable by libraries.

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

(The runtime fixtures exercise these through `slowEcho`, a camelCase
foreign binding to the async test bridge — camelCase means foreign;
`Parallel`/`Race` themselves are the language surface.)

**Cancellation** has no primitive. It is a consequence of composition:
`Race` cancels its losing branch. To abandon a future, stop using it.

## Where Async Is Visible

Three places, all diagnostic:

1. **Binding files**: `Future<T>` in a generated signature is the
   ground truth of "this interface suspends".
2. **Type errors**: pathological cases can surface `Future<T>` in a
   message before auto-await resolves it.
3. **`canon inspect`**: shows which functions the compiler marked
   suspending.

Day-to-day code sees none of it. See the [Async
chapter](../learn/async-without-keywords.md) for the working
introduction.
