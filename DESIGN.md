# Oneway

Oneway is a new programming language. The reference implementation transpiles to Rust — Oneway inherits Rust's ownership model and zero-cost abstractions, while presenting a much smaller surface area to the programmer.

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

With `extern Rust`, the pattern is the same but the body is omitted:

```
Url = String

extern Rust("oneway_url_parse")
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
use Foo          # load a local file or module
use std/Foo      # load a standard library type
```

Local imports (`use Foo`) look for `foo.ow` or `foo/main.ow` relative to the current file. Standard library types require the explicit `std/` prefix (`use std/JsonValue`, `use std/Url`, `use std/Now`). Each import names exactly one type — there are no wildcard imports. If you use `JsonValue` and `JsonArray`, you write both `use std/JsonValue` and `use std/JsonArray`.

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

`main` always compiles to an async entry point backed by tokio. There is no condition on this — Oneway targets async-first use cases and every program gets the async runtime. The programmer writes uniform, sync-looking code; the compiler handles async machinery based on what the program does.

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

Oneway has **no garbage collector**. The reference implementation transpiles to Rust and inherits Rust's ownership and borrowing rules. However, **ownership is invisible to the Oneway programmer**: there are no lifetimes, no `&` / `&mut` sigils at the value level, no explicit `Box` or `Rc`. The transpiler infers all of this from usage.

Rough mapping to Rust:

| Oneway                                  | Transpiled to                                  |
|-----------------------------------------|-------------------------------------------------|
| Non-`mut` parameter                     | `T` (moved) or `&T` (borrowed) — transpiler picks |
| `mut T` parameter                       | `&mut T`                                        |
| Recursive type (e.g. `Tree`)            | Auto-boxed (`Box<T>`)                           |
| Shared ownership the transpiler can't otherwise prove | `Rc<T>` / `Arc<T>`                  |

If the transpiler cannot find a valid ownership scheme for a given Oneway program, it is a compile-time error — equivalent to a Rust borrow-checker rejection. The error is surfaced in Oneway terms, not Rust terms.

## Mutability

Values are immutable by default. The `mut` keyword marks a **component** as mutable. There are no local variables, so there is nothing else `mut` can apply to.

```
add = (mut Counter) -> Unit {
    ...
}
```

`mut T` transpiles directly to `&mut T` in Rust: the caller's value is mutated in place.

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

`print = (String) -> Unit` is a built-in that writes to stdout. It requires nothing beyond a `String` — no capability token, no permission, no threading. This covers the overwhelming majority of output needs.

For redirectable output — writing to a file, a log sink, a test buffer — construct a `Fileout` from a `File` and pass it along:

```
logFn = (Fileout) -> Unit {
    "event occurred".print(Fileout)
}

logFn(Path("./app.log").File()?.Fileout())
```

`print = (Fileout * String) -> Unit` is the overload that writes to the given output instead of stdout. Functions that need configurable output declare `Fileout` as a parameter; functions that just want to emit to stdout call `.print` directly.

### Threading Effects

When a function performs a meaningful effect — reading a file, talking to a database — the relevant value appears in its signature. This is not enforced by a capability type system; it is the natural consequence of needing the value to do the work:

```
save = (Database * User) -> Result<Unit, DbError>
```

`user.save(database)` and `database.save(user)` are both valid (commutative calling). No `UserRepository`. No `DatabaseManager`. The `Database` value IS the access; having it means you can use it. You receive it because you had to construct it (from a connection string, a config, something real) and thread it to the functions that need it.

### Async

All programs compile to async Rust under tokio. There is no `async` keyword and no `.await` in Oneway source. The compiler handles this uniformly for all programs.

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

Oneway has no `async` keyword and no `.await` at the source level. All programs compile to async Rust under tokio. The programmer writes uniform function calls; the compiler emits the state-machine and await machinery.

`Task.spawn` maps to `tokio::spawn`. Channels map to `tokio::sync::mpsc`. A spawned task's body is an ordinary function.

## Interop With the Host Ecosystem

Oneway is **batteries-included**. Beyond the small type-system core, Oneway ships opinionated binding packages for the major application domains — HTTP server and client, filesystem, date/time, JSON, crypto, logging — each wrapping a chosen Rust crate (`axum`, `tokio::fs`, `serde_json`, `chrono`, etc.). Every type is individually importable with the `std/` prefix, so you declare exactly what you use.

The community is free to publish additional bindings, but the headline batteries ship with the language. This is the same pragmatic choice Go made (large stdlib + go.mod) and Python made (`stdlib` + PyPI): a curated default that lets new users be productive in minutes, with an open ecosystem behind it.

Under the hood, every binding is implemented via `extern Rust` declarations — there is no privileged path. Anyone could write the same bindings; Oneway just ships them so users don't have to.

### `extern Rust` Declarations

A type or function can be declared as backed by a Rust item. The transpiler emits direct calls — no runtime glue, no marshalling.

```
extern Rust("std::io::stdout")
print = (String) -> Unit

extern Rust("axum::Router")
HttpRouter

extern Rust("axum::Router::route")
route = (Handler * HttpRouter * Path) -> HttpRouter
```

> **First-class references.** `extern Rust` function declarations are entered into the value scope on equal footing with ordinary functions. They may be referenced as first-class values (`Type.fn`) and passed as callbacks wherever a matching function signature is expected.

#### Async Extern Items

To bind an `async fn` from Rust, use `extern Rust.async`. The compiler inserts `.await` at every call site, and the calling Oneway function is compiled as `async fn`:

```
extern Rust.async("axum::serve")
serve = (HttpRouter * HttpServer * TcpListener) -> Result<Unit, IoError>
```

Functions that call a `Rust.async` extern are automatically compiled as `async fn` by the transpiler. Any function that calls an async function is transitively made async. There is no annotation required on user-defined functions — the compiler propagates async-ness through the call graph.

### Dependency Manifest

Each Oneway project carries a manifest listing the Rust crates it depends on. The transpiler emits a `Cargo.toml` that mirrors it, and `oneway build` is a thin wrapper around `cargo build`.

```
[deps]
axum       = "0.7"
serde_json = "1"
sqlx       = "0.7"
```

### Binding Packages

Idiomatic Oneway code does not call `extern Rust` directly. Instead, it imports individual types from the shipped binding packages:

```
use std/HttpServer      # HTTP server (axum)
use std/ServerRequest   # incoming HTTP request
use std/HttpError       # HTTP client errors + get on Url (reqwest)
use std/JsonValue       # JSON parsing/emitting (serde_json)
use std/JsonArray       # JSON array operations
use std/Now             # current UTC time (chrono)
use std/File            # file I/O (tokio::fs)
```

Each `use std/X` imports exactly the named type along with its associated constructor and methods. The community can publish additional or alternative bindings under any path; the `std/` namespace is reserved for the shipped set.

### What Oneway Ships Itself

Two layers ship with the language:

**Core** — the small set of language-level primitives, owned by the compiler and not bindable to Rust crates:

- Type system primitives: `Off`, `On`, `Bit`, `Byte`, `Bytes`, `Unit`, `Never`
- Numeric and text: `Float`, `Hex`, `Int`, `String`
- Generic containers: `List<T>`, `Map<K, V>`, `Option<T>`, `Result<T, E>`, `Set<T>`
- Standard unions: `Bool`, `Ord`
- I/O built-ins: `print` (stdout), `File`, `Fileout`

`Map<K, V>` is a sorted key-value map backed by `BTreeMap`. `K` must implement `Ord`. Iteration order is alphabetical by key. `Set<T>` is a sorted set backed by `BTreeSet`; `T` must implement `Ord`.

**Batteries** — opinionated binding packages over major Rust crates, written in ordinary Oneway with `extern Rust` declarations:

- `HttpServer`, `ServerRequest`, `Port`, `RoutePath`, … (axum)
- `HttpError` + `get` on `Url` (reqwest)
- `Now`, `Datetime` (chrono)
- `File`, `Fileout`, `IoError` (tokio::fs)
- `Url`, `InvalidUrl` (url)
- `JsonValue`, `JsonArray`, `JsonObject`, `MalformedJson`, … (serde_json)
- `Database`, `DbError` (sqlx) — planned
- `Crypto`, `Log` (ring / rustls, tracing) — planned

Anything outside these two layers is a third-party binding the community publishes — same mechanism, no privileged path.

### Tradeoffs

- **Error messages may leak Rust types** when crossing the FFI boundary. Unavoidable to some degree; mitigated by good bindings.
- **Async-flavored crates** are bound via `Rust.async` externs. The cost — tokio in the dep tree, state-machine codegen — is uniform across all programs.
- **Oneway is permanently coupled to Rust** unless a second backend is later added. A real strategic dependency, accepted in exchange for sharing the entire Rust ecosystem.

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
