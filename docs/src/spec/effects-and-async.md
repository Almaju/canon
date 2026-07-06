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

When a function performs an effect, the value carrying that effect
appears in its signature:

```canon
save = (Database * User) => Result<Unit, DbError>
```

There is no other way to reach the effect. No globals, no singletons,
no ambient authority. The one exception is deliberate:
`print = (String) => Unit` writes to stdout with no token, lowered
against `wasi:cli/stdout`.

## Dependencies Thread Implicitly

A value carrying an effect must appear in the signature of every function
that touches it — but it need not be *repeated* at every call. When a
constructor call omits an argument the callee requires, and the enclosing
function holds exactly one value of that type, the compiler supplies it:

```canon
SqliteConnection => Main {
    Query()
}

SqliteConnection => Query {
    …
}
```

`Query()` is rewritten to `SqliteConnection -> Query` before the checker
runs — identical to writing it out by hand. The dependency flows down the
call tree without being named at each hop.

This is implicit *threading*, never implicit *authority*. `Query` still
declares `SqliteConnection => Query`; the capability stays in the type,
and a function that does not name a dependency cannot reach it — a `Main`
without a `SqliteConnection` parameter cannot call `Query()` at all, and
the checker reports the same missing-argument error it always did. The
signature remains the whole truth of what a function requires; only the
plumbing is inferred. It is the discretionary-choice rule applied to
arguments: when exactly one value can fill a slot, naming it is ceremony,
so the compiler removes it.

Resolution is unambiguous or it does not fire. Matching is by declared
type name against the enclosing parameters (and receiver):

- **Exactly one** in-scope value of the needed type → supplied.
- **Zero** → nothing is conjured; the usual missing-argument error stands.
- **Two or more** of that type in scope → ambiguous; the caller passes it
  explicitly.

A value already passed fills its slot, and the remaining slots are filled
from scope, so a call may mix explicit and inferred arguments. Matching is
by exact declared type — alias/newtype widening is not (yet) inferred, and
the caller's own signature is never widened: inference threads what a
function already declares, it never adds a requirement behind your back.

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
        .body()?
        -> Print
}
```

`get()` and `body()` return futures; the user writes a flat chain. The
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

Fan-out is expressed as combinator methods on the futures themselves,
not keywords: the same commutative method-call shape as every other
Canon call:

```canon
"a"
    .slowEcho()
    .parallel("b".slowEcho())
    .Json()
    .print()
"a"
    .slowEcho()
    .race("b".slowEcho())
    .print()
```

```
parallel = <T>(Future<T> * Future<T>) => Future<List<T>>
race     = <T>(Future<T> * Future<T>) => Future<T>
```

`a.parallel(b)` awaits both and returns results in receiver-then-argument
order; `a.race(b)` returns the first and cancels the loser. There is no
bare call form: `parallel(a, b)` is a compile error. The auto-await
rule fires when the composed future is consumed, still with no keyword.

**Cancellation** has no primitive. It is a consequence of composition:
`race` cancels its losing branch; dropping a `Stream<T>` mid-iteration
stops it. To abandon a future, stop using it.

## Where Async Is Visible

Three places, all diagnostic:

1. **Binding files**: `Future<T>` in a generated signature is the
   ground truth of "this interface suspends".
2. **Type errors**: pathological cases can surface `Future<T>` in a
   message before auto-await resolves it.
3. **`canon inspect`**: shows which functions the compiler marked
   suspending.

Day-to-day code sees none of it. See the [Async chapter of the
Tour](../guide.md#async) for the design rationale and comparisons.
