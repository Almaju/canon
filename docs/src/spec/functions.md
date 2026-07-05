# Functions and Traits

## Declaration

```canon
name = (Components) => ReturnType {
    body
}
```

The parenthesised components form a **product**: the function's input
is a product type, written with the same `*` used everywhere else. There
are no commas, no parameter names, no defaults:

```canon
greet = (Greeting * Name) => Line {
    Line(Greeting.String.concat(Name.String))
}
```

- Components follow the [alphabetical rule](./ordering.md):
  `(Greeting * Name)` is legal, `(Name * Greeting)` is a compile error.
- Components must be distinct types; disambiguate duplicates with a
  newtype (`OtherUser = User`).
- Inside the body, each component is referenced by **its type name**:
  `Greeting` is the greeting value, `Name` the name value.
- There is no `Self` and no local variables.

## Commutative Calling

At the call site, **any component may appear before the dot**; the rest
are passed in parentheses:

```canon
Greeting("hi ").greet(Name("ada"))
Name("ada").greet(Greeting("hi "))
```

Both are the same call, a consequence of `*`'s commutativity: the
receiver position is not privileged, it merely selects which component
the caller writes on the left. For arities above two, the remaining
components are passed as a product value:

```canon
router.route(Handler(...) * Path("/api"))
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
   `compare = (OtherUser * User) => Ord`, `alice.compare(bob)` is
   rejected: which value is the `OtherUser` decides `Less` versus
   `Greater`. Write `alice.compare(OtherUser(bob))`.

**Repeated components bind positionally.** A function over a fixed
repetition, such as `merge = (User^2) => User`, has positional
components (`.1`, `.2`, …), so binding is positional too: the receiver
fills `.1`, remaining arguments fill `.2` and onward in the order
written. Commutative reordering does not apply, because position *is*
the identity of a repeated component. Use `T^N` when order is the
honest semantic (pairs, coordinates); use distinct newtypes when
components mean different things.

## Optional Components

No special syntax; optionality is `Option<T>` in the signature, and the
call site may omit the component:

```canon
paint = (Option<Color> * String) => Unit { ... }

"hello".paint()
"hello".paint(Red(0xFF0000))
```

Omission is legal **only when the component's type implements the
`Default` trait**; the compiler inserts `T.Default()` for the missing
component. `Default = <T>() => T` is an ordinary trait, and core ships
exactly one implementation: `Option<T>`'s, which returns `None()`. So
omitting an `Option<Color>` means `None()`, but the defaulting is not a
special case: it is opt-in, declared in source, and a user type that
wants omission semantics implements `Default` for itself.

## First-Class Functions and Lambdas

A function is referenced as a value by qualifying it with one of its
component types (`Int.double`) and passed wherever a matching signature
is expected. Anonymous functions are lambda literals with a **full
signature** (there is no inference):

```canon
Numbers.map((Int) => Int { Int.mul(Int(3)) })
```

Lambda syntax is declaration syntax minus the `name =` prefix.

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

Case alone distinguishes the forms: `show` would be a regular function;
`Show` is a trait implementation. Call sites use ordinary commutative
syntax: `Greeting("hi").Show()`.

- **Multi-method traits** are products of single-method traits:
  `Presentable = Debug * PrintString`. Implementing the product means
  implementing every factor.
- **Traits as components**: a trait may appear directly in a parameter
  list; the component binds the implementation, which is invocable:
  `needsShow = (Show) => Unit { Show().print() }`.
- **Defaults**: a trait declaration may carry a default body marked
  `{ impl }`; implementing types may override or inherit it.
- **Constraints**: `<T: Show>` bounds a generic parameter by a trait.

## The Entry Point

A module becomes a runnable program when **exactly one** free function
returns a type matching a known WASI world's primary export. Selection
is by signature, not by name:

| Return type | World | Export |
|---|---|---|
| `Unit`, `ExitCode`, `Result<Unit, _>`, `Result<ExitCode, _>` | `wasi:cli/command` | `wasi:cli/run.run` |
| `Response`, `Result<Response, _>` | `wasi:http/service` | `wasi:http/handler.handle` |

A third world — the browser [web target](../reference/web-target.md) — is
selected differently. It can't key on a return type, because every view
helper returns `Html`, so detection matches the conventional **triple of
names and shapes**: `init = () => Model`, `update = (Model * String) ->
Model`, and `view = (Model) => Html` together. The triple compiles to a
core wasm module plus a generated JS host rather than a component.

Rules the compiler enforces:

- Two functions returning a world type: compile error (ambiguous
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
([Testing](../tour/testing.md)).

## Declaration Order

Functions in a file must be declared in alphabetical order; the checker
enforces this at compile time. The synthesised test `main` and similar
compiler-generated entries are exempt (they are distinguished by role,
not name). See [Ordering Rules](./ordering.md).
