# Oneway

Oneway is a new programming language. The reference implementation compiles to a **WebAssembly Component** targeting **WASI Preview 3** — every Oneway program is a portable `.wasm` that runs on any Component Model host. The compiler itself is written in Rust and embeds `wasmtime` to make `oneway run` a single-step experience, but no Rust toolchain is required at build or run time and no Rust source is emitted along the way.

The language inherits Rust-style ownership and zero-cost abstractions through the compiler's analysis, while presenting a much smaller surface area to the programmer.

## Guiding Principle: Alphabetical Order Everywhere

Wherever ordering is discretionary, Oneway requires **alphabetical order**. This is not a style suggestion — it is enforced by the compiler. The rule applies to:

- Components of a product type: `User = Birthday * Username`
- Variants of a union type: `Bool = False + True`
- Type definitions within a file (declared top-to-bottom alphabetically)
- Function declarations within a file (declared top-to-bottom alphabetically)
- Arms of a dispatch (in the order of the union's variants — which are themselves alphabetical)
- Trait composition: `Show = Debug * PrintString`
- Error unions inside `Result`: `Result<T, IoError + NotFound + PermissionDenied>`
- Imports: multiple `use` statements at the top of a file

The reasoning: ordering is a constant source of bikeshedding and diff noise. By forcing one canonical order, code reads the same way no matter who wrote it, and reordering is never a meaningful change.

## Core Types

The language is built from two primitive types: `Off` and `On` (names TBD). Every other type is composed from these via unions and products.

Two identity types complete the algebra: `Unit` is the type with exactly one value (the multiplicative identity — `T * Unit ≡ T`), and `Never` is the type with zero values (the additive identity — `T + Never ≡ T`). Together with `+` and `*`, these form a type semiring.

A small set of built-in primitive operations (e.g. arithmetic on `Int`) is supplied by the compiler — these cannot be derived purely from bits, but their *shape* is still described by the type system.

## Type Composition

### Unions (`+`)

A union expresses "this or that":

```
Bit = Off + On
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
byte.1   // first Bit
byte.2   // second Bit
```

#### Newtypes Are 1-Component Products

A newtype alias `A = B` (where `B` is a single named type) is a degenerate product with one component, named `B`. The field-access rule applies uniformly:

```
Greeting = String

Greeting("hi").String   // unwrap to the underlying String
```

This composes with method lookup through the alias chain: `Greeting("hi").print()` works because `print` is inherited from `String` (no need to unwrap first). Methods declared explicitly on the newtype shadow the inherited ones.

For argument passing, newtypes are *substitutable* for their underlying type — a function expecting `String` accepts a `Greeting` directly, without `.String`. The unwrap form exists for cases where the explicit step is clearer (e.g. inside a dispatch arm where the bound name reads more naturally with the unwrap visible).

Multi-step aliases unwrap one step at a time: with `A = B` and `B = C`, write `aValue.B.C` to reach the bottom.

#### Field Access vs Construction

Because both field names and type constructors are PascalCase, the dot syntax would be ambiguous without a rule. Oneway resolves it with parentheses:

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
print = <T: Print>(List<T>) -> Unit {
    ...
}
```

### Type Arguments at Call Sites

Where Oneway cannot infer a generic function's type parameters from context, the caller pins them with `::<...>` after the function name (the same "turbofish" form Rust uses):

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
| `"abc"`        | `String(abc)`      |
| `0xFF0000`     | `Hex(0xFF0000)`    |

String literals exist to avoid the parsing ambiguity of bare `String(...)` with spaces and punctuation. Numeric literals exist to avoid boilerplate in arithmetic-heavy code.

#### Zero-Data vs Data-Carrying Constructors

`T()` with no arguments is valid **only** when `T` has no underlying composition — i.e., it is a zero-data type like `Unit`, `True`, `False`, or a union variant with no payload. These types have exactly one value; `()` simply signals "I am producing it."

`String()`, `Int()`, `User()` — calling any data-carrying constructor with no arguments is a compile-time error. If a value can legitimately be "missing", that absence belongs in the type as `Option<T>`; otherwise the type requires its data.

For factory-style construction (e.g. "an empty list"), use an explicit lowercase function — `List.empty()`, `String.empty()`.

### Zero-Data Types

A type with no underlying composition (e.g. `Unit`, `True`, `False`, `Off`, `On`) has exactly one value. In expression position, it is constructed with `T()` — the empty argument list signals that you are producing a value, not accessing a field:

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

So:

```
red  = Hex(0xFF0000)
user = User(Birthday(...) * Username("ahanot"))
```

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
extern "oneway:builtins/url@0.1.0"

Url = String

Url = (String) -> Result<Url, InvalidUrl>
```

#### Error Naming

Errors are types like any other, and they're named *semantically* — by what failed, not by who emitted them. `InvalidUrl`, `MalformedJson`, `FileNotFound`, `PermissionDenied` carry information; `UrlError`, `JsonError`, `FsError` don't. The exception is opaque wrappers around foreign error types (e.g., `HttpError` wrapping the entire `reqwest::Error` enum) where the underlying error space hasn't been decomposed into Oneway variants yet.

## Naming Conventions

- **Types**: `PascalCase`
- **Traits**: `PascalCase` (traits are types)
- **Functions**: `camelCase`

The case difference disambiguates trait implementations from regular functions: `print` is a function, `Print` is the implementation of the `Print` trait.

## File and Module Layout

- **Files** use `snake_case.ow` names (chosen for git/Linux compatibility).
- A file's name **must match** the type it declares: `foo.ow` must declare a type named `Foo`.
- A **module is a folder**. There is no `mod` declaration. Importing `Foo` from a sibling folder is enough.
- The entry point is `main.ow`; libraries live in `lib.ow`.

### Imports

```
use Foo                    # local: load `foo.ow` or `foo/main.ow` relative to this file
use models/User            # local: subfolder lookup, `models/user.ow`
use oneway/std/Json        # package: <namespace>/<package>/<Type>
use acme/image/Decoder     # third-party: same shape, no privileged path
```

There is exactly one `use` resolution rule. Given `use a/b/c/…/Z`:

1. If the leading segments `a/b` (the first two) match a declared dependency in the project's package manifest, resolve as a **package import**: locate the package in the cache, then look up the type `Z` (or, for multi-file packages, the file matching `Z`'s [kebab-case form](#naming-conventions)) inside it.
2. Otherwise, resolve as a **local import**: walk the segments as directories relative to the current file and load `z.ow` or `z/main.ow` at the end.

The shipped packages `oneway/std` and `oneway/wasi` are pre-installed and bundled with the compiler binary, but indistinguishable from any other package at the language level — they appear in the cache and must be listed as deps to be used.

Each import names exactly one type — there are no wildcard imports. If you use `JsonValue` and `JsonArray`, you write both `use oneway/std/JsonValue` and `use oneway/std/JsonArray`.

Packages have versions. The version pin lives in the project's package manifest (see [Package Manifests](#package-manifests)), not in source. `use oneway/std/Json` never carries an `@version`.

### Visibility

Everything is **public**. There is no private visibility modifier.

This is deliberate. The language already enforces radical transparency — no comments, no local variables, types as documentation. Hiding functions would cut against that philosophy. The one place encapsulation matters — protecting type invariants — is handled by [validated constructors](#validated-constructors): declaring a constructor replaces the total constructor, so the raw inner representation cannot be bypassed.

## Type Inference

There is **no type inference**. Every type must be explicitly written.

Additionally, every declared type must be *used*: if a function returns `Result<T, Err>` but no `Err` ever flows through, this is a compile-time error. Declared types must match inferred shape exactly.

## Functions

A function is declared as:

```
name = (components) -> ReturnType {
    ...
}
```

The components inside the parentheses form a product — the function's input. When a function takes multiple inputs, they are composed with `*`, the same operator that composes product types elsewhere in the language:

```
print = (Stdout * String) -> Unit {
    Stdout.write(String)
}
```

Components follow the same alphabetical-order rule as product members: `(Stdout * String)` is valid because `Stdout` precedes `String`; `(String * Stdout)` would be a compile error.

There are no commas in parameter lists, no positional arguments — only type composition. This is a deliberate unification: a function's input is a product type, described with the same `*` used everywhere else.

### Commutative Calling

At the call site, **any component can appear before the dot**. The remaining components are passed inside the parentheses:

```
"hello".print(Stdout)
Stdout.print("hello")
```

Both calls are equivalent. This follows from the commutativity of `*` in product types — `Stdout * String` and `String * Stdout` describe the same composition. The dot is syntax sugar: it selects which component of the product the caller writes to the left.

For a function with more than two components, the remaining components are passed as a product value:

```
route = (Handler * HttpRouter * Path) -> HttpRouter { ... }

router.route(Handler(...) * Path("/api"))
```

This is a genuinely novel feature: in most languages, the receiver is a privileged position — *the* object you're calling the method on. In Oneway, there is no privilege. A function is defined over a composition of types, and the caller enters it through whichever component reads most naturally in context.

### The Entry Point

`main` is the single exception. It takes no input and has no receiver — it is never called via dot syntax:

```
main = () -> Unit {
    "hello".print
}
```

`main` is lifted as the component's `wasi:cli/run.run` export. It is always emitted as an *async-stackful* function at the Component Model boundary so nested calls to suspending externs (filesystem, network, server handlers) can yield without trapping. The programmer writes uniform, sync-looking code; the compiler handles all of this.

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
Color = Blue + Green + Red
Blue  = Hex(0000FF)
Green = Hex(00FF00)
Red   = Hex(FF0000)

print = (Option<Color> * String) -> Unit {
    ...
}
```

This allows both forms at the call site:

```
"hello".print()
"hello".print(Red)
```

## No Local Bindings

Oneway has **no `let` keyword and no local variables**. This is deliberate.

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
        Equal   => Zero,
        Greater => Positive,
        Less    => Negative,
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

Oneway has **no garbage collector**. The compiler performs Rust-style ownership analysis and lowers each value to a concrete linear-memory layout in the emitted core wasm module. **Ownership is invisible to the Oneway programmer**: there are no lifetimes, no `&` / `&mut` sigils at the value level, no explicit `Box` or `Rc`. The compiler infers all of this from usage.

Rough mapping of source-level concepts to lowered wasm:

| Oneway                                  | Lowered to                                              |
|-----------------------------------------|---------------------------------------------------------|
| Non-`mut` parameter                     | Moved or borrowed value passed through wasm locals      |
| `mut T` parameter                       | Mutable reference through a linear-memory pointer       |
| Recursive type (e.g. `Tree`)            | Heap-allocated cell in the bump heap (auto-boxed)       |
| Shared ownership the compiler can't otherwise prove | Reference-counted cell                      |

If the compiler cannot find a valid ownership scheme for a given program, it is a compile-time error. The error is surfaced in Oneway terms.

## Mutability

Values are immutable by default. The `mut` keyword marks a **component** as mutable. There are no local variables, so there is nothing else `mut` can apply to.

```
add = (mut Counter) -> Unit {
    ...
}
```

`mut T` lowers to an in-place update of the caller's value through a linear-memory pointer.

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
# leading * on every arm (recommended for alignment)
bool.(
    * (False) -> Unit { "no".print(Stdout) }
    * (True)  -> Unit { "yes".print(Stdout) }
)

# leading * omitted from the first arm
bool.(
    (False) -> Unit { "no".print(Stdout) }
    * (True)  -> Unit { "yes".print(Stdout) }
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

Oneway does not have a separate capability or effect system. Effects emerge naturally from the values you construct and thread through your program.

### Domain-First Design

The guiding principle: **start with real domain objects and transform them**. There are no service singletons, no manager objects, no repository classes. A `Path` value represents a file path; transforming it to `File` represents opening that file; reading the `File` gives you a `String`. The type chain is the access control.

```
Path("./data.json").File()?.read()?.print
```

Having a `File` value *is* the capability to read that file. You cannot get a `File` without having constructed it from a `Path`, and you cannot construct a `Path` from nowhere — it requires a `String`. The type system enforces this naturally.

This applies everywhere:

```
# Fetching from the network — start with a Url
Url("https://api.example.com/data")?.get()?.print

# Current time — call the Now constructor directly
Now().toRfc3339().print

# HTTP server — start with a Port
Port(3000).HttpServer(State(Unit()))
    .get(RoutePath("/"), handler)
    .serve()

# JSON — start with a String
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

### Async

There is no `async` keyword and no `.await` in Oneway source. Both are inferred and inserted by the compiler.

A function is *suspending* if it (1) is a body-less declaration in a [binding file](#binding-files) whose corresponding WIT entry is `async func`, (2) consumes a `Future<T>` or iterates a `Stream<T>`, or (3) transitively calls a suspending function. The compiler computes this set bottom-up over the call graph and lifts the affected functions as `async func(…)` in the emitted Component Model world. Where a `Future<T>` value is consumed in a position that expects `T`, the loader automatically rewrites the call site into an `await` — the user writes neither keyword.

This is uniform: synchronous-looking code is the only style. The async machinery is invisible by design.

## Traits

A trait is a callable type signature. It is declared like a function type:

```
Print = <Error>() -> Result<Unit, Error>
```

Because traits are types, they are written in `PascalCase`.

### Multi-Method Traits

A trait with multiple methods is just a product of single-method traits:

```
Show = Debug * PrintString
```

### Default Implementations

A trait declaration can carry a default body marked `{ impl }`:

```
Greet = () -> String { impl }
```

Implementing types may then either override or inherit the default.

### Implementing a Trait

A trait is implemented for a type by declaring a function with the trait's name (PascalCase) and the implementing type as a component:

```
Print = (User) -> Result<Unit, IoError> {
    ...
}
```

This is distinguished from a regular function by case alone: `print` (camelCase) is a regular function, `Print` (PascalCase) is a trait implementation.

Multiple implementations of the same trait for different types:

```
Show = (Greeting) -> String { "HELLO!" }
Show = (Name) -> String { "Alice" }
```

### Using a Trait as a Parameter

A trait can be used directly as a component type. The component binds the trait implementation, which is then invocable:

```
needsPrint = (Print) -> Unit {
    Print()
}
```

### `Self`

There is no `Self` keyword. Inside a function body, components are referenced by their type names directly — every component is explicit in the signature, so there is no ambiguity and no need for an alias.

## Concurrency

Oneway has no `async` keyword and no `.await` at the source level. The compiler infers which functions are suspending and lifts them as `async func(…)` in the emitted component (see [Async](#async) above for the inference rules).

Task spawning, channels, and structured concurrency are planned and will be expressed as ordinary stdlib types over WASI Preview 3's task model — not as language keywords.

## Interop With the Host Ecosystem

Oneway compiles to a **WebAssembly Component**, so interop happens at the Component Model boundary rather than at a source-level FFI. Every Component-Model interface a program uses is exposed as a **binding package** — an ordinary Oneway package whose `.ow` files are binding files. The compiler emits the matching component world; any compliant host (`wasmtime`, browser polyfills, edge runtimes) satisfies the imports.

The shipped stdlib is **layered**:

- `oneway/wasi` — **raw bindings**, machine-generated from upstream WIT by `oneway gen-bindings`. One `.ow` file per WIT interface, each a [binding file](#binding-files). No idioms, no capability discipline, no opinions. Regenerated, never hand-edited.
- `oneway/std` — **curated wrappers**, hand-written. One primary type per file (`oneway/std/clock.ow` declares `Clock`, `oneway/std/file.ow` declares `File`, …). Methods, constructors, and capability arguments live here. Idiomatic Oneway code only ever imports from `oneway/std`.

Where a `wasi:*` interface isn't yet usable from the canonical ABI, the corresponding `oneway/wasi` file binds an `oneway:builtins/*` bridge instead. The split is invariant — only the WIT path on the file's `extern` header changes.

### Binding Files

A **binding file** is a `.ow` file whose first declaration is an `extern` header naming a Component-Model interface:

```
extern "wasi:random/random@0.3.0-rc-2026-03-15"

getRandomBytes = (Int) -> Bytes
getRandomU64 = () -> Int
```

The header pins the WIT interface this file binds. Every declaration in the file describes one symbol from that interface, mapped through the mechanical [WIT → Oneway mapping](#wit--oneway-mapping):

- **Function declarations are body-less.** Each function's name and signature must match a `func` (or `async func`) in the named WIT interface. The compiler verifies this when the package is loaded.
- **Type declarations** describe types defined by the WIT interface (records, variants, resources). They follow the same mechanical mapping.
- The `extern` header is the **only** way to introduce a Component-Model import. Body-less function declarations are **only legal inside a binding file** — anywhere else, a missing body is a compile error ("forgot a body").

The header string follows Component-Model path syntax: `namespace:package/interface@version`. The `@version` is optional for unversioned interfaces. There are no per-function path strings, no async annotations, no `from`/`sha256` fields on declarations — a binding file looks like ordinary Oneway code, just without bodies.

> **First-class references.** Functions declared in a binding file are entered into the value scope on equal footing with ordinary functions. They may be referenced as first-class values (`Type.fn`) and passed as callbacks wherever a matching function signature is expected.

#### Async

Async-ness is **not** declared in source. If the WIT marks a function `async func`, the binding's mechanical mapping gives it a `Future<T>` return type. The async-ness is then visible in the signature itself, and the suspension-propagation rule in [Async](#async) handles the rest. No annotation, no keyword.

### Package Manifests

Each package — shipped, installed, or local — has an `oneway.toml` at its root. The manifest is **TOML**, fixed schema:

```toml
# oneway/std/oneway.toml
name    = "oneway/std"
version = "0.1.0"

[deps]
"oneway/wasi" = "0.3.x"
```

A **project** is a package whose source includes a `main.ow` entry point and whose manifest lists its dependencies. Dependencies are pinned by semver constraint:

```toml
# my-app/oneway.toml
name    = "my-app"
version = "0.1.0"

[deps]
"oneway/std"         = "0.1.x"
"acme/image-decoder" = "1.0.x"
```

When a package's component must be **fetched** (rather than satisfied by the host), the package's own manifest declares it:

```toml
# acme/image-decoder/oneway.toml
name    = "acme/image-decoder"
version = "1.0.0"
from    = "https://components.example/image-decoder-1.0.0.wasm"
sha256  = "ab12cd34ef56…"
```

The compiler parses only the subset shown above (string values at the top level, a single `[deps]` table with quoted keys and string values). Full TOML compatibility is a non-goal; the format choice is for editor and tooling support, not for expressiveness.

Rules:

- `from` is **optional**. Absent means the package's bindings are host-provided (WASI, `oneway:builtins/*`). Present means the build tool must resolve it.
- `from` schemes: `https://…`, `file://…`, and `github:owner/repo@vX.Y.Z/path.wasm` (sugar that expands to the corresponding GitHub release-download URL).
- `sha256` is **required whenever `from` is present**. No "fetch on first run and trust". The hash is computed by `oneway install` at install time and written into the manifest by the tool; humans don't type it.
- Resolved components are cached at `~/.oneway/cache/<sha256>.wasm`. The cache is content-addressed and machine-independent, so the same `(from, sha256)` pair is reproducible across checkouts.
- `oneway build` with a cache miss while offline is a hard error. There is no implicit network access at build time — fetching is `oneway install`'s job.
- When `from` is present, `oneway build` **inlines** the fetched component as a nested instance in the output `.wasm`, producing a self-contained binary. (This is the role `wac plug` plays in the broader ecosystem; in Oneway it's a built-in build step.)

The lockfile is the manifest. There is no separate `oneway.lock`.

### Generating Bindings from WIT

The `oneway/wasi` layer is produced by:

```
oneway gen-bindings <path-or-url>
```

The input is either a `.wit` file (parsed directly) or a WebAssembly Component `.wasm` (the embedded `component-type` custom section is extracted and parsed). The output is a complete package: one [binding file](#binding-files) per WIT interface under `<namespace>/<package>/<interface>.ow`, plus an `oneway.toml` manifest. Output is deterministic and alphabetically ordered, so it round-trips through `oneway fmt` and produces clean diffs on regeneration.

`oneway install <url>` is a convenience that combines fetch + verify + `gen-bindings` + record:

1. Fetch the `.wasm` from the URL.
2. Compute its sha256. Confirm it is a valid Component.
3. Extract its WIT and emit a binding package, with its `oneway.toml` carrying `from = "<url>", sha256 = "<digest>"`.
4. Populate `~/.oneway/cache/<sha256>.wasm`.
5. Add the package to the project manifest's `deps`.

After `oneway install`, the consumer writes `use <namespace>/<package>/<Type>` and the rest is ordinary Oneway.

#### WIT → Oneway Mapping

The mapping is mechanical. Every shape on the left maps to exactly one shape on the right.

| WIT | Oneway |
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

**Identifier case** is converted mechanically: WIT `kebab-case` becomes Oneway `camelCase` for values and `PascalCase` for types. `get-resolution` → `getResolution`; `incoming-request` → `IncomingRequest`; the WIT interface `wasi:clocks/monotonic-clock` becomes the file path `wasi/clocks/monotonic_clock.ow` within the `oneway/wasi` package.

**Record field ordering.** WIT records have a positional ABI order; Oneway products are alphabetical. The generator preserves both: source-level fields are sorted alphabetically, and the canonical-ABI lowering reorders to the WIT layout. This reordering is invisible to user code.

#### Resources as `Handle`

WIT `resource` types are opaque, owning handles with methods and an implicit `drop`. Oneway models them as a single-field product wrapping a builtin `Handle`:

```
# oneway/wasi/filesystem/types.ow  (generated)
extern "wasi:filesystem/types@0.3.0"

Descriptor = Handle

readViaStream = (Descriptor * Int) -> Result<InputStream, ErrorCode>
```

- `Handle` is a language-level primitive. It is **non-copyable** and **non-printable**; the only operations on it are passing it to a binding function and going out of scope.
- Methods on a WIT resource become free functions in the generated binding, with the resource as the first parameter. The canonical-ABI `[method]resource.fn` form is matched to a body-less Oneway function whose first parameter is the corresponding resource type; the prefix is implicit and never written in source. Collisions are resolved by Oneway's normal dispatch rules.
- Static methods (`[static]resource.fn`) become free functions with no `Handle` parameter; constructors (`[constructor]resource`) become functions returning the resource type.
- When a `Handle` value goes out of scope, the compiler emits the matching `resource.drop` call. Ownership is linear: a `Handle` cannot be aliased, only moved.

This means `oneway/std/file.ow` can wrap `oneway/wasi/filesystem/types#Descriptor` in a clean type without the user ever touching a `Handle` directly — exactly the layering the stdlib split is for.

### Importing from Bindings

Idiomatic Oneway code does not import binding files directly. Instead, it imports curated wrappers from `oneway/std`:

```
use oneway/std/Clock           # wraps oneway/wasi/clocks/monotonic_clock
use oneway/std/File            # wraps oneway/wasi/filesystem/types (or oneway:builtins/* until P3 lands)
use oneway/std/HttpServer      # wraps oneway:builtins/http-server
use oneway/std/Now             # wraps oneway/wasi/clocks/wall_clock (or oneway:builtins/* until P3 lands)
use oneway/std/Random          # wraps oneway/wasi/random/random
use oneway/std/Url             # wraps oneway:builtins/url + oneway:builtins/http
```

Each `use oneway/std/X` imports exactly the named type along with its associated constructor and methods. The community can publish additional or alternative bindings under any namespace; the `oneway/` namespace is reserved for packages shipped with the language.

A direct `use oneway/wasi/clocks/monotonic_clock/now` works (everything is public), but you give up the capability discipline and the cleaned-up names that the `oneway/std` wrapper provides.

### What Oneway Ships Itself

Three things ship with the language:

**Core** — the small set of language-level primitives, owned by the compiler (not a package):

- Type system primitives: `Off`, `On`, `Bit`, `Byte`, `Bytes`, `Unit`, `Never`
- Numeric and text: `Float`, `Hex`, `Int`, `String`
- Generic containers: `List<T>`, `Map<K, V>`, `Option<T>`, `Result<T, E>`, `Set<T>`
- Standard unions: `Bool`, `Ord`
- Async wrappers (rarely written by users): `Future<T>`, `Stream<T>`
- I/O built-ins: `print` (stdout), wired against `wasi:cli/stdout`

`Map<K, V>` is a sorted key-value map. `K` must implement `Ord`. Iteration order is alphabetical by key. `Set<T>` is its set-shaped counterpart.

**Batteries** — two packages, bundled with the compiler binary and pre-populated in the cache:

- **`oneway/wasi`** — generated binding files against `wasi:*` (and `oneway:builtins/*` for interfaces that don't yet have a stable canonical-ABI shape). Mechanically produced by `oneway gen-bindings`.
- **`oneway/std`** — hand-written wrappers presenting a clean, capability-disciplined API. One primary type per file.

| Wrapper | Underlying binding | Status |
|---|---|---|
| `oneway/std/Clock` (capability) + `oneway/std/Instant` | `oneway/wasi/clocks/monotonic_clock` | ✅ |
| `oneway/std/Exit` (`Int.exit()`) | `oneway:builtins/cli` (will move to `wasi/cli/exit` once narrow-int codegen lands) | ✅ |
| `oneway/std/Random` | `oneway/wasi/random/random` | ✅ |
| `oneway/std/Now` (RFC 3339 wall-clock time) | `oneway/wasi/clocks/wall_clock` (today: `oneway:builtins/clock`) | ✅ |
| `oneway/std/File`, `oneway/std/Path`, `oneway/std/IoError` | `oneway/wasi/filesystem/types` (today: `oneway:builtins/filesystem`) | ✅ |
| `oneway/std/Url`, `oneway/std/InvalidUrl`, `oneway/std/HttpError` | `oneway:builtins/url` + `oneway:builtins/http` | ✅ — will move to `wasi/http/outgoing_handler` |
| `oneway/std/HttpServer`, `oneway/std/Request`, `oneway/std/HttpResponseBody`, `oneway/std/HttpStatus`, `oneway/std/Body`, `oneway/std/Port`, `oneway/std/RoutePath` | `oneway:builtins/http-server` | ⏳ stub host; real `.serve()` semantics pending |
| `oneway/std/Json` (string handle) | — | ⏳ `JsonValue` / `JsonArray` / `JsonObject` parser pending |
| `oneway/std/TestResult` (`Pass` / `Fail` + `assert`) | pure Oneway | ✅ |

The `oneway:builtins/*` interfaces are temporary scaffolds. Each one moves to the corresponding `wasi:*` interface as that interface's canonical-ABI shape (async, streams, resources) becomes available — the binding file in `oneway/wasi` is regenerated, the `oneway/std` wrapper stays the same.

Anything outside `oneway/std` and `oneway/wasi` is a third-party package the community publishes — same mechanism (`oneway install`), no privileged path.

### Tradeoffs

- **WASM-first means no direct OS handles.** A Oneway program cannot, for example, embed a raw `std::fs::File`; it sees a `wasi:filesystem/types#descriptor` instead. This is the price of portability.
- **Phase-5 interfaces are scaffolded.** Where a `wasi:*` interface isn't yet usable from the canonical ABI (async filesystem, HTTP, server-side handlers), Oneway ships an `oneway:builtins/*` bridge that the embedded runtime fulfils. The user-facing API doesn't change when the bridge is later swapped for native WASI.
- **Hosts must support WASI Preview 3.** `oneway run` embeds `wasmtime` with the P3 + component-model-async feature gates; other hosts will need equivalent support.

## Disambiguating Same-Typed Parameters

Oneway has no named parameters — types serve as the documentation. When two components would share the same type, create a newtype alias.

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
| `mut`      | Mutable binding                          |
