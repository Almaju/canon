# Canon

Canon is a new programming language. The reference implementation compiles to a **WebAssembly Component** targeting **WASI Preview 3** — every Canon program is a portable `.wasm` that runs on any Component Model host. The compiler itself is written in Rust and embeds `wasmtime` to make `canon run` a single-step experience, but no Rust toolchain is required at build or run time and no Rust source is emitted along the way.

The language inherits Rust-style ownership and zero-cost abstractions through the compiler's analysis, while presenting a much smaller surface area to the programmer.

## Guiding Principle: Alphabetical Order Everywhere

Wherever ordering is discretionary, Canon requires **alphabetical order**. This is not a style suggestion — it is enforced by the compiler. The rule applies to:

- Components of a product type: `User = Birthday * Username`
- Variants of a union type: `Bool = False + True`
- Type definitions within a file (declared top-to-bottom alphabetically)
- Function declarations within a file (declared top-to-bottom alphabetically)
- Arms of a dispatch (in the order of the union's variants — which are themselves alphabetical)
- Trait composition: `Show = Debug * PrintString`
- Error unions inside `Result`: `Result<T, IoError + NotFound + PermissionDenied>`
- Imports: multiple `use` statements at the top of a file

The reasoning: ordering is a constant source of bikeshedding and diff noise. By forcing one canonical order, code reads the same way no matter who wrote it, and reordering is never a meaningful change.

Because the canonical order is mechanical, it is also **auto-fixable**: `canon fmt` sorts `use` imports, type definitions, function declarations, and dispatch arms into canonical order. The checker's ordering errors are the backstop for code that bypassed the formatter, not a hand-sorting chore. (The entry point — `main` or the HTTP handler — is exempt and keeps its position: it is a distinguished role, not a regular free function.)

One caveat worth knowing: alphabetical order is a *source-level* canon, never a wire format. Union variants are numbered by their alphabetical position internally, so adding a variant renumbers everything after it. Serialized values must therefore always carry variant *names*, not indices; at the Component Model boundary the WIT file's declared order governs the ABI, and the compiler maps between the two.

## Core Types

The language has one primitive zero-data type: `Unit` — the type with exactly one value (the multiplicative identity — `T * Unit ≡ T`). `Never` completes the algebra as the type with zero values (the additive identity — `T + Never ≡ T`). Together with `+` and `*`, these form a type semiring.

Everything else is composed. The two boolean atoms are themselves newtype aliases of `Unit`:

```
False = Unit
True  = Unit
Bool  = False + True
```

A `Bit` is just `Bool` under a different name (`Bit = False + True`), so the same algebra extends from booleans up through `Byte = Bit^8` and `Bytes = Byte^*`.

A small set of built-in primitive operations (e.g. arithmetic on `Int`) is supplied by the compiler — these cannot be derived purely from bits, but their *shape* is still described by the type system.

## Type Composition

### Unions (`+`)

A union expresses "this or that":

```
Bit = False + True
```

### Products (`*`)

The `*` operator expresses "this and that" — a value of the resulting type has all of its component parts.

```
Byte = Bit * Bit * Bit * Bit * Bit * Bit * Bit * Bit
```

#### Product Members Are Alphabetical

By the global alphabetical-order rule, the components of a product are always written in alphabetical order:

```
User = Birthday * Username
```

The same applies to construction:

```
User(Birthday(...) * Username(...))
```

#### Field Access

A product's components are accessed by their type name:

```
user.Birthday
user.Username
```

For repeated components (or anonymous sequences), positional access by 1-based index is used:

```
byte.1
byte.2
```

`.1` is the first `Bit`, `.2` the second.

#### Newtypes Are 1-Component Products

A newtype alias `A = B` (where `B` is a single named type) is a degenerate product with one component, named `B`. The field-access rule applies uniformly:

```
Greeting = String

Greeting("hi").String
```

The `.String` access unwraps to the underlying `String`.

This composes with method lookup through the alias chain: `Greeting("hi").print()` works because `print` is inherited from `String` (no need to unwrap first). Methods declared explicitly on the newtype shadow the inherited ones.

For argument passing, newtypes are *substitutable* for their underlying type — a function expecting `String` accepts a `Greeting` directly, without `.String`. The unwrap form exists for cases where the explicit step is clearer (e.g. inside a dispatch arm where the bound name reads more naturally with the unwrap visible).

Multi-step aliases unwrap one step at a time: with `A = B` and `B = C`, write `aValue.B.C` to reach the bottom.

#### Field Access vs Construction

Because both field names and type constructors are PascalCase, the dot syntax would be ambiguous without a rule. Canon resolves it with parentheses:

- `user.Birthdate` — **field access**: reads the `Birthdate` component of `user`
- `user.Birthdate()` — **constructor call**: calls `Birthdate` as a function with `user` as the receiver

The `()` unambiguously signals intent to produce a new value. Its absence signals observation of an existing one.

### Fixed Repetition (`Type^N`)

For a fixed count of the same type, use `Type^N`:

```
Byte = Bit^8
```

Algebraically, `T^N` is the N-fold product `T * T * … * T`. The caret reads as exponentiation, which is exactly what a fixed-size array is.

### Unbounded Repetition (`Type^*`)

For unbounded sequences:

```
Bytes = Byte^*
```

`T^*` is the Kleene star — zero or more repetitions of `T`. Together with `^N`, both repetition forms share the same operator and sit naturally in the `+` / `*` / `^` semiring.

`List<T>` is not a separate concept: core defines **`List<T> = T^*`** — the nominal name and the algebraic form are the same type. `Bytes = Byte^*` therefore has every `List` method (`map`, `first`, `get`, …) with nothing to declare, and `List(…)` is simply the value-level constructor for the star (the algebraic form has no literal syntax of its own).

Higher-level types like `Int`, `Float`, and `String` are defined from `Byte`/`Bytes`.

## Generics

Types can be parameterized by other types using angle brackets:

```
List<T>
Option<T>
Result<T, E>
Map<String, Int>
```

The chevron syntax does not conflict with `[]` repetition or `*` product.

### Generic Constraints

Constraints on type parameters use `:`, naming a trait the parameter must implement:

```
showAll = <T: Show>(List<T>) -> Unit {
    ...
}
```

### Type Arguments at Call Sites

Where Canon cannot infer a generic function's type parameters from context, the caller pins them with `::<...>` after the function name (the same "turbofish" form Rust uses):

```
"[1, 2, 3]".parse::<List<Int>>()?
```

Reads as: call `parse` with `T = List<Int>`. The `::` separator disambiguates the `<` from a comparison.

Turbofish is only required when the surrounding type context is insufficient. A function with an explicit `Result<List<Int>, _>` return type, for instance, lets the compiler infer `T` from the return position without an annotation.

## Literals

The language is values-only — there is no `new`, no implicit nullability, no bare keywords like `true` / `false`. Constructors are just regular functions named after the type:

```
Int(123)
```

For ergonomics, several literal forms are sugar over their constructors:

| Literal        | Desugars to        |
|----------------|--------------------|
| `123`          | `Int(123)`         |
| `1.0`          | `Float(1.0)`       |
| `"abc"`        | `String("abc")`    |
| `0xFF0000`     | `Hex(0xFF0000)`    |
| `{"k":v}`, `[v]` | a `Json` value (static parts constant-folded; interpolated parts via `ToJson`) |

String literals exist to avoid the parsing ambiguity of bare `String(...)` with spaces and punctuation. Numeric literals exist to avoid boilerplate in arithmetic-heavy code.

#### Zero-Data vs Data-Carrying Constructors

`T()` with no arguments is valid **only** when `T` has no underlying composition — i.e., it is a zero-data type like `Unit`, `True`, `False`, or a union variant with no payload. These types have exactly one value; `()` simply signals "I am producing it."

`String()`, `Int()`, `User()` — calling any data-carrying constructor with no arguments is a compile-time error. If a value can legitimately be "missing", that absence belongs in the type as `Option<T>`; otherwise the type requires its data.

For factory-style construction (e.g. "an empty list"), use an explicit lowercase function — `List.empty()`, `String.empty()`.

### Zero-Data Types

A type with no underlying composition (e.g. `Unit`, or any newtype alias of it such as `True` or `False`) has exactly one value. In expression position, it is constructed with `T()` — the empty argument list signals that you are producing a value, not accessing a field:

```
Ok(Unit())
True().(
    * (False) -> Unit { "no".print }
    * (True) -> Unit { "yes".print }
)
```

In **type position** (signatures, type definitions, dispatch arm patterns) the bare name is used as usual:

```
describe = (Tree) -> String {
    Tree.(
        * (Branch) -> String { "branch" }
        * (Leaf) -> String { "leaf" }
    )
}
```

The rule: `()` signals construction; its absence after a PascalCase name signals observation (field access or type reference).

## Constructor Arguments

Every type `T` has a constructor `T(_)`. The argument is a value matching the type's underlying definition:

| Kind             | Constructor                            | Argument is…                                  |
|------------------|----------------------------------------|-----------------------------------------------|
| Primitive        | `Int(123)`, `Float(1.0)`, `String("hi")` | a literal of the corresponding lexical kind   |
| Hex              | `Hex(0xFF0000)`                        | a hex literal                                  |
| Product `A * B`  | `T(A(...) * B(...))`                   | a value-level product joined with `*`          |
| Union `A + B`    | `T(A(...))` or `T(B(...))`              | a value of any variant                         |
| Newtype          | `T(inner)`                             | a value of the aliased type                    |

So `Hex(0xFF0000)` constructs a `Hex` value (the literal `0xFF0000` desugars to the same form), and `User(Birthday(...) * Username("ahanot"))` constructs a `User` from its two components.

`*` is overloaded across the two levels: at the type level it forms a product type, at the value level it forms a product value. The two never appear in the same context.

### Validated Constructors

By default, a type's constructor is total: `T(inner)` always succeeds and returns `T`. For types whose construction can fail — `Url` from a `String`, `Email` from a `String`, parsing in general — the construction belongs in the type system as `Result<T, E>` (or `Option<T>` for "this might just not exist"), the same way "missing" is expressed as `Option<T>`.

A type opts into this by declaring a constructor with the **same name as the type** — a function whose PascalCase name matches the file's type:

```
Url = String

Url = (String) -> Result<Url, InvalidUrl> {
    ...
}
```

This follows the same `Name = ...` pattern as everything else. The compiler distinguishes it from the type definition by its shape: it has a function signature and a body. This is exactly how trait implementations work — `Show = (Greeting) -> String { ... }` implements the `Show` trait; `Url = (String) -> Result<Url, InvalidUrl> { ... }` implements the `Url` constructor.

**Rules:**

- If a file declares a constructor, that *is* the constructor. The implicit total constructor is replaced.
- The signature is unconstrained — total (`(String) -> Url`), fallible (`Result<Url, InvalidUrl>`), or optional (`Option<Url>`).
- Call sites use the ordinary constructor syntax: `Url("https://example.com")`. The expression's type is whatever the constructor returns, so a fallible constructor *forces* `?` (or `dispatch`) at the call site.
- External callers cannot bypass the constructor. Only functions declared in the same file have access to the type's raw inner representation.

```
main = () -> Result<Unit, HttpError + InvalidUrl> {
    Url("https://example.com")?.get()?.print
    Ok(Unit())
}
```

This generalizes the same principle the language already applies to absence: if a value can legitimately be invalid, the fallibility belongs in the type, not in a runtime convention.

In a [binding file](#binding-files), the pattern is the same but the body is omitted (and the WIT path lives in the file's `extern` header, not on each declaration):

```
extern "canon:builtins/url@0.1.0"

Url = String

Url = (String) -> Result<Url, InvalidUrl>
```

#### Error Naming

Errors are types like any other, and they're named *semantically* — by what failed, not by who emitted them. `InvalidUrl`, `MalformedJson`, `FileNotFound`, `PermissionDenied` carry information; `UrlError`, `JsonError`, `FsError` don't. The exception is opaque wrappers around foreign error types (e.g., `HttpError` wrapping the entire `reqwest::Error` enum) where the underlying error space hasn't been decomposed into Canon variants yet.

### Conversions

**Conversion is construction.** There is no `parse`, no `toString`, no `from`/`into` family — converting a value to type `T` is spelled the same way as constructing a `T`, because it *is* constructing a `T`:

```
String(42)            # "42" — decimal rendering
42.String()           # the same declaration, method spelling
Int("42")             # Result<Int, MalformedInt> — parsing can fail
"42".Int()?           # method spelling, ?-propagated
String(Byte(65))      # "A" — a Byte renders as its character
List("1", "2").Json() # [1,2] — a list of JSON values as a JSON array
```

One rule covers everything:

- **Infallible conversions return the target type.** `String(Int)` always succeeds, so it returns `String`. The function's name is its return type — it cannot lie.
- **Fallible conversions are [validated constructors](#validated-constructors).** `Int(String)` can fail, so it returns `Result<Int, MalformedInt>` and forces `?` or dispatch at the call site — exactly like `Url(String)` or `Json(String)`.
- **The method spelling is free.** `T(value)` and `value.T()` are the same declaration (the commutative method-call rule), so what Rust splits into `From` and `Into` is one function here.
- **Ambiguity is resolved by newtypes.** `Int` → `String` has two readings — decimal rendering and byte-to-character — so the second one takes a wrapper: `String(42)` renders, `String(Byte(42))` is `"*"`. Wrapping to mean the other thing is what newtypes are for.

User types opt in the same way the stdlib does: declare a function named after the target type taking the source type. `Celsius = (Fahrenheit) -> Celsius { … }` makes both `Celsius(f)` and `f.Celsius()` work.

## Naming Conventions

- **Types**: `PascalCase`
- **Traits**: `PascalCase` (traits are types)
- **Functions**: `camelCase`

The case difference disambiguates trait implementations from regular functions: `print` is a function, `Print` is the implementation of the `Print` trait.

## File and Module Layout

- **Files** use `kebab-case.can` names — the mechanical conversion of the declared type's PascalCase name (`http-server.can` declares `HttpServer`).
- A file's name **must match** the type it declares: `foo.can` must declare a type named `Foo`.
- A **module is a folder**. There is no `mod` declaration. Importing `Foo` from a sibling folder is enough.
- The entry point is `main.can`; libraries live in `lib.can`.

### Imports

```
use Foo
use acme/image/Decoder
use canon/std/Json
use models/User
```

| Import | Resolves to |
|---|---|
| `use Foo` | local: `foo.can` or `foo/main.can` relative to this file |
| `use models/User` | local: subfolder lookup, `models/user.can` |
| `use canon/std/Json` | package: `<namespace>/<package>/<Type>` |
| `use acme/image/Decoder` | third-party: same shape, no privileged path |

There is exactly one `use` resolution rule. Given `use a/b/c/…/Z`:

1. If the leading segments `a/b` (the first two) match a declared dependency in the project's package manifest, resolve as a **package import**: locate the package in the cache, then look up the type `Z` (or, for multi-file packages, the file matching `Z`'s [kebab-case form](#naming-conventions)) inside it.
2. Otherwise, resolve as a **local import**: walk the segments as directories relative to the current file and load `z.can` or `z/main.can` at the end.

The shipped packages `canon/std` and `canon/wasi` are pre-installed and bundled with the compiler binary, but indistinguishable from any other package at the language level — they appear in the cache and must be listed as deps to be used.

Each import names exactly one type — there are no wildcard imports. If you use `JsonValue` and `JsonArray`, you write both `use canon/std/JsonValue` and `use canon/std/JsonArray`.

**The prelude.** A handful of core names need no import at all: `Bool`, `Option`, `Result`, `List`, the primitives — and `Json`. JSON literals are part of the syntax, so `{"k":"v"}` types as `Json` (an alias of `String`) with zero ceremony; the loader pulls in `canon/std/Json` automatically the moment a program uses its machinery (interpolation, the validating `Json(...)` constructor, or `.ToJson()`). This mirrors Rust's prelude: the standard vocabulary is ambient, everything else is an explicit import.

Packages have versions. The version pin lives in the project's package manifest (see [Package Manifests](#package-manifests)), not in source. `use canon/std/Json` never carries an `@version`.

### Visibility

Everything is **public**. There is no private visibility modifier.

This is deliberate. The language already enforces radical transparency — no comments, no local variables, types as documentation. Hiding functions would cut against that philosophy. The one place encapsulation matters — protecting type invariants — is handled by [validated constructors](#validated-constructors): declaring a constructor replaces the total constructor, so the raw inner representation cannot be bypassed.

## Type Inference

There is **no type inference**. Every type must be explicitly written.

## Dead Code

Canon stays clean by construction: a **program's** declarations must be reachable from its entry point. `canon check` walks the reference graph from `main` (or the HTTP handler) and flags every unreachable type and function — dead code is not allowed to accumulate. The lint is a warning at the command line and promoted to a failure in CI.

Libraries are exempt: with no private visibility, every declaration in a library *is* exported surface, so there is nothing to flag — its dead code shows up downstream, in the programs that stopped calling it.

(There is no "unused error variant" rule beyond this: [error union widening](#error-union-widening) makes it natural to declare exactly the errors that actually flow, and adding one later widens callers' unions without breakage.)

## Functions

A function is declared as:

```
name = (components) -> ReturnType {
    ...
}
```

The components inside the parentheses form a product — the function's input. When a function takes multiple inputs, they are composed with `*`, the same operator that composes product types elsewhere in the language:

```
greet = (Greeting * Name) -> Line {
    Line(Greeting.String.concat(Name.String))
}
```

Components follow the same alphabetical-order rule as product members: `(Greeting * Name)` is valid because `Greeting` precedes `Name`; `(Name * Greeting)` would be a compile error.

There are no commas in parameter lists, no positional arguments — only type composition. This is a deliberate unification: a function's input is a product type, described with the same `*` used everywhere else.

### Commutative Calling

At the call site, **any component can appear before the dot**. The remaining components are passed inside the parentheses:

```
Greeting("hi ").greet(Name("ada"))
Name("ada").greet(Greeting("hi "))
```

Both calls are equivalent. This follows from the commutativity of `*` in product types — `Greeting * Name` and `Name * Greeting` describe the same composition. The dot is syntax sugar: it selects which component of the product the caller writes to the left.

For a function with more than two components, the remaining components are passed as a product value:

```
route = (Handler * HttpRouter * Path) -> HttpRouter { ... }

router.route(Handler(...) * Path("/api"))
```

This is a genuinely novel feature: in most languages, the receiver is a privileged position — *the* object you're calling the method on. In Canon, there is no privilege. A function is defined over a composition of types, and the caller enters it through whichever component reads most naturally in context.

#### Binding Rule

Commutative calling is a *syntactic* freedom — it must never become a *semantic* ambiguity. Arguments (including the receiver) bind to components by this rule:

1. **Exact type match binds first.** A value whose type is exactly `OtherUser` binds only the `OtherUser` component.
2. **Substitutability resolves what remains.** A value flows into an alias-compatible component (a `User` into an `OtherUser` slot) only when exactly one unfilled component accepts it.
3. **Anything else is a compile error.** If two same-typed bare values could each fill two alias-related slots, the call is ambiguous — the caller must wrap one explicitly.

For a non-symmetric function like `compare = (OtherUser * User) -> Ord`, this means `alice.compare(bob)` with two bare `User` values is a compile error (which argument is the `OtherUser`? the answer flips `Less` and `Greater`). The caller writes `alice.compare(OtherUser(bob))` — one wrap, and the binding is unambiguous in both the compiler's eyes and the reader's.

**Repeated components (`T^N`) bind positionally.** When newtyping is overkill, a function may take a fixed repetition — `merge = (User^2) -> User`. Repetition components are *positional* (they are accessed as `.1`, `.2`, …), so binding is positional too: the receiver fills `.1` and the remaining arguments fill `.2`… in the order written. `alice.merge(bob)` puts `alice` at `.1`, full stop — commutative reordering does not apply to repeated components, because position *is* their identity. Use `T^N` when order is the honest semantic (pairs, coordinates), and distinct newtypes when the components mean different things.

### The Entry Point

A module becomes a runnable program when exactly one of its top-level free functions has a **return type matching a known WASI world's primary export**. That function is the entry. The compiler scans the module by signature, not by name — there is no magic `main`.

The world registry:

| Return type | World | WASI export |
|---|---|---|
| `Unit`, `ExitCode`, `Result<Unit, _>`, `Result<ExitCode, _>` | `wasi:cli/command` | `wasi:cli/run.run` |
| `Response`, `Result<Response, _>` | `wasi:http/service` | `wasi:http/handler.handle` |
| the `init`/`update`/`view` triple (see below) | web app (browser) | `init` / `update` / `view` |

A CLI program:

```
main = () -> Unit {
    "hello".print()
}
```

An HTTP service:

```
home = (Request) -> Response {
    Response(Body("hello") * Headers() * Status(200))
}
```

A web app (browser frontend) — the one world that can't key on a
single return type, because every view helper returns `Html`. It is
selected by the **Elm-architecture triple**, names and shapes
together:

```
init = () -> Model               # the starting state
update = (Model * String) -> Model   # fold a message into the state
view = (Model) -> Html           # render the whole page
```

`Model` is any user type. The program compiles to a wasm module plus
a generated JS host that owns the browser event loop; the guest stays
pure. See `WEB-TARGET.md`.

A CLI entry takes no parameters — output goes through the built-in
`.print()`, which the compiler wires to `wasi:cli/stdout` in the
emitted component. An HTTP entry takes the incoming `Request`. The
compiler validates that the parameter list is consistent with the
selected world.

Rules:

- **Exactly one** top-level function may return a world-shape type. Multiple matches are a compile error.
- **Mixed worlds** in the same module (one function returning `Unit`, another returning `Response`) are a compile error — a component exports exactly one world.
- **Zero matches** means the module is a library, not a program. It can be `use`d from another module but not run with `canon run`.
- The entry is lifted as *async-stackful* at the Component Model boundary so nested calls to suspending externs (filesystem, network, …) can yield without trapping. The programmer writes uniform, sync-looking code; the compiler handles the rest.

Helpers should return non-world types (`String`, user-defined products, etc.). The discipline "helpers return data, the entry returns the world-shape" is the layering this rule encourages.

See `WASI-HTTP-HANDLER.md` for the implementation plan.

### Referring to Components

Inside a function body, each component is referenced by **its type name**:

```
format = (Greeting * Name) -> String {
    Greeting
}
```

`Greeting` here is the greeting value; `Name` is the name value. Each type name binds to the value of that type that was passed in.

### Disambiguation

If two components would share the same type, create a newtype alias. Product members must be distinct types, so `(User * User)` is a compile error:

```
OtherUser = User

compare = (OtherUser * User) -> Ord {
    User.Birthday.compare(OtherUser.Birthday)
}
```

Because calling is commutative, both `alice.compare(bob)` and `bob.compare(alice)` are valid.

### Example

```
shout = (Greeting) -> String {
    "HELLO"
}
```

### Declaration Order

Functions in the same file must be declared in alphabetical order:

```
add    = (User * ...) -> ...
export = (User * ...) -> ...
remove = (User * ...) -> ...
```

This is a compile-time requirement, not a convention.

### Optional Parameters via `Option<T>`

There is no special syntax for optional parameters. Optionality is expressed through the type system using `Option<T>`:

```
Blue  = Hex
Green = Hex
Red   = Hex
Color = Blue + Green + Red

paint = (Option<Color> * String) -> Unit {
    ...
}
```

This allows both forms at the call site:

```
"hello".paint()
"hello".paint(Red(0xFF0000))
```

A component may be omitted at the call site **only when its type implements the `Default` trait**; the compiler inserts `T.Default()` for the missing component. `Default` is an ordinary trait with the shape `Default = <T>() -> T`, and core ships exactly one implementation:

```
Default = () -> Option<T> {
    None()
}
```

So omitting an `Option<Color>` argument means `None()` — but the defaulting is not a hidden special case for `Option`: it is opt-in, declared in source, and visible in the type. A user type that wants omission semantics implements `Default` for itself and accepts the same trade the language did: the absence at the call site is now meaningful. (Whether core should ever ship more `Default` impls than `Option`'s is deliberately conservative — defaults hide information, and `Option` is the one type whose *entire meaning* is "absence is fine".)

## No Local Bindings

Canon has **no `let` keyword and no local variables**. This is deliberate.

If you need to manipulate intermediate state, declare a new type for it. Names lie; types don't. Forcing every intermediate value through a named type makes the data flow explicit and the documentation structural.

## Function Bodies

A body is a **newline-separated sequence of expressions**. The last expression is the return value. There are no semicolons.

- A dispatch `.( )` is an expression — it can be the final line of a body, or appear as a sub-expression.
- Non-final lines whose results are discarded are valid (they exist for side effects or `?` propagation).

```
compare = (OtherUser * User) -> Ord {
    User.Birthday.compare(OtherUser.Birthday)
}

readConfig = (File * Path) -> Result<Config, IoError + ParseError> {
    File.read(Path)?
        .parse()?
        .validate()
}

classify = (Int) -> Sign {
    Int.compare(Int(0)).(
        * (Equal)   -> Sign { Zero }
        * (Greater) -> Sign { Positive }
        * (Less)    -> Sign { Negative }
    )
}
```

Without `let`, the only way to thread a value through multiple operations is method chaining. That is the intended style.

## First-Class Functions

Functions are first-class values. You refer to a function by qualifying it with one of its component types — `Type.function` — and pass it where a matching trait signature is expected:

```
Numbers = Int^*

doubleAll = (Numbers) -> Numbers {
    Numbers.map(Int.double)
}
```

### Lambdas

For one-off operations, write a lambda literal with its **full signature**. There is no signature inference.

```
tripleAll = (Numbers) -> Numbers {
    Numbers.map((Int) -> Int { Int.mul(Int(3)) })
}
```

Lambda syntax mirrors function declaration syntax: `(components) -> ReturnType { body }`. The only difference is the absence of a `name =` prefix.

## Memory Model

Canon has **no garbage collector**. The compiler performs Rust-style ownership analysis and lowers each value to a concrete linear-memory layout in the emitted core wasm module. **Ownership is invisible to the Canon programmer**: there are no lifetimes, no `&` / `&mut` sigils at the value level, no explicit `Box` or `Rc`. The compiler infers all of this from usage.

Rough mapping of source-level concepts to lowered wasm:

| Canon                                  | Lowered to                                              |
|-----------------------------------------|---------------------------------------------------------|
| Function parameter                      | Moved or borrowed value passed through wasm locals      |
| Recursive type (e.g. `Tree`)            | Heap-allocated cell in the bump heap (auto-boxed)       |
| Shared ownership the compiler can't otherwise prove | Reference-counted cell                      |

If the compiler cannot find a valid ownership scheme for a given program, it is a compile-time error. The error is surfaced in Canon terms.



## Recursive Types

Recursive type definitions are allowed and **boxed automatically** by the compiler — there is no user-visible `Box<T>`:

```
Tree   = Branch + Leaf
Branch = Left * Right * Value
Left   = Tree
Right  = Tree
Value  = Int
```

Whether the compiler boxes `Left` and `Right` individually or via some other indirection is an implementation choice; it is never spelled out in source.

## Control Flow

### Dispatch

There is no `if`/`else` or `match` keyword. All branching is via **dispatch** on a union — the value is the receiver, and the arms are the argument:

```
ord.(
    * (Equal)   -> String { "equal" }
    * (Greater) -> String { "greater" }
    * (Less)    -> String { "less" }
)
```

Each arm is a lambda whose single parameter is the variant type. Arms are separated by `*` (a product of handlers). The leading `*` before the first arm is optional — both forms are equivalent:

```
bool.(
    * (False) -> Unit { "no".print() }
    * (True)  -> Unit { "yes".print() }
)
```

Leading `*` on every arm is the recommended form (it aligns). Omitting it from the first arm is also accepted:

```
bool.(
    (False) -> Unit { "no".print() }
    * (True)  -> Unit { "yes".print() }
)
```

Dispatch arms follow the union's variant order, which is itself alphabetical.

Both `Bool` and `Ord` are ordinary union types in the standard library:

```
Bool = False + True
Ord  = Equal + Greater + Less
```

Algebraically, a dispatch on a sum type is isomorphic to a product of functions — one handler per variant:

```
(A + B + C) -> R  ≅  (A -> R) * (B -> R) * (C -> R)
```

The `.( )` syntax makes this literal: no keyword, no special form, just a value applied to its product of handlers.

**Accessing the inner value.** When a variant carries a payload, the inner value is in scope inside the arm body under the payload's type name. For standard library variants (`Ok<T>`, `Err<E>`, `Some<T>`) write the type argument explicitly — it binds the unwrapped value:

```
result.(
    * (Err<IoError>) -> String { IoError.message() }
    * (Ok<String>)   -> String { String }
)
```

For user-defined variants that have their own type definition (e.g. `Branch = Left * Right * Value`), write just the variant name — the matched value is in scope under that name and its fields are accessible:

```
Tree.(
    * (Branch) -> String { Branch.value().show() }
    * (Leaf)   -> String { "leaf" }
)
```

**Shadowing.** An arm binding is an ordinary lexical binding: inside the arm body it shadows any outer component of the same type name. A function that already has a `String` component and dispatches over a `Result<String, E>` sees the *payload* as `String` inside the `Ok<String>` arm. When both values are needed in the same arm, disambiguate the outer one with a newtype alias before dispatching — the same rule as same-typed parameters.

### Literal Dispatch

Dispatch extends to **equality dispatch on `String` and `Int`** scrutinees: arms may be literals, and the final arm is a **mandatory catch-all** naming the scrutinee's type. This is still dispatch — no `if`, no `match` keyword — extended to the two primitive kinds with unambiguous literal syntax:

```
route = (String) -> Response {
    String.(
        * ("/notes")   -> Response { index() }
        * ("/notes/1") -> Response { noteOne() }
        * (String)     -> Response { notFound() }
    )
}
```

Rules:

- The scrutinee must be `String` or `Int`, directly or through a newtype alias chain (`Path = String` dispatches with a `(Path)` catch-all).
- Literal arms can never be exhaustive, so **totality comes from the catch-all** — it is required, and it is always the last arm.
- Literal arms follow canonical order: alphabetical for strings, ascending for ints. Duplicates are a compile error. (`canon fmt` sorts arms automatically.)
- Inside every arm body the scrutinee value is in scope under its type name, exactly like a bound payload in union dispatch.

The catch-all is the same shape as a union dispatch arm — `(String) -> R { … }` reads as "any other `String`". Nested dispatch composes: dispatch on a union, then literal-dispatch the payload inside an arm (see `examples/notes-api`).

### Loops

There are no loop keywords (`while`, `for`). Iteration is expressed through higher-order methods on collections — `map`, `fold`, `for`, and friends — or through recursion.

## Error Handling

Errors are values, carried by the standard `Result<T, E>` type. The error slot is a regular type, so it can be a union written inline:

```
read = (File * Path) -> Result<Bytes, IoError + NotFound + PermissionDenied> {
    ...
}
```

This is more ergonomic than Rust's approach, where each call site typically needs a dedicated error enum.

### Error Union Widening

Inline error unions **widen** along `?`-propagation: a `Result<T, IoError>` propagates out of a function declared `Result<U, IoError + ParseError>` without ceremony — the error slot of the callee is a subset of the caller's, so `?` lifts the error into the wider union. Formally, `Result<T, E1>` flows into `Result<T, E2>` wherever `E1`'s variants are a subset of `E2`'s.

Alphabetical enforcement makes this cheap: every union has exactly one canonical spelling, so union equality — and the subset test — is purely syntactic. `IoError + NotFound` *is* the same type everywhere it appears; there is no nominal wrapper to unify.

### The `?` Operator

The postfix `?` operator propagates failure. It works on both `Result<T, E>` and `Option<T>`:

- On `Result<T, E>`: short-circuits with the error, otherwise unwraps to `T`.
- On `Option<T>`: short-circuits with `None`, otherwise unwraps to `T`.

```
functionName = (Foo) -> ReturnType {
    Foo.test()?
    Foo.test2()?
}
```

### Option vs Result

`Option<T>` and `Result<T, Empty>` are structurally similar but **kept distinct**: `None` means "absent", `Err(_)` means "failed". The semantic difference is worth the duplication.

## Values and Effects

Canon does not have a separate capability or effect system. Effects emerge naturally from the values you construct and thread through your program.

### Domain-First Design

The guiding principle: **start with real domain objects and transform them**. There are no service singletons, no manager objects, no repository classes. A `Path` value represents a file path; transforming it to `File` represents opening that file; reading the `File` gives you a `String`. The type chain is the access control.

```
Path("./data.json").File()?.read()?.print
```

Having a `File` value *is* the capability to read that file. You cannot get a `File` without having constructed it from a `Path`, and you cannot construct a `Path` from nowhere — it requires a `String`. The type system enforces this naturally.

This applies everywhere:

Fetching from the network — start with a `Url`:

```
Url("https://api.example.com/data")?.get()?.print
```

Current time — call the `Now` constructor directly:

```
Now().toRfc3339().print
```

HTTP server — wrap a `Port`, register routes, serve:

```
HttpServer(Port(3000))
    .get(HttpStatus(200), RoutePath("/"), "hello")
    .serve()
```

JSON — start with a `String`:

```
"[1, 2, 3]".JsonValue()?.JsonArray()?.length().print
```

No service object is ever conjured from thin air. No singleton is accessed statically. Every transformation requires a value you already hold.

### Print

`print = (String) -> Unit` is a built-in that writes to stdout. It requires nothing beyond a `String` — no capability token, no permission, no threading:

```
"hello".print()
42.print()
```

Under the hood, `.print` is lowered against `wasi:cli/stdout` so the produced component is portable to any Component Model host. For redirectable output — writing to a file or a log sink — construct the destination as an ordinary value (a `File`, eventually a `Fileout`) and thread it through the call chain. This is not a special form; it is the same domain-first pattern used everywhere else.

### Threading Effects

When a function performs a meaningful effect — reading a file, talking to a database — the relevant value appears in its signature. This is not enforced by a capability type system; it is the natural consequence of needing the value to do the work:

```
save = (Database * User) -> Result<Unit, DbError>
```

`user.save(database)` and `database.save(user)` are both valid (commutative calling). No `UserRepository`. No `DatabaseManager`. The `Database` value IS the access; having it means you can use it. You receive it because you had to construct it (from a connection string, a config, something real) and thread it to the functions that need it.

### Capability Entry Points (planned)

Today the type chain is *sequencing*, not access control: anyone can mint a `Path` from a `String`, and `Now()` is conjured from thin air. The planned completion of this design makes the claim literal, and it falls out of the compilation target: WASI **is** a capability system, and the entry point's signature becomes the program's authority manifest:

```
main = (Clock * FileSystem * Stdout) -> Unit {
    ...
}
```

- The compiler already selects the entry by signature shape and synthesises the component's world; under this design it derives the **imported WASI interfaces from the entry's parameter product**. A program whose `main` takes no `FileSystem` produces a component that cannot import `wasi:filesystem` — the sandbox is visible in the type signature.
- Capability values (`Clock`, `FileSystem`, `Stdout`, `Network`, …) are **unforgeable**: they have no constructor and arrive only as components of the entry point, threaded from there like every other value. Constructing a `Path` stays free; *opening* it requires the `FileSystem` value only `main` received.
- A zero-parameter `main = () -> Unit` stays valid — it declares a program with no ambient authority beyond `print` (see below).
- Existing programs migrate incrementally: the effectful stdlib wrappers gain capability-taking forms, and the ambient forms (`Now()`, `Path.File()`) are deprecated once the entry-point plumbing lands.

This subsumes the "no service singletons" discipline above: the reason no service object is conjured from thin air is that *nothing* is conjured from thin air — every authority is a value that entered through `main`'s signature.

### Async

There is no `async` keyword and no `.await` in Canon source. Both are inferred and inserted by the compiler.

A function is *suspending* if it (1) is a body-less declaration in a [binding file](#binding-files) whose corresponding WIT entry is `async func`, (2) consumes a `Future<T>` or iterates a `Stream<T>`, or (3) transitively calls a suspending function. The compiler computes this set bottom-up over the call graph and lifts the affected functions as `async func(…)` in the emitted Component Model world. Where a `Future<T>` value is consumed in a position that expects `T`, the loader automatically rewrites the call site into an `await` — the user writes neither keyword.

This is uniform: synchronous-looking code is the only style. The async machinery is invisible by design.

## Traits

A trait is declared like a body-less function signature. It names the shape that implementations must match:

```
Show = () -> String
```

Because traits are types, they are written in `PascalCase`. The shape is read as "any type that can produce a `String`".

### Implementing a Trait

A trait is implemented for a type by declaring a function with the trait's name (PascalCase) and the implementing type as a component. The implementing type is prepended to the trait's parameter list:

The first `Show` line below is the trait declaration (no body); the following two implement it:

```
Show = () -> String

Show = (Greeting) -> String {
    "HELLO!"
}

Show = (Name) -> String {
    "Alice"
}
```

This is distinguished from a regular function by case alone: `print` (camelCase) is a regular function, `Show` (PascalCase) is a trait implementation.

Call sites use ordinary commutative-call syntax:

```
Greeting("hi").Show()
Name("Alice").Show()
```

The first call invokes the `Greeting` impl of `Show`; the second, the `Name` impl.

### Coherence

A program may contain **at most one implementation per (trait, type) pair**. There is no preemptive orphan rule — any file in any package may implement any trait for any type — because Canon compiles whole programs: the build simply checks the final set of implementations and errors on an actual conflict, naming both defining files. The cost surfaces only when a conflict really exists (typically: adding a dependency that also implemented the trait), which beats forbidding whole classes of fine programs to guard against a conflict that might never happen.

### Multi-Method Traits

A trait with multiple methods is just a product of single-method traits:

```
Debug       = () -> String
PrintString = () -> Unit
Presentable = Debug * PrintString
```

Implementing `Presentable` for a type means implementing both `Debug` and `PrintString` for it.

### Using a Trait as a Parameter

A trait can be used directly as a component type. The component binds the trait implementation, which is then invocable:

```
needsShow = (Show) -> Unit {
    Show().print()
}
```

### `Self`

There is no `Self` keyword. Inside a function body, components are referenced by their type names directly — every component is explicit in the signature, so there is no ambiguity and no need for an alias.

## Concurrency

Canon has no `async` keyword and no `.await` at the source level. The compiler infers which functions are suspending and lifts them as `async func(…)` in the emitted component (see [Async](#async) above for the inference rules).

Task spawning, channels, and structured concurrency are planned and will be expressed as ordinary stdlib types over WASI Preview 3's task model — not as language keywords.

## Interop With the Host Ecosystem

Canon compiles to a **WebAssembly Component**, so interop happens at the Component Model boundary rather than at a source-level FFI. Every Component-Model interface a program uses is exposed as a **binding package** — an ordinary Canon package whose `.can` files are binding files. The compiler emits the matching component world; any compliant host (`wasmtime`, browser polyfills, edge runtimes) satisfies the imports.

The shipped stdlib is **layered**:

- `canon/wasi` — **raw bindings**, machine-generated from upstream WIT by `canon bindgen`. One `.can` file per WIT interface, each a [binding file](#binding-files). No idioms, no capability discipline, no opinions. Regenerated, never hand-edited.
- `canon/std` — **curated wrappers**, hand-written. One primary type per file (`canon/std/clock.can` declares `Clock`, `canon/std/file.can` declares `File`, …). Methods, constructors, and capability arguments live here. Idiomatic Canon code only ever imports from `canon/std`.

Where a `wasi:*` interface isn't yet usable from the canonical ABI, the corresponding `canon/wasi` file binds an `canon:builtins/*` bridge instead. The split is invariant — only the WIT path on the file's `extern` header changes.

### Binding Files

A **binding file** is a `.can` file whose first declaration is an `extern` header naming a Component-Model interface:

```
extern "wasi:random/random@0.3.0-rc-2026-03-15"

getRandomBytes = (Int) -> Bytes
getRandomU64 = () -> Int
```

The header pins the WIT interface this file binds. Every declaration in the file describes one symbol from that interface, mapped through the mechanical [WIT → Canon mapping](#wit--canon-mapping):

- **Function declarations are body-less.** Each function's name and signature must match a `func` (or `async func`) in the named WIT interface. The compiler verifies this when the package is loaded.
- **Type declarations** describe types defined by the WIT interface (records, variants, resources). They follow the same mechanical mapping.
- The `extern` header is the **only** way to introduce a Component-Model import. Body-less function declarations are **only legal inside a binding file** — anywhere else, a missing body is a compile error ("forgot a body").

The header string follows Component-Model path syntax: `namespace:package/interface@version`. The `@version` is optional for unversioned interfaces. There are no per-function path strings, no async annotations, no `from`/`sha256` fields on declarations — a binding file looks like ordinary Canon code, just without bodies.

> **First-class references.** Functions declared in a binding file are entered into the value scope on equal footing with ordinary functions. They may be referenced as first-class values (`Type.fn`) and passed as callbacks wherever a matching function signature is expected.

#### Async

Async-ness is **not** declared in source. If the WIT marks a function `async func`, the binding's mechanical mapping gives it a `Future<T>` return type. The async-ness is then visible in the signature itself, and the suspension-propagation rule in [Async](#async) handles the rest. No annotation, no keyword.

### Package Manifests

Each package — shipped, installed, or local — has an `canon.toml` at its root. The manifest is **TOML**, fixed schema:

```toml
# canon/std/canon.toml
name    = "canon/std"
version = "0.1.0"

[deps]
"canon/wasi" = "0.3.x"
```

A **project** is a package whose source includes a `main.can` entry point and whose manifest lists its dependencies. Dependencies are pinned by semver constraint:

```toml
# my-app/canon.toml
name    = "my-app"
version = "0.1.0"

[deps]
"canon/std"         = "0.1.x"
"acme/image-decoder" = "1.0.x"
```

When a package's component must be **fetched** (rather than satisfied by the host), the package's own manifest declares it:

```toml
# acme/image-decoder/canon.toml
name    = "acme/image-decoder"
version = "1.0.0"
from    = "https://components.example/image-decoder-1.0.0.wasm"
sha256  = "ab12cd34ef56…"
```

The compiler parses only the subset shown above (string values at the top level, a single `[deps]` table with quoted keys and string values). Full TOML compatibility is a non-goal; the format choice is for editor and tooling support, not for expressiveness.

Rules:

- `from` is **optional**. Absent means the package's bindings are host-provided (WASI, `canon:builtins/*`). Present means the build tool must resolve it.
- `from` schemes: `https://…`, `file://…`, and `github:owner/repo@vX.Y.Z/path.wasm` (sugar that expands to the corresponding GitHub release-download URL).
- `sha256` is **required whenever `from` is present**. No "fetch on first run and trust". The hash is computed by `canon install` at install time and written into the manifest by the tool; humans don't type it.
- Resolved components are cached at `~/.canon/cache/<sha256>.wasm`. The cache is content-addressed and machine-independent, so the same `(from, sha256)` pair is reproducible across checkouts.
- `canon build` with a cache miss while offline is a hard error. There is no implicit network access at build time — fetching is `canon install`'s job.
- When `from` is present, `canon build` **inlines** the fetched component as a nested instance in the output `.wasm`, producing a self-contained binary. (This is the role `wac plug` plays in the broader ecosystem; in Canon it's a built-in build step.)

The lockfile is the manifest. There is no separate `canon.lock`.

### Generating Bindings from WIT

The `canon/wasi` layer is produced by:

```
canon bindgen <path-or-url>
```

The input is either a `.wit` file (parsed directly) or a WebAssembly Component `.wasm` (the embedded `component-type` custom section is extracted and parsed). The output is a complete package: one [binding file](#binding-files) per WIT interface under `<namespace>/<package>/<interface>.can`, plus an `canon.toml` manifest. Output is deterministic and alphabetically ordered, so it round-trips through `canon fmt` and produces clean diffs on regeneration.

`canon install <url>` is a convenience that combines fetch + verify + `bindgen` + record:

1. Fetch the `.wasm` from the URL.
2. Compute its sha256. Confirm it is a valid Component.
3. Extract its WIT and emit a binding package, with its `canon.toml` carrying `from = "<url>", sha256 = "<digest>"`.
4. Populate `~/.canon/cache/<sha256>.wasm`.
5. Add the package to the project manifest's `deps`.

After `canon install`, the consumer writes `use <namespace>/<package>/<Type>` and the rest is ordinary Canon.

#### WIT → Canon Mapping

The mapping is mechanical. Every shape on the left maps to exactly one shape on the right.

| WIT | Canon |
|---|---|
| `bool` | `Bool` |
| `u8` … `u64`, `s8` … `s64` | `Int` |
| `f32`, `f64` | `Float` |
| `char` | `String` (single-grapheme) |
| `string` | `String` |
| `list<T>` | `List<T>` |
| `option<T>` | `Option<T>` |
| `result<T, E>` | `Result<T, E>` |
| `result<T>` (no error) | `Result<T, Unit>` |
| `result` (no payload) | `Result<Unit, Unit>` |
| `tuple<A, B, …>` | product with positional field names `_0`, `_1`, … (alphabetised by index) |
| `record { a: A, b: B }` | product `A * B` with fields kept in WIT order at the ABI boundary but exposed alphabetically in source |
| `variant { a, b(T), c }` | union; data-carrying arms become 1-component products |
| `enum { a, b, c }` | zero-data union |
| `flags { a, b, c }` | product of `Bool` per flag |
| `resource foo { … }` | newtype `Foo = Handle` (see below) |
| `func` | body-less top-level fn in a binding file |
| `async func` | body-less fn whose return type is wrapped in `Future<T>` |
| `stream<T>` | `Stream<T>` |
| `future<T>` | `Future<T>` |

**Identifier case** is converted mechanically: WIT `kebab-case` becomes Canon `camelCase` for values and `PascalCase` for types. `get-resolution` → `getResolution`; `incoming-request` → `IncomingRequest`; the WIT interface `wasi:clocks/monotonic-clock` becomes the file path `wasi/clocks/monotonic_clock.can` within the `canon/wasi` package.

**Record field ordering.** WIT records have a positional ABI order; Canon products are alphabetical. The generator preserves both: source-level fields are sorted alphabetically, and the canonical-ABI lowering reorders to the WIT layout. This reordering is invisible to user code.

#### Resources as `Handle`

WIT `resource` types are opaque, owning handles with methods and an implicit `drop`. Canon models them as a single-field product wrapping a builtin `Handle`:

`canon/wasi/filesystem/types.can` (generated):

```
extern "wasi:filesystem/types@0.3.0"

Descriptor = Handle

readViaStream = (Descriptor * Int) -> Result<InputStream, ErrorCode>
```

- `Handle` is a language-level primitive. It is **non-copyable** and **non-printable**; the only operations on it are passing it to a binding function and going out of scope.
- Methods on a WIT resource become free functions in the generated binding, with the resource as the first parameter. The canonical-ABI `[method]resource.fn` form is matched to a body-less Canon function whose first parameter is the corresponding resource type; the prefix is implicit and never written in source. Collisions are resolved by Canon's normal dispatch rules.
- Static methods (`[static]resource.fn`) become free functions with no `Handle` parameter; constructors (`[constructor]resource`) become functions returning the resource type.
- When a `Handle` value goes out of scope, the compiler emits the matching `resource.drop` call. Ownership is linear: a `Handle` cannot be aliased, only moved.
- **Own vs borrow is invisible at the source level.** WIT distinguishes `own<T>` (consuming) from `borrow<T>` (non-consuming) at the canonical-ABI boundary, but Canon source mentions neither. A binding function written as `(Descriptor * Int) -> Result<InputStream, ErrorCode>` may be lowered with a borrowing receiver or an owning one depending on the underlying WIT signature; the compiler reads that off the WIT and routes the call accordingly. This matches the rest of the language's [memory model](#memory-model) — ownership is inferred by the compiler, never written by the user. The same handle therefore flows through a chain of borrowing methods (`File.read`, `File.seek`, …) without the user threading anything back, and is dropped at the end of its last use just like every other value.

This means `canon/std/file.can` can wrap `canon/wasi/filesystem/types#Descriptor` in a clean type without the user ever touching a `Handle` directly — exactly the layering the stdlib split is for.

### Importing from Bindings

Idiomatic Canon code does not import binding files directly. Instead, it imports curated wrappers from `canon/std`. The wrappers are grouped into thematic sub-namespaces (`cli`, `fs`, `http`, `time`); the top level of `canon/std` is reserved for cross-cutting types (`IoError`, `Json`, `MalformedJson`, `Random`, `TestResult`).

```
use canon/std/IoError
use canon/std/Random
use canon/std/cli/Exit
use canon/std/fs/File
use canon/std/fs/Path
use canon/std/http/HttpServer
use canon/std/http/Url
use canon/std/time/Instant
use canon/std/time/Now
```

| Wrapper | Wraps |
|---|---|
| `canon/std/cli/Exit` | `canon:builtins/cli` |
| `canon/std/fs/File`, `canon/std/fs/Path` | `canon/wasi/filesystem/types` (or `canon:builtins/*` until P3 lands) |
| `canon/std/http/HttpServer` | `canon:builtins/http-server` |
| `canon/std/http/Url` | `canon:builtins/url` + `canon:builtins/http` |
| `canon/std/time/Instant` | `canon/wasi/clocks/monotonic_clock` |
| `canon/std/time/Now` | `canon/wasi/clocks/wall_clock` (or `canon:builtins/*` until P3 lands) |
| `canon/std/Random` | `canon/wasi/random/random` |

Each `use canon/std/<path>/X` imports exactly the named type along with its associated constructor and methods. The community can publish additional or alternative bindings under any namespace; the `canon/` namespace is reserved for packages shipped with the language.

A direct `use canon/wasi/clocks/monotonic_clock/now` works (everything is public), but you give up the capability discipline and the cleaned-up names that the `canon/std` wrapper provides.

### What Canon Ships Itself

Three things ship with the language:

**Core** — the small set of language-level primitives, owned by the compiler (not a package):

- Type system primitives: `Unit`, `Never`
- Boolean atoms (newtype aliases of `Unit`): `False`, `True`
- Bit‑level composition: `Bit` (`= False + True`), `Byte` (`= Bit^8`), `Bytes` (`= Byte^*`)
- Numeric and text: `Float`, `Hex`, `Int`, `String`
- Generic containers: `List<T>`, `Option<T>`, `Result<T, E>`
- Standard unions: `Bool` (`= False + True`), `Ord`
- Async wrappers: `Future<T>`, `Stream<T>`
- I/O built-ins: `print` (stdout), wired against `wasi:cli/stdout`

`Map` and `Set` are deliberately **not** core: they are ordinary pure-Canon stdlib types (`canon/std/Map`, `canon/std/Set`) — sorted recursive unions built from nothing but dispatch, recursion, and `String` comparison. `Map` is a sorted key-value map; iteration order is alphabetical by key, kept that way by construction (`insert` is a sorted insert). `Set` is its set-shaped counterpart, and `set.List()` returns the members alphabetically. The stdlib versions are `String`-keyed/`String`-valued until stdlib generics land; the guiding rule is that anything expressible in the language lives in the language.

`Future<T>` and `Stream<T>` appear in user-visible signatures only when a binding file mirrors an `async func` / `stream<T>` from its WIT interface (see [WIT → Canon Mapping](#wit--canon-mapping)). Everywhere else they are inferred and inserted by the compiler — ordinary Canon code consumes the unwrapped `T` and the async/streaming machinery is invisible (see [Async](#async)).

**Batteries** — two packages, bundled with the compiler binary and pre-populated in the cache:

- **`canon/wasi`** — generated binding files against `wasi:*` (and `canon:builtins/*` for interfaces that don't yet have a stable canonical-ABI shape). Mechanically produced by `canon bindgen`.
- **`canon/std`** — hand-written wrappers presenting a clean, capability-disciplined API. One primary type per file.

| Wrapper | Underlying binding | Status |
|---|---|---|
| `canon/std/time/Instant` | `canon/wasi/clocks/monotonic_clock` | ✅ |
| `canon/std/cli/Exit` (`Int.exit()`) | `canon:builtins/cli` (will move to `wasi/cli/exit` once narrow-int codegen lands) | ✅ |
| `canon/std/Random` | `canon/wasi/random/random` | ✅ |
| `canon/std/time/Now` (RFC 3339 wall-clock time) | `canon/wasi/clocks/wall_clock` (today: `canon:builtins/clock`) | ✅ |
| `canon/std/fs/File`, `canon/std/fs/Path`, `canon/std/fs/Contents` (write), `canon/std/IoError` | `canon/wasi/filesystem/types` (today: `canon:builtins/filesystem`) | ✅ — read + write (`Contents("x").write(Path("f"))?.File()?.read()?` round-trips) |
| `canon/std/http/Url`, `canon/std/http/InvalidUrl`, `canon/std/http/HttpError` | `canon:builtins/url` + `canon:builtins/http` | ✅ — will move to `wasi/http/outgoing_handler` |
| `canon/std/http/HttpServer`, `canon/std/http/HttpStatus`, `canon/std/http/Port`, `canon/std/http/RoutePath` | `canon:builtins/http-server` | ⏳ stub host; real `.serve()` semantics pending |
| `canon/std/Json`, `canon/std/MalformedJson` | `canon:builtins/json` (primitive builders only) | ✅ — `Json` validator is pure Canon (recursive-descent parser over `String.byteAt` / `.length` / `.substring` / `.eq`); `ToJson` trait for primitive types; `{"k":v}` / `[v,...]` literal syntax with interpolation; structural derive for user types pending |
| `canon/std/TestResult` (`Pass` / `Fail` + `assert`) | pure Canon | ✅ |
| `canon/std/Int` (`Int(String) -> Result<Int, MalformedInt>`), `canon/std/MalformedInt` | pure Canon | ✅ — the fallible half of [Conversions](#conversions); the infallible half (`String(Int)`, `String(Byte)`) is a compiler builtin |
| `canon/std/Byte` (`Byte = Int`; `String(Byte(65))` is `"A"`) | pure Canon (rendering builtin) | ✅ |
| `canon/std/Map` (sorted, alphabetical iteration; `String` keys/values for now) | pure Canon — recursive union | ✅ |
| `canon/std/Set` (sorted; `set.List()` = members alphabetically) | pure Canon — recursive union | ✅ |

The `canon:builtins/*` interfaces are temporary scaffolds. Each one moves to the corresponding `wasi:*` interface as that interface's canonical-ABI shape (async, streams, resources) becomes available — the binding file in `canon/wasi` is regenerated, the `canon/std` wrapper stays the same.

Anything outside `canon/std` and `canon/wasi` is a third-party package the community publishes — same mechanism (`canon install`), no privileged path.

### Tradeoffs

- **WASM-first means no direct OS handles.** A Canon program cannot, for example, embed a raw `std::fs::File`; it sees a `wasi:filesystem/types#descriptor` instead. This is the price of portability.
- **Phase-5 interfaces are scaffolded.** Where a `wasi:*` interface isn't yet usable from the canonical ABI (async filesystem, HTTP, server-side handlers), Canon ships an `canon:builtins/*` bridge that the embedded runtime fulfils. The user-facing API doesn't change when the bridge is later swapped for native WASI.
- **Hosts must support WASI Preview 3.** `canon run` embeds `wasmtime` with the P3 + component-model-async feature gates; other hosts will need equivalent support.

## Disambiguating Same-Typed Parameters

Canon has no named parameters — types serve as the documentation. When two components would share the same type, create a newtype alias.

Newtypes are **distinct but compatible**: a value of the original type can flow into a parameter of the alias, but the two are not interchangeable for disambiguation purposes.

Consider comparing two users by birthday:

```
User = Birthday * Username

compare = (User * User) -> Ord {
    User.Birthday.compare(User.Birthday)
}
```

This doesn't work — product members must be distinct types, so `(User * User)` is a compile error. Introduce a distinct alias:

```
User      = Birthday * Username
OtherUser = User

compare = (OtherUser * User) -> Ord {
    User.Birthday.compare(OtherUser.Birthday)
}
```

Because calling is commutative, both `alice.compare(bob)` and `bob.compare(alice)` are valid.

This is a deliberate design choice: types lie less than names.

## Strings

A `String` is `Byte^*` interpreted as UTF-8. Indexing yields bytes, not codepoints. Higher-level operations (grapheme iteration, etc.) are stdlib functions, not language built-ins.

### Ordering

Strings carry the same comparison surface as `Int` — `eq`, `ne`, `lt`, `le`, `gt`, `ge` — one spelling for comparison regardless of type. Order is byte-wise lexicographic, with the shorter string first on a shared prefix (`"app".lt("apple")` is `True`). This is the same order the compiler enforces on declarations, dispatch arms, and imports, now available to programs.

### Indexing Is 1-Based

All indexing in Canon is **1-based**, everywhere, matching positional product access (`byte.1` is the first bit): `s.byteAt(1)` is the first byte, `list.get(1)` the first element, and `s.substring(start, end)` is the **inclusive** slice `[start, end]` — `s.substring(1, 4)` is the first four bytes. "Position past the end" is `length + 1`, so an exhausted parser position satisfies `pos.gt(s.length())`. Unconventional relative to C-family languages, deliberate here: one origin for `.1`, `byteAt`, and `get` beats two origins used consistently never.

### String Escape Sequences

String literals support standard escape sequences:

| Sequence     | Meaning                          |
|--------------|----------------------------------|
| `\\`         | Backslash                        |
| `\"`         | Double quote                     |
| `\n`         | Newline (LF)                     |
| `\r`         | Carriage return (CR)             |
| `\t`         | Horizontal tab                   |
| `\0`         | Null byte                        |
| `\xNN`       | Byte by hex value (2 digits)     |
| `\uNNNN`     | Unicode scalar (4 hex digits)    |
| `\UNNNNNNNN` | Unicode scalar (8 hex digits)    |

An unrecognised escape sequence (e.g. `\q`) is a compile-time lexer error. There are no raw string literals.

## Comments

There are no comments. Code must speak for itself through types and naming.

## Operator Precedence

### Type-level (tightest first)

1. `T^N`, `T^*` — postfix repetition / Kleene star
2. `T<...>` — generic application
3. `*` — product
4. `+` — union

So `A + B * C^3` parses as `A + (B * (C^3))`.

### Expression-level (tightest first)

1. `.` — function call / field access / dispatch — PascalCase with `()` constructs, without `()` accesses a field
2. `()` — function application
3. `?` — postfix error propagation
4. `*` — value-level product (only inside a constructor argument)

So `foo.bar()?` is `((foo.bar)())?`.

## Glossary of Operators and Sigils

| Symbol     | Meaning                                  |
|------------|------------------------------------------|
| `+`        | Union (sum)                              |
| `*`        | Product                                  |
| `T^N`      | Fixed repetition (N copies)              |
| `T^*`      | Unbounded repetition (Kleene star)       |
| `<T>`      | Generic parameter                        |
| `<T: Tr>`  | Generic with trait constraint            |
| `.`        | Function call / field access / dispatch — `T()` constructs, `T` accesses field |
| `.( )`     | Dispatch on a union                      |
| `?`        | Propagate `Result` / `Option` failure    |

| `"..."`    | String literal sugar                     |
