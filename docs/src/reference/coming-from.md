# Coming from Another Language

Nobody learns Canon first. The fastest way in is to map the habits you
arrive with onto Canon's handful of ideas — then notice which ones you
have to put down at the door. First, a taste: a union, two products,
and a function that branches by dispatch.

```canon
Circle = Radius

Shape = Circle + Square

Square = Side

Shape => Area {
    Shape -> (
        * Circle => Area { Circle -> Squared -> Product(3.14) }
        * Square => Area { Square -> Squared }
    )
}
```

No `if`, no `switch`, no variables, no comments — and the arms are
alphabetical because the compiler insists. If that makes you curious
rather than annoyed, you're in the right place.

## From TypeScript

You already think in unions — Canon takes them seriously.
**You'll like:** discriminated unions as the *default* data model,
exhaustive dispatch with no `default:` to silence, and a browser
target that is the Elm architecture you've been approximating.
**You'll learn:** you never needed `let` — chain, and name
intermediate values with types. **Watch out:** Canon is nominal, not
structural; there is no `any`, no inference of signatures, and
`Option<T>` replaces `null`/`undefined` — and you must handle it.

## From Python

The surface is small and readable; the discipline is new.
**You'll like:** very little ceremony, a pipe that reads like
chaining, backtick format strings, and sorted immutable `Map`/`Set`
out of the box. **You'll learn:** what an always-on type system buys —
declare a union and the compiler proves you covered every case, and
nothing mutates. **Watch out:** there's a compile step, no REPL, no
duck typing, no loops (recursion and `Mapped`), and a young library
ecosystem.

## From Rust

The most direct port — you have the mental model already.
**You'll like:** `+` is `enum`, `*` is `struct`, `Result`/`Option`/`?`
work as expected (error unions spell inline), dispatch is `match` — all
without lifetimes or borrow sigils. **You'll learn:** formatting as a
language rule, capabilities as values, and `From`/`Into` collapsed
into conversion-is-construction. **Watch out:** no `let` at all, no
macros, no comments, and `async`/`await` vanish — suspension is
inferred.

## From Go

If you like Go's smallness and one-true-format, Canon takes both
further. **You'll like:** a language you hold in your head, one
portable artifact, everything public, and formatting debates ended in
the *language*, not a tool. **You'll learn:** sum types and exhaustive
matching — `Result` + `?` instead of `if err != nil`, `Option` instead
of `nil`. **Watch out:** no goroutines or channels as keywords
(concurrency is combinators), no mutation, no loops — and yes, no
comments.

## So… Is Canon for You?

You'll enjoy it if you like languages that make decisions *for* you
and mean it — if enforced formatting sounds like a relief and "illegal
states are unrepresentable" is a goal, not a slogan. You'll fight it
if you love a REPL, reach for `any`/`nil`/reflection, want comments,
or need a mature package ecosystem today.

Either way the next step is the same: walk the **Learn** chapters
(start at [Types & Values](../learn/types-and-values.md) — they run in
the browser) and poke at the
[examples](https://github.com/Almaju/canon/tree/main/examples). An
afternoon is enough to know whether it's your kind of strange.
