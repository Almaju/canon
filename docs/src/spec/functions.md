# Functions and Traits

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

A named form also exists -- `Line = (Greeting * Name) => Line { ... }` --
and is how a *result newtype* declares the operation that produces it
(`Inserted = (Map * String * Value) => Map { ... }`). The name must be
PascalCase: a camelCase declaration is a checker error everywhere
except [binding files](./compilation.md) and `canon test` functions.

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

## Traits

A trait is a **callable type signature**, declared like a body-less
function type and named in PascalCase (traits are types):

```canon
Show = () => String
```

**Implementation** declares a function with the trait's name, prepending
the implementing type to the parameter list:

```canon
Show = (Greeting) => String {
    "HELLO!"
}
```

The bodied declaration and the body-less signature share one name and
one namespace: a trait is a family of implementations selected by the
input's type. Call sites use the ordinary pipe:
`Greeting("hi") -> Show`.

- **Multi-method traits** are products of single-method traits:
  `Presentable = Debug * PrintString`. Implementing the product means
  implementing every factor.
- **Traits as components**: a trait may appear directly in a parameter
  list; the component binds the implementation, which is invocable:
  `Show => Unit { Show() -> Print }`.
- **Defaults**: a trait declaration may carry a default body marked
  `{ impl }`; implementing types may override or inherit it.
- **Constraints**: `<T: Show>` bounds a generic parameter by a trait.

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
| `Args => Exit` (also `Unit => Program`, `... => Result<Exit, _>`, and the legacy `ExitCode`) | `wasi:cli/command` | `wasi:cli/run.run` |
| `Request => Response`, `Request => Result<Response, _>` | `wasi:http/service` | `wasi:http/handler.handle` |

`Args` (`= List<String>`, from `canon/std`) is the program's `argv`: the
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

The same signature-driven selection powers testing: every
`() => TestResult` function in a file is a test under `canon test`
([Testing](../guide.md#testing)).

## Declaration Order

Declarations in a file must appear in alphabetical order; the checker
enforces this at compile time. The entry point and other
compiler-synthesised arrows are exempt (they are distinguished by
role, not name). A declaration nothing reaches -- **dead code** -- is a
hard error, not a warning. See [Ordering Rules](./ordering.md).
