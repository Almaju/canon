# Canon for AI

Most languages were designed for humans typing into an editor, and only
later asked to tolerate code written by a machine. Canon's constraints —
adopted for human clarity — happen to make it an unusually good target for
**AI code generation and agentic development**. The same rules that give a
person one obvious way to write a program give a language model a small,
regular target and a compiler that turns its mistakes into precise,
fixable errors.

None of this is retrofitted. Every property below is a design decision
already documented elsewhere in this book; this page only connects them to
the way models write code.

## One canonical form shrinks the target

A language model generates code by sampling a sequence of tokens. The more
stylistic freedom a language grants — brace placement, import ordering,
where to declare a helper, `if`/`else` versus `match` — the more of that
sampling budget is spent choosing between spellings that mean the same
thing, and the more ways there are to drift into something subtly
non-idiomatic.

Canon removes the choices. Wherever ordering is discretionary, the
compiler enforces alphabetical order: product fields, union variants,
function declarations, dispatch arms. Every call has one spelling —
`canon fmt` rewrites `B(a)`, `a.B()`, and `a * c -> B` all to
`a -> B(c)`. Two programmers — or two model samples — writing the same
program produce the **same bytes**.

For a model, that means the correct output is a far narrower distribution:
there is usually one canonical answer, not a cloud of stylistic variants to
pick among. For the human reviewing the result, it means the diff is
minimal — a generated edit changes exactly the lines it must, because the
formatter pins everything else in place.

## A surface small enough to hold in context

Canon is deliberately tiny. There is no `let`, no `if`/`else`, no `while`
or `for`, no local variables, no comments, and no import statement.
Branching is dispatch on a union; iteration is collection methods and
recursion; effects are values passed as capabilities.

```canon
True() -> (
    * False => Unit { "no" -> Print }
    * True  => Unit { "yes" -> Print }
)
```

A smaller grammar is fewer constructs to confuse, fewer keywords to
hallucinate, and a specification that fits comfortably inside a model's
context window alongside the task. The model spends its reasoning on *what
the program should do*, not on which of five loop forms to reach for.

## No imports, so no hallucinated paths

Import statements are one of the most reliable sources of model error: a
wrong path, a symbol that doesn't exist, a package invented wholesale
because it *sounds* like it should. Canon has no `use` keyword and no
import statement at all. A reference to a name the file doesn't define
resolves automatically — name to file — through the local tree, then
bindings, dependencies, and the bundled standard library.

```canon
Unit => Program {
    Path("./data.json")
        -> File?
        -> Read?
        -> Print
}
```

`Path`, `File`, `Read`, and `Print` are never imported; the compiler finds
them. An entire category of generation mistake simply cannot be expressed,
because there is no import line to get wrong.

## Types are the only names

Canon has no parameter names and no local bindings to keep consistent
across a function. A function's inputs are a product of types, referenced
in the body by their type names:

```canon
OtherUser * User => Ord {
    User.Birthday -> Compared(OtherUser.Birthday)
}
```

There is no naming convention for a model to invent and then contradict
three lines later, and no gap to open up between a name and what it holds.
The model reasons in types — which the checker verifies — instead of in
identifiers it must remember it chose. If code needs explaining, the fix is
a better type, not a comment the model has to keep in sync.

## The checker refuses ambiguity

Many things that other languages accept and let fail at runtime are hard
compile errors in Canon — and each one is a class of model mistake caught
at the earliest possible moment:

- **Dispatch is exhaustive.** There is no wildcard arm; every variant of a
  union must be handled. A forgotten case is a compile error, not a
  surprise in production.
- **No duplicate or dead code.** Duplicate dispatch arms and declarations
  unreachable from the entry point are hard errors — the model can't leave
  half-finished scaffolding behind.
- **Ordering is enforced.** Out-of-order fields, variants, or functions are
  errors (and `canon fmt` fixes them mechanically).
- **Capabilities are typed.** You can't read a file without a `File` value,
  which you can only obtain from a `Path`. Reaching for an effect the
  function wasn't handed doesn't typecheck.

Every one of these turns a plausible-looking-but-wrong generation into a
located, readable diagnostic instead of a silent bug.

## A tight, deterministic feedback loop

Agentic coding lives or dies on the quality of the loop: write, check,
read the error, fix, repeat. Canon's toolchain is built for exactly that.

The compiler is self-contained — source goes straight to a WebAssembly
component with no external toolchain invoked — so a check is fast and
reproducible. Errors carry precise spans. `canon fmt` is a fixpoint an
agent can run to normalize its own output. Tests are golden files and
typed `TestResult` constructors, so "did this change break anything?" has a
deterministic, machine-readable answer. The ground truth a model needs to
iterate against is fast, local, and unambiguous.

## Generated code is safe to run

When a model writes code, you eventually have to run it. Canon compiles to
a **WebAssembly component** with no ambient authority: effects arrive as
capabilities, and having the value *is* having the permission. A generated
program can only touch the filesystem, the network, or the clock if you
handed it that capability. Untrusted, machine-authored code runs inside a
sandbox by construction — its reach is bounded by the values you chose to
pass in, not by your trust in the model that wrote it.

## In short

The properties that make Canon pleasant for a person to read are the same
ones that make it tractable for a model to write:

| Design choice | Why it helps a model |
|---|---|
| One canonical form | Narrow target distribution, minimal diffs |
| Tiny surface area | Spec fits in context; fewer ways to be wrong |
| No imports | Whole class of path/symbol errors removed |
| Types are the only names | No naming state to keep consistent |
| Exhaustive, duplicate-free dispatch | Missing cases caught at compile time |
| Enforced ordering + `canon fmt` | Deterministic, mergeable edits |
| In-process compiler, golden tests | Fast, unambiguous feedback loop |
| Capabilities as values | Generated code is sandboxed by construction |

Canon wasn't designed *for* AI — it was designed to have one obvious way to
do everything. That it is also a good language to hand a model is the same
idea seen from the other side.
