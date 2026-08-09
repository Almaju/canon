# Functions

## Declaration

Every callable is a **constructor**, named after the type it produces.
The declaration arrow is `=>`; writing `->` at a declaration site is a
parse error (`->` is the value-level pipe -- see
[Expressions](./expressions.md)). The anonymous form needs no name of
its own, because the return type *is* the name:

```canon
Components => ReturnType {
    body
}
```

The components form a **product**: the input is a product type, written
with the same `*` used everywhere else. There are no commas, no
parameter names, no defaults:

```canon
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
  `Frobnicated = (Int) => Int` with no `Frobnicated` newtype
  anywhere -- is a checker error: *a name carries no information the
  types don't*.
- An arrow may not construct a type that is also one of its inputs. An
  endomorphism (`Map * String => Map`) is the one operation whose types
  cannot identify it -- insert, remove, and update all share that
  signature -- so the operation takes a **result newtype**: `Inserted =
  Map` plus `Map * String * Value => Inserted { ... }`. Exact-name
  comparison only: a newtype input flowing into its base type's
  constructor (`Rest = Map` into a `Map` constructor) is a different
  type and stays legal.

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

```canon
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
Numbers -> Mapped((Int) => Int { Int -> Product(3) })
```

Lambda syntax is declaration syntax with the parentheses kept and no
top-level name: the same `=>` arrow that declares every constructor.

## No Traits

Canon has no traits, and a body-less function-type declaration (a
"shape", `Show = () => String`) is a **checker error** -- there are no
exceptions. What a trait provides elsewhere, a **result newtype with a
constructor family** provides here with a checked name: `Length` spans
`Map`, `Set`, `String`, and `List` as a merged `Length = Int` plus one
anonymous arrow per receiver; `Encoded` spans `Bool`, `Float`, `Int`,
and `String` the same way. One name, per-type implementations selected
by the input's type, call sites on the ordinary pipe -- and the
compiler checks that every implementation constructs the declared
type, which no trait system does. Even the literal-interpolation
boundary needs nothing more: a JSON hole converts through the
`Encoded = Json` family, an HTML hole through `Escaped = Html`, and a
format-string hole through `String` itself -- interpolation is
construction all the way down.

## The Entry Point

A module becomes a runnable program when **exactly one** anonymous
arrow returns a type matching a known WASI world's primary export.
Entries have no name -- selection is by signature only, and giving the
entry a name (a literal `main =` is the classic mistake) is a checker
error. The CLI entry is `Args => Exit { ... }` -- the command's argument
vector flows in, an exit status flows out, mirroring the HTTP entry's
`Request => Response { ... }`:

| Signature | World | Export |
|---|---|---|
| `Args => Exit` (also `Unit => Program` and `... => Result<Exit, _>`) | `wasi:cli/command` | `wasi:cli/run.run` |
| `Request => Response`, `Request => Result<Response, _>` | `wasi:http/service` | `wasi:http/handler.handle` |

(The legacy `ExitCode` return is retired -- `Exit` is the one
exit-status type.)

`Args` (`= List<String>`, from `canon`) is the program's `argv`: the
compiler binds it from `wasi:cli/environment#get-arguments` at the lifted
`run` boundary and hands it to the entry, exactly as the HTTP world hands
the handler its `Request` -- you never fetch it. `Exit` (`= Int`) is the
exit status. Because `wasi:cli/run` returns a bare `result`, `Exit(0)`
maps to success (process exit 0) and any nonzero `Exit` to failure
(exit 1); an exact nonzero code uses the hard `Exited(n)`
(`wasi:cli/exit#exit-with-code`) escape hatch. A program that reads no
arguments and reports nothing may use the arg-less shorthand
`Unit => Program { ... }` (`Program = Unit`), whose body needs no explicit
exit.

A third world -- the browser [web target](../reference/web-target.md) -- is
selected by a **triple of anonymous, type-selected constructors**:
`Model => Html` (the view), `Unit => Init` (init), and
`Model * Msg => Update` (update), where `Init` / `Update` are model-alias
markers. Detection anchors on the view -- the sole `Model => Html` with a
user-type receiver -- then finds the model's nullary and two-input
constructors. The triple compiles to a core wasm module plus a generated
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
test under `canon test` ([Testing](../learn/testing.md)) -- the name is
a type name, and the arrow stays anonymous.

## Declaration Order

Declarations in a file must appear in alphabetical order; the checker
enforces this at compile time. The entry point and other
compiler-synthesised arrows are exempt (they are distinguished by
role, not name). A declaration nothing reaches -- **dead code** -- is a
hard error, not a warning. See [Ordering Rules](./ordering.md).
