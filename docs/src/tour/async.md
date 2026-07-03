# Async

Async is the most fashionable bug in modern language design. Rust, JavaScript,
Python, C#, Swift — every mainstream language eventually adopts `async` /
`await`, and every one of them ends up with the same complaints: function
coloring, ecosystem fracture, executor wars, lifetimes that don't quite work
the same on this side of the boundary. The diagnosis isn't subtle. The
machinery is real, but it has been welded to the *syntax* of the language,
and once syntax is involved every function has to declare which world it
lives in.

Canon takes a different position: **async is a property of types, never of
syntax**. There is no `async` keyword. There is no `.await`. There is no
function color the programmer has to track. You write synchronous-looking
code, the compiler infers everything else.

This chapter explains what that means in practice, what the rules are, and
where the seams are.

## The Two Things People Mean by "Async"

When practitioners say "I don't like async" they are usually conflating
two different things:

1. **Asynchronous *execution*** — the fact that some operations don't
   complete instantly. Reading a file, talking to a network, waiting on
   a timer. This is a property of the *world*, not the language. WebAssembly
   in particular has no way to block a host call without cooperation; if you
   want concurrency on the WASM Component Model, you must lower async
   imports through the canonical ABI's `async-lower` rule. You don't get
   to opt out.

2. **The `async` *keyword*** and its ecosystem — `Future` trait objects,
   `Pin<Box<dyn Future>>`, `tokio` vs `async-std` vs `smol`, executor
   selection, `Send` bounds on futures, "this function is sync but I need
   to call it from async context, what do I do." This is a property of
   the *language*, and it is optional.

Canon accepts (1) — we ship to WASM, we have no choice — and rejects (2)
as much as the underlying machinery permits.

## The Rules

The full rule is short enough to fit on one page:

1. **The user never writes `async`. The user never writes `await`.** These
   keywords do not exist in the Canon grammar.

2. **Async-ness enters the program through one door:** a [binding file](
   ./extern.md). If the WIT interface declares `async func read(…) -> …`,
   the mechanical WIT → Canon mapping gives the binding a `Future<T>`
   return type. That is the *only* way `Future` ever appears in source.

3. **`Future<T>` and `Stream<T>` are real types**, and the type checker
   knows about them. But you almost never write them yourself, because:

4. **The compiler auto-inserts `await`.** Wherever a `Future<T>` value is
   used in a position that expects `T` — as a method receiver, as the
   operand of `?`, or as a function argument whose declared parameter
   type is `T` — the checker rewrites the expression to insert an
   implicit await. The user sees a flat method chain. All three
   positions are handled today; see `src/checker/auto_await.rs` for the
   exact rules.

5. **Suspension propagates automatically.** A function is *suspending* if
   (a) it is a body-less declaration in a binding file whose WIT entry is
   `async func`, (b) its body contains an `Expr::Await` (inserted by rule
   4), or (c) it transitively calls a suspending function. The compiler
   computes this bottom-up and lifts the affected functions as `async
   func(…)` in the emitted Component Model world.

That's the entire model. There is no syntax to learn, no annotation to
write, no executor to pick.

## What This Looks Like in Source

This program performs an HTTP GET and prints the response body. Under the
hood, `wasi:http/outgoing-handler` is an async interface — every step of
the chain returns a `Future`:

```canon
main = () -> Unit {
    Url("https://example.com")?
        .get()?
        .body()?
        .print()
}
```

There is no `async fn`. There is no `.await`. The user's code looks
identical to a synchronous version of the same program. The compiler:

- Sees that `get()` returns `Future<Result<Response, HttpError>>`.
- Inserts an implicit await before `?` extracts the `Result`.
- Sees that `body()` is itself an async method on `Response`.
- Inserts another implicit await before the next `?`.
- Walks up the call graph and marks `main` as suspending.
- Lifts `main` to `async func(…)` in the emitted component world.

You can read the program as if it were synchronous. The machinery underneath
matches WASI Preview 3's async ABI exactly. Nothing leaks into the surface
language.

## Why Not `async` / `await`?

Three reasons, in increasing order of importance.

### Function Coloring

In Rust, JavaScript, Python, every function call site has to know whether
the callee is `async` or not, because the calling convention is literally
different: an async function returns a `Future` (or `Promise`, or
coroutine), not the value, and you must either `.await` the result or
forward it. This means:

- `map`, `filter`, `reduce` need async variants.
- A trait declared sync cannot be implemented async, and vice versa.
- A library that didn't think about async on day one needs a parallel
  async API on day two.
- The phrase "this function should be sync but it has to call this async
  thing" generates an industry of workarounds: `block_on`, `spawn_blocking`,
  threadpools, `Handle::current()`, and so on.

Canon doesn't have this problem because the calling convention at the
source level is the same for both cases. `f(x)` is `f(x)`. The compiler
makes the calling convention right at codegen time, based on whether `f`
ended up in the suspending set.

### Ecosystem Fracture

The Rust ecosystem split into `tokio`, `async-std`, and `smol` because
the *runtime* — the thing that polls futures — is not part of the
language. Library authors have to pick (or paper over the difference with
feature flags), and downstream consumers inherit the choice.

Canon has no library-author-visible runtime. The runtime is `wasmtime`'s
implementation of WASI Preview 3's async semantics, which is fixed by the
Component Model spec, not by the language. There is no executor to pick
because there is no executor anyone in Canon's world chooses.

### The Slogan Doesn't Match Reality

The pitch for `async` / `await` is "look how clear it is — you can see
exactly where the suspension points are." The pitch is wrong. In practice,
the suspension points are *every* method call that returns a `Future`, and
the only thing the keyword does is force the user to type it. Awareness
doesn't increase; ceremony does.

What you actually need to reason about — *can this function block? does it
require an executor? does it propagate cancellation?* — is captured by
**the type signature**, not by a keyword somewhere in the body. Canon
keeps the type signature (the `Future<T>` in the binding) and drops the
keyword.

## Comparison

| Language | Source-visible | Function coloring | Runtime choice |
|---|---|---|---|
| Rust | `async fn`, `.await` | Yes | tokio / async-std / smol |
| JavaScript | `async`, `await` | Yes (in practice) | platform-fixed |
| Go | none — goroutines + channels | No (sync surface) | green-thread scheduler in every binary |
| Canon | none | No (sync surface) | WASI Preview 3 (host-provided) |

Canon is in the "Go column" on developer experience — synchronous-looking
code, no keywords — without paying Go's price (a green-thread scheduler
baked into every binary). The win comes from targeting WASM exclusively:
the Component Model gives us async at the *ABI*, so we don't need to
recreate it in the runtime. The cost is that we accept the Component
Model's semantics; we don't get to invent our own.

## Parallelism, Spawning, and "Wait for Many"

A common follow-up: *if there's no `async` keyword, how do I express "fire
off two HTTP requests in parallel, then combine the results"?*

The answer, consistent with [domain-first design](./effects.md), is
**as methods on the futures themselves, not keywords** — the same
commutative method-call shape as everything else in Canon:

```canon
"a"
    .slowEcho()
    .parallel("b".slowEcho())
    .toJsonArray()
    .print()
"a"
    .slowEcho()
    .race("b".slowEcho())
    .print()
```

`a.parallel(b)` fans out, awaits both, and returns the results in
receiver-then-argument order as a `Future<List<T>>`; `a.race(b)`
returns the first to finish and cancels the loser. Both sides must
produce the same payload type:

```
parallel = <T>(Future<T> * Future<T>) -> Future<List<T>>
race     = <T>(Future<T> * Future<T>) -> Future<T>
```

These are not language features — they're combinators over futures,
entered through the receiver like any other Canon call. There is no
bare call form (`parallel(a, b)` is a compile error steering you to
the method spelling). The user never writes `await` on the result;
the auto-await rule fires the moment the composed future is used in a
position that expects its payload. The surface remains keyword-free.

> Implementation notes:
>
> `a.parallel(b)` joins two subtasks to a fresh waitable-set, loops on
> `waitable-set.wait` until both events fire, then builds a `List<T>`
> with the results in receiver-first order. `a.race(b)` waits for the
> first event and emits `canon.subtask.cancel` on the losing branch
> (the cancel is declared with `async_ = false`, which is permitted
> because `run` is lifted async-stackful). Both are recognised by name
> in the codegen — see `compile_parallel` / `compile_race` in
> `src/codegen/wasm/mod.rs` — and emit the canonical-ABI multi-subtask
> wait sequence inline; no host bridge is needed. Pinned by
> `tests/runtime/parallel_two_echoes.can` and
> `tests/runtime/race_two_echoes.can`.

## Streams

`Stream<T>` is to `List<T>` as `Future<T>` is to `T`. A stream is a
sequence of values produced over time. The same auto-detection rule
applies: a method that returns a `Stream<T>`, used at a position that
expects iteration, becomes a suspending iteration loop.

A hypothetical `tail -f`, where `lines()` would return
`Stream<String>`:

```canon
Path("./log.txt").File()?.lines().each((String) -> Unit {
    String.print()
})
```

The user writes `.each`. The compiler sees `Stream<String>` and generates
the Component Model stream-poll loop. There is no `for await … of`.

> Status: the stdlib surface is declared in
> `packages/canon/std/src/stream.can` (`map`, `filter`, `take`, `concat`,
> `toList`, `toString`) and the checker accepts it. Codegen for stream-
> carrying imports is the open piece: `build_extern_component_params`
> currently returns `None` on `Stream<T>` params/returns so the imports
> are silently dropped. The full plan is in `STREAMING.md` (slice 1b
> routes Stream-using programs through `wit_component::ComponentEncoder`
> instead of the hand-rolled type section). Until that lands, a program
> using stream combinators fails to link at runtime.

## Cancellation

The Component Model has `subtask.cancel`. Canon doesn't expose it as a
primitive — cancellation is a consequence of using `Race<…>` (the losing
branches are cancelled) or of dropping a `Stream<T>` mid-iteration. There
is no `cancel()` method to call directly. If you want a future to be
abandoned, you stop using it.

This is consistent with the rest of the language: control flow is
expressed through types and dispatch, not through imperative operations
on objects.

## Where the User Actually Sees Async

In normal Canon code: **nowhere**. The keyword doesn't exist, the
`Future<T>` and `Stream<T>` types are inferred from binding signatures and
collapsed at use sites, and the runtime is fixed.

Three places it leaks:

1. **Binding files** — if you're authoring or reading a binding file,
   you'll see `Future<T>` in return types. This is the canonical "what is
   async about this interface" surface.
2. **Type errors** — if you store a `Future<String>` in a place where the
   inferred type couldn't be unified with `String`, the error message
   will mention `Future`. (In practice the auto-await rule fires before
   the error reaches the user, but pathological cases exist.)
3. **`canon inspect`** — when you ask the compiler what it inferred, you
   can see which functions are suspending and which aren't.

For day-to-day code, the model is what it should be: a sequence of method
calls that read top-to-bottom, with the machine doing the bookkeeping.
