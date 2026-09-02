# Functions

## Declaration

Every callable is a **constructor**, named after the type it produces.
The declaration arrow is `=>`; writing `->` at a declaration site is a
parse error (`->` is the value-level pipe -- see
[Expressions](./expressions.md)). The anonymous form needs no name of
its own, because the return type *is* the name:

```text
Components => ReturnType {
    body
}
```

The components form a **product**: the input is a product type, written
with the same `*` used everywhere else. There are no commas, no
parameter names, no defaults:

```canon
Greeting = String

Line = String

Name = String

Greeting * Name => Line {
    Greeting -> Joined(Name)
}
```

The anonymous arrow is the **only** bodied declaration form in
canonical Canon. A named declaration whose name is just the constructed
type spells the name twice, so `canon check --fix` rewrites it to the
anonymous arrow (`Url = (String) => Result<Url, InvalidUrl>` becomes
`String => Result<Url, InvalidUrl>`); a named declaration whose name is
anything else is a checker error. The checker enforces the
boundary from both sides:

- A bodied declaration's name must be the **type it constructs**
  (modulo `Result`/`Option`/`Future` peeling and newtype chains).
  Anything else -- an arbitrary verb wearing PascalCase, like
  `Frobnicated = (Int) => Int` with no `Frobnicated` newtype anywhere
  -- is a checker error: *a name carries no information the types
  don't*.
- An arrow that constructs one of its own inputs is a **command**, and
  the types of a command cannot identify it -- insert, remove, and update
  all share `Map * String => Map` -- so a command takes exactly one other
  input, its **message**: a type declared for the operation, holding its
  arguments. `Insert = Key * Value` plus `Map * Insert => Map { ... }`. A
  message is a declared, non-primitive type that is not a part of the
  value it applies to (`Key` is a part of `Map`, so `Map * Key => Map` is
  an error); a message with no payload is a `Unit` newtype (`Clear =
  Unit`). Exact-name comparison only: a newtype input flowing into its
  base type's constructor (`Rest = Map` into a `Map` constructor) is a
  different type and an ordinary constructor.

The name must be PascalCase: a camelCase declaration is a checker error
everywhere except [binding files](./compilation.md).

- Components follow the [alphabetical rule](./ordering.md):
  `Greeting * Name => Line` is legal, `Name * Greeting => Line` is a
  compile error.
- Components must be distinct types; disambiguate duplicates with a
  newtype (`OtherUser = User`).
- Inside the body, each component is referenced by **its type name**:
  `Greeting` is the greeting value, `Name` the name value.
- `Unit` is the name of "no input": a nullary constructor is
  `Unit => X`, and call sites write `X()` -- the `Unit` is
  auto-supplied.
- There are no local variables.

## Commutative Calling

At the call site, **any component may pipe in on the left of `->`**;
the rest ride in the parentheses:

```text
Greeting("hi ") -> Line(Name("ada"))
Name("ada") -> Line(Greeting("hi "))
```

Both are the same call, a consequence of `*`'s commutativity: the piped
position is not privileged, it merely selects which component the
caller writes on the left. For arities above two, the remaining
components are passed as a product value:

```canon
0 -> Digits(Pos(1) * String)
```

### Applying a Message

A command is reached only through its message: the value pipes into the
message, and what rides in the parentheses builds it.

```text
Map() -> Insert(Key("a") * Value("1")) -> Remove("a")
todos -> Clear
Node.Rest -> Insert(Insert)
```

`map -> Insert(…)` applies `Map * Insert => Map` to `map`; the
expression's type is the command's return type (`Map`, or
`Result<Map, E>` for a fallible command, unwrapped by `?` as usual). A
value that already is the message passes through, which is the shape a
recursive body takes. Constructing the receiver's type around the
message (`map -> Map(Insert(…))`) is an error: one spelling.

### The Binding Rule

Commutative calling is a *syntactic* freedom, never a semantic
ambiguity. Arguments (including the receiver) bind to components by:

1. **Exact type match binds first.** A value typed `OtherUser` binds
   only the `OtherUser` component.
2. **Substitutability resolves what remains.** A bare `User` flows into
   an alias-compatible slot (`OtherUser`) only when exactly one
   unfilled component accepts it.
3. **Anything else is a compile error.** If two same-typed bare values
   could each fill two alias-related slots, the call is ambiguous and
   the caller must wrap one explicitly. For
   `Ord = (OtherUser * User) => Ord`, `alice -> Ord(bob)` is rejected:
   which value is the `OtherUser` decides `Less` versus `Greater`.
   Write `alice -> Ord(OtherUser(bob))`.

**Repeated components bind positionally.** A constructor over a fixed
repetition, such as `User^2 => Merged`, has positional components
(`.1`, `.2`, ...), so binding is positional too: the piped value fills
`.1`, remaining arguments fill `.2` and onward in the order written.
Commutative reordering does not apply, because position *is* the
identity of a repeated component. Use `T^N` when order is the honest
semantic (pairs, coordinates); use distinct newtypes when components
mean different things.

## Lambdas

One-off operations are lambda literals with a **full signature** (there
is no inference), passed wherever a matching function type is expected:

```canon
Numbers = List<Int>

Tripled = List<Int>

Numbers => Tripled {
    Numbers -> Mapped((Int) => Int { Int -> Product(3) })
}
```

Lambda syntax is declaration syntax with the parentheses kept and no
top-level name: the same `=>` arrow that declares every constructor.

## Operations Have No Names

There is no trait system and no shape declaration: a body-less
signature naming a type family is a checker error
(`… operations take result newtypes`). One name shared across many
receivers is spelled as a **result newtype plus a family of anonymous
arrows** — `Length = Int` is declared by `Map`, `Set`, `String`, and
`List` alike, each contributing its own `… => Length` arrow, and the
structural merge makes them one type.

The literal boundary needs no exception either: a JSON hole converts
through the `Encoded = Json` family, an HTML hole through
`Escaped = Html`, and a format-string hole through `String` itself.
Interpolation is construction all the way down.

## The Entry Point

A module becomes a runnable program when **exactly one** anonymous
arrow returns a type matching a known WASI world's primary export.
Entries have no name -- selection is by signature only, and giving the
entry a name (a literal `main =` is the classic mistake) is a checker
error. The CLI entry is `Unit => Program { ... }` (`Program = Unit`,
from `canon`) -- no arguments in, no explicit exit out, mirroring
the HTTP entry's `Request => Response { ... }` in anonymity:

| Signature | World | Export |
|---|---|---|
| `Unit => Program`, `Unit => Result<Program, _>` | `wasi:cli/command` | `wasi:cli/run.run` |
| `Request => Response`, `Request => Result<Response, _>` | `wasi:http/service` | `wasi:http/handler.handle` |

The CLI entry's shape is the ABI's: `wasi:cli/run.run` takes nothing
and reports only success/failure. A `Result` / `Option` entry that ends
on `Err` / `None`, or whose `?` meets one, reports failure: a string
payload is printed and the process exits 1. The argument vector is
fetched, not passed -- `Args()` (`= List<String>`, from `canon`, bound from
`wasi:cli/environment#get-arguments`) reads `argv` from any body -- and
an exact exit code is the hard `Exited(n)`
(`wasi:cli/exit#exit-with-code`) escape hatch.

A third world -- the browser [web target](../reference/web-target.md) -- is
selected by a **triple of anonymous, type-selected constructors**:
`Model => Html` (the view), `Unit => Init` (init, `Init` a model-alias
marker), and `Model * Msg => Model` (update -- the model's one command,
`Msg` its message). Detection anchors on the view -- the sole `Model =>
Html` with a user-type receiver -- then finds the model's nullary
constructor and its command. The triple compiles to a core wasm module plus a generated
JS host rather than a component.

Rules the compiler enforces:

- Two arrows returning a world type: compile error (ambiguous
  entry). **Helpers must return ordinary data**, never `Response`.
- Mixed worlds in one module: compile error; a component exports
  exactly one world.
- Zero matches: the module is a library, usable by reference from
  other modules, not runnable.
- The entry is lifted **async-stackful** at the Component Model
  boundary, so suspending calls anywhere beneath it can yield without
  trapping ([Effects and the Async Model](./effects-and-async.md)).

The same shape-driven selection powers testing: every result newtype
`X = TestResult` with a nullary `Unit => X` constructor in a file is a
test under `canon test` ([Testing](../tour/testing.md)) -- the name is
a type name, and the arrow stays anonymous.

## Declaration Order

Declarations in a file must appear in alphabetical order; the checker
enforces this at compile time. The entry point and other
compiler-synthesised arrows are exempt (they are distinguished by
role, not name). A declaration nothing reaches -- **dead code** -- is a
hard error, not a warning. See [Ordering Rules](./ordering.md).
