# Coming from Another Language

Nobody learns Canon first. You arrive carrying habits from somewhere
else, and the fastest way in is to map those habits onto Canon's
handful of ideas — then notice which ones you have to put down at the
door.

Canon is small on purpose. It compiles straight to WebAssembly
Components, and its closest mainstream cousin is **Elm**: pure
functions, tagged unions, a pipe operator, effects modeled as values.
But the flavor changes depending on where you're standing, so pick your
language below. Each stop lists **what you'll like**, **what you'll
learn**, and **what might trip you up**.

First, a taste. Here's the whole shape of the language in one snippet —
a union, two products, and a function that branches by dispatch:

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

No `if`, no `switch`, no local variables, no comments, and the two arms
are in alphabetical order because the compiler insists. If that snippet
makes you curious rather than annoyed, you're in the right place.

## Coming from TypeScript

You already think in unions — Canon just takes them seriously.

**What you'll like.** Discriminated unions are the *default* way to
model data, and dispatch on them is exhaustive with no `default:` case
to silence the compiler — add a variant and every branch that forgot it
stops compiling. If you've built front-ends with a `Model` / `update` /
`view` loop, the browser target will feel like home; it's the Elm
architecture, typed. And the types are sound: no `any`, no unchecked
casts, no `strictNullChecks` to remember to switch on.

**What you'll learn.** That you never needed local variables. There is
no `let` or `const` — you chain methods and, when you want to name an
intermediate value, you give it a *type* instead. Conversion stops being
a zoo of `String(x)`, `.toString()`, `Number()`, `JSON.parse` and
becomes one idea: constructing the target type (`Int(s)?`, `x.ToJson()`).

**What might trip you up.** Canon is nominal, not structural — two
records with the same fields are different types unless you say
otherwise, so there's no `{ x: number }`-shaped duck typing. There's no
`any` escape hatch, signatures aren't inferred (you write them), and
`null` / `undefined` are replaced by `Option<T>` that you must actually
handle. Also: no comments. The compiler rejects them.

## Coming from Python

The surface area will feel familiar; the discipline will not.

**What you'll like.** Canon is *small* — a handful of concepts, very
little ceremony, and a readable pipe (`->`) that reads a lot like
chaining. Backtick format strings (`` `hello, {name}` ``) plus the
string-literal `Json` and `Html` types with `{hole}` interpolation will
feel like f-strings that happen to be first-class language features. And
the standard library gives you sorted, immutable `Map` and `Set` out of
the box.

**What you'll learn.** How much a type system can do for you when it's
always on. Instead of tagging dicts with a `"kind"` key and branching on
`isinstance`, you declare a union and dispatch — and the compiler proves
you covered every case. Everything is immutable, so a whole category of
"who mutated this?" bugs simply can't occur.

**What might trip you up.** There's a compile step and no REPL-driven
dynamism — types are explicit and checked before anything runs. No duck
typing, no monkey-patching, no mutation, and no `for`/`while` loops
(you use recursion and collection methods like `.Mapped`). The library
ecosystem is young compared to PyPI. And, again: no comments.

## Coming from Rust

The most direct port — you already have most of the mental model.

**What you'll like.** The type algebra is the same shape: `+` is your
`enum`, `*` is your `struct`, and `Result<T, E>` / `Option<T>` / `?`
work as you'd expect (error unions can even be spelled inline). Dispatch
is `match` by another name. The project layout mirrors Cargo
(`src/main.can` + `build/`, no manifest). And you get ADTs and
pattern matching *without* lifetimes or borrow sigils — ownership is
inferred, so `'a`, `&`, and `&mut` are gone.

**What you'll learn.** That formatting can be a language rule, not a
tool you run: wherever order is discretionary — fields, variants,
arguments, declarations, arms — it's alphabetical, and two people
writing the same program emit identical bytes. You'll also meet
capabilities: effects flow through values you hold (`File`, `Database`),
so a signature tells you what a function can touch. And `From`/`Into`
collapse into a single idea — conversion is construction.

**What might trip you up.** No local variables (`let` is gone, not just
discouraged), and no macros or comments -- string interpolation is a
backtick format string (`` `x is {x}` ``), not a `format!` macro.
`async` / `await`
disappear from the source entirely — suspension is inferred and
concurrency is combinator methods (`.parallel`, `.race`). It's Rust's
type discipline with even less room to improvise.

## Coming from Go

If you like Go for its smallness and its one-true-format, Canon takes
both further than you may be ready for.

**What you'll like.** It's a small language you can hold in your head,
it compiles fast to a single portable artifact (a `.wasm` Component,
much like a static binary), everything is public with no export
ceremony, and `canon check --fix` ends formatting debates the way `gofmt` did —
except the canonical form is baked into the *language*, so there's
nothing to disagree about in the first place.

**What you'll learn.** Sum types and exhaustive matching — the thing Go
pointedly left out. Instead of `if err != nil` threaded through every
call, you get `Result<T, E>` and the `?` operator; instead of `nil`,
`Option<T>`. Generics are unremarkable and everyday, and effects are
capabilities you pass explicitly rather than reach for globally.

**What might trip you up.** No `nil`, no `if err != nil` (it's `?`), no
goroutines or channels as keywords (async is inferred; concurrency is
combinators), no mutation, no loops, and interfaces aren't quite what
you know — a "trait" is just a callable type signature. Coming from Go's
deliberate plainness, Canon will feel like *more* opinions, not fewer —
just enforced instead of conventional. And, as everywhere: no comments.

## So… Is Canon for You?

You'll enjoy Canon if you like languages that make decisions *for* you
and mean it — if enforced formatting sounds like a relief, if
"illegal states are unrepresentable" is a goal rather than a slogan, and
if you're happy trading a familiar escape hatch for a guarantee.

You'll fight it if you love a REPL, reach often for `any` / `nil` /
reflection, want comments, or need a mature package ecosystem today.

Either way, the fastest next step is the same: walk the **Learn**
chapters (start at [Types & Values](../learn/types-and-values.md) --
they run in the browser), and poke at the
[`examples/`](https://github.com/Almaju/canon/tree/main/examples). The
language is small enough that an afternoon is enough to know whether it's
your kind of strange.
