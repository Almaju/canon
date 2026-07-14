# The Philosophy

Most languages are designed by addition: a feature is useful, so it goes
in. Canon is designed by **subtraction**: a choice is discretionary, so
it goes out. Every idea on this page is an application of one rule —

> Wherever a choice is discretionary, the compiler removes the choice or
> enforces one answer.

Read this page as a voyage: each stop removes something you thought a
language needed, and shows what grows in the space it leaves behind.

## One Way to Do Everything

Give ten programmers a problem and most languages let them hand back
ten different-looking programs — braces here or there, `if` or `match`,
fields in whatever order they came to mind. None of those differences
mean anything. They are noise that reviews argue about and diffs drown
in.

Canon deletes the noise at the root:

- **Ordering is never yours to choose.** Product fields, union
  variants, declarations in a file, dispatch arms — everything whose
  order carries no meaning must be in alphabetical order, enforced by
  the compiler. (Where order *does* carry meaning — call operands, list
  elements — it is never touched.)
- **Formatting is part of the language.** There is one canonical
  layout, `canon check --fix` produces it mechanically, and a file that
  deviates from it does not compile. There is no style guide because
  there is nothing left to have an opinion about.
- **Every call has one spelling.** `B(a)`, `a.B()`, and `a -> B` would
  be three ways to write one call, so the formatter rewrites them all to
  the canonical form: values flow through pipes, literals are born in
  the parens.
- **One construct per job.** There is no `if`/`else` *and* `match` —
  there is dispatch. No `while` *and* `for` *and* recursion — there are
  collection operations and recursion.

The payoff compounds: two programmers writing the same program produce
the **same bytes**. A diff shows exactly the lines whose meaning
changed. "Moved a declaration" disappears as a category of change, and
so does every argument it used to start.

## Types Are the Only Names

Names are the leakiest abstraction in programming: a variable named
`userList` that holds a map, a function named `validate` that also
saves, a parameter named `data`. Names lie; types don't. So Canon keeps
only the names the compiler can check — **type names** — and removes
every other kind:

- **No local variables.** Values thread through the `->` pipe. To name
  an intermediate value, give it a *type* — the name is then checked,
  not decorative.
- **No parameter names.** A function's input is a product of types, and
  the body refers to each value by its type. Two inputs of the same
  type must be told apart by a newtype (`OtherUser = User`) — and that
  newtype documents *why* there are two.
- **No function names.** Every callable is a constructor, named after
  the type it produces. An operation that transforms a value takes a
  **result newtype** named for what it did: inserting into a `Map`
  yields an `Inserted` (`Inserted = Map`). The naming treadmill —
  `fromX`, `toX`, `parseX`, `tryX` — disappears because there is
  nothing left to name.
- **No comments.** If code needs explaining, the fix is a better type,
  not prose the compiler can't check and the next edit won't update.

One consequence deserves its own line: **conversion is construction.**
Turning a value into a `T` is spelled by constructing a `T` —
`String(42)`, `Int("42")?` — because that is what it is. And when a type
declares its own validating constructor, invalid values of it cannot
exist: callers are forced through the validation by the type system,
which is Canon's entire encapsulation story.

## Branching Is Dispatch

Canon has exactly one way to make a decision: pipe a union value into
one handler per variant. The handlers must cover every variant, in the
union's order, with no wildcard — adding a variant breaks every
dispatch that forgot it, at compile time.

This is not a pattern-matching feature bolted onto an expression
language; it is the algebra taken literally. A union is a sum, a
handler group is a product, and dispatch is the isomorphism between
`(A + B) -> R` and `(A -> R) * (B -> R)`. Even `Bool` is an ordinary
union (`False + True`), and `if` is just dispatch on it — which is why
`if` does not exist.

The same construct scales up: an HTTP route table is literal dispatch
on the path string, a web app's reducer is dispatch on the message.
There is no router DSL and no state-machine library, because the one
branching construct already is both.

## Having a Value Is Having the Capability

Most languages answer "what can this function touch?" with nothing: any
line of code can open a socket, read a file, or check the clock. Canon
answers it with the signature. Effects are not annotations or
permissions — they are **values**:

- Reading a file requires a `File`, which only a `Path` can produce,
  which only a `String` can produce. The construction chain *is* the
  access control.
- Dependencies thread explicitly. A function that queries a database
  takes the connection as an input; there are no globals, no
  singletons, no service locators, and no hidden filling-in of an
  omitted argument.
- Effects can leave **evidence**. A write returns a `Written` value; a
  downstream function that takes `Written` instead of `Path` demands
  proof the write happened — sequencing enforced by types, with no new
  machinery.

The guarantee survives compilation: a Canon program is a WebAssembly
component with **no ambient authority**, so its reach is bounded by the
capabilities its host hands it. That makes even machine-generated code
safe to run — the sandbox is the type discipline, continued at the ABI.

## Async Is a Property of Types, Not Syntax

`async` and `await` are bookkeeping the compiler can do. In Canon,
whether a function suspends is *inferred*: a host binding whose
interface is asynchronous returns a `Future<T>`, and wherever a
`Future<T>` meets a position expecting `T`, the compiler inserts the
await and propagates suspension up the call graph. You write
straight-line pipes; there is no function coloring, no executor to
pick, and no keyword to forget. Concurrency is two combinators over the
futures you already have — `Parallel` fans out, `Race` returns the
winner and cancels the loser.

## The Structure Is the Declaration

Configuration files restate what a project's shape already says, so
Canon has none:

- **No import statement.** Files are named after the type they declare
  (`http-server.can` ⇄ `HttpServer`), so a reference *is* an import:
  mentioning a name loads its file, and the same rule reaches the
  standard library. Ambiguity is a hard error, not a shadowing rule.
- **No manifest.** A directory with `src/main.can` is a package; a
  directory of packages is a workspace; a WIT file under `wit/` is an
  external import; a vendored directory under `deps/` is a dependency,
  and its name is the version pin. There is no `canon.toml` and no
  lockfile, because the file tree already says everything they would.
- **No entry-point registration.** A program is whatever its types say
  it is: an arrow returning `Exit` is a CLI command, an arrow returning
  `Response` is an HTTP service, a `Model => Html` view (with its init
  and update) is a browser app. Signatures select the world; nothing is
  named `main`.

## Small Decisions, Deliberately Strange

A few choices look wrong until the principle behind them is visible:

- **Indexing is 1-based**, everywhere: `At(1)` is the first element,
  `ByteAt(1)` the first byte, `.1` the first positional field — one
  origin for every kind of access, and `Substring(a * b)` is inclusive
  on both ends. There is no second convention to reconcile.
- **"Alphabetical" is byte-wise**, case-sensitively — the one ordering
  that needs no locale, no configuration, and no judgment.
- **Errors and absence are different things.** `Result` means *failed*,
  `Option` means *not there*; conflating them is how `null` happened.

## What Canon Refuses, and What Replaces It

| Removed | Replaced by |
|---|---|
| `if` / `else`, `match`, `switch` | dispatch on a union |
| `while`, `for` | collection operations and recursion |
| `let`, local variables | the `->` pipe and newtypes |
| parameter names | input products of distinct types |
| function names | constructors and result newtypes |
| comments | types and names the compiler checks |
| `import` / `use` | references resolved by file naming |
| package manifest, lockfile | directory structure (`src/`, `wit/`, `deps/`) |
| `async` / `await` | inferred suspension and auto-await |
| `null` | `Option<T>` |
| exceptions | `Result<T, E>` and `?` |
| style guides, formatter config | one canonical form, enforced at compile time |
| permission systems | capabilities as values |

Nothing on the left was forgotten. Each was weighed and found to be a
choice the compiler could make better than a person — and the sections
above are what each removal bought.

The rest of this book shows the mechanics: the **Learn** chapters teach
each construct with runnable examples, and the **Specification** states
the precise rules.
