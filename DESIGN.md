# Oneway

Oneway is a new programming language. The reference implementation transpiles to Rust — Oneway inherits Rust's ownership model and zero-cost abstractions, while presenting a much smaller surface area to the programmer.

## Guiding Principle: Alphabetical Order Everywhere

Wherever ordering is discretionary, Oneway requires **alphabetical order**. This is not a style suggestion — it is enforced by the compiler. The rule applies to:

- Components of a product type: `User = Birthday & Username`
- Variants of a union type: `Bool = False | True`
- Multiple method/trait declarations on a type (declared top-to-bottom alphabetically)
- Arms of a `match` (in the order of the union's variants — which are themselves alphabetical)
- Trait composition: `Show = Debug & PrintString`
- Error unions inside `Result`: `Result<T, IoError | NotFound | PermissionDenied>`
- Imports: multiple `use` statements at the top of a file

The reasoning: ordering is a constant source of bikeshedding and diff noise. By forcing one canonical order, code reads the same way no matter who wrote it, and reordering is never a meaningful change.

## Core Types

The language is built from two primitive types: `Off` and `On` (names TBD). Every other type is composed from these via unions and products.

A small set of built-in primitive operations (e.g. arithmetic on `Int`) is supplied by the compiler — these cannot be derived purely from bits, but their *shape* is still described by the type system.

## Type Composition

### Unions (`|`)

A union expresses "this or that":

```
Bit = Off | On
```

### Products (`&`)

The `&` operator expresses "this and that" — a value of the resulting type has all of its component parts.

```
Byte = Bit & Bit & Bit & Bit & Bit & Bit & Bit & Bit
```

> **Note**: `&` is technically a product type operator, not a true type-theoretic intersection. The symbol is reused because it reads naturally as "has-a".

#### Product Members Are Alphabetical

By the global alphabetical-order rule, the components of a product are always written in alphabetical order:

```
User = Birthday & Username
```

The same applies to construction:

```
User(Birthday(...) & Username(...))
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

### Fixed Repetition (`Type[N]`)

For a fixed count of the same type, use `Type[N]`:

```
Byte = Bit[8]
```

The `[]` syntax was chosen because `.` is reserved for method calls and field access, and `[]` does not conflict with the `<>` generic syntax.

### Unbounded Repetition (`...Type`)

For unbounded sequences:

```
Bytes = ...Byte
```

This pairs naturally with `Bit[8]` as its finite counterpart.

Higher-level types like `Int`, `Float`, and `String` are defined from `Byte`/`Bytes`.

## Generics

Types can be parameterized by other types using angle brackets:

```
List<T>
Option<T>
Result<T, E>
Map<String, Int>
```

The chevron syntax does not conflict with `[]` repetition or `&` product.

### Generic Constraints

Constraints on type parameters use `:`, naming a trait the parameter must implement:

```
List.print = <T: Print>() -> Noop {
    ...
}
```

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

#### No Empty Constructors

`String()`, `Int()`, `User()` — calling any constructor with zero arguments is a compile-time error. The reasoning: if a value can legitimately be "missing", that absence belongs in the type as `Option<T>`; otherwise the type requires its data.

For factory-style construction (e.g. "an empty list"), use an explicit method on the type — `List.empty`, `String.empty`, etc.

### Singleton Types

A type with no underlying composition (e.g. `Noop`, `Off`, `On`) has exactly one value. The value is referenced by writing the type name itself:

```
main = () -> Noop {
    Noop
}
```

`Noop` in return position is the type; `Noop` in expression position is its sole value. No constructor call is needed (and would not work — there is no data to pass).

## Constructor Arguments

Every type `T` has a constructor `T(_)`. The argument is a value matching the type's underlying definition:

| Kind             | Constructor                            | Argument is…                                  |
|------------------|----------------------------------------|-----------------------------------------------|
| Primitive        | `Int(123)`, `Float(1.0)`, `String("hi")` | a literal of the corresponding lexical kind   |
| Hex              | `Hex(0xFF0000)`                        | a hex literal                                  |
| Product `A & B`  | `T(A(...) & B(...))`                   | a value-level product joined with `&`          |
| Union `A \| B`   | `T(A(...))` or `T(B(...))`              | a value of any variant                         |
| Newtype          | `T(inner)`                             | a value of the aliased type                    |

So:

```
red  = Hex(0xFF0000)
user = User(Birthday(...) & Username("ahanot"))
```

`&` is overloaded across the two levels: at the type level it forms a product type, at the value level it forms a product value. The two never appear in the same context.

### Validated Constructors (`Type.Self`)

By default, a type's constructor is total: `T(inner)` always succeeds and returns `T`. For types whose construction can fail — `Url` from a `String`, `Email` from a `String`, parsing in general — the construction belongs in the type system as `Result<T, E>` (or `Option<T>` for "this might just not exist"), the same way "missing" is expressed as `Option<T>`.

A type opts into this by declaring its constructor explicitly, as a method whose name is `Self`:

```
Url = String

extern Rust("oneway_url_parse")
Url.Self = (String) -> Result<Url, UrlError>
```

`Self` is already the alias for the receiver type's name; `Type.Self` is read as "the constructor that produces a `Self`." The convention slots naturally into the existing PascalCase / camelCase rule, alongside trait implementations like `Type.Print`.

**Rules:**

- If a type declares `Type.Self`, that *is* the constructor. The implicit total constructor is replaced.
- The signature is unconstrained — total (`(String) -> Url`), fallible (`Result<Url, UrlError>`), or optional (`Option<Url>`).
- Call sites use the ordinary constructor syntax: `Url("https://example.com")`. The expression's type is whatever `Url.Self` returns, so a fallible constructor *forces* `?` (or `match`) at the call site.
- External callers cannot bypass the constructor. Only methods declared on `Type` itself (in the same file) have access to the type's raw inner representation.

```
main = (HttpClient & Stdout) -> Result<Noop, HttpError | UrlError> {
    HttpClient.get(Url("https://example.com")?)?.print(Stdout)
    Ok(Noop)
}
```

This generalizes the same principle the language already applies to absence: if a value can legitimately be invalid, the fallibility belongs in the type, not in a runtime convention.

## Naming Conventions

- **Types**: `PascalCase`
- **Traits**: `PascalCase` (traits are types)
- **Methods (functions)**: `camelCase`

The case difference disambiguates trait implementations from regular methods on the same type: `Type.print` is a method, `Type.Print` is the implementation of the `Print` trait.

## File and Module Layout

- **Files** use `snake_case.ow` names (chosen for git/Linux compatibility).
- A file's name **must match** the type it declares: `foo.ow` must declare a type named `Foo`.
- A **module is a folder**. There is no `mod` declaration. Importing `Foo` from a sibling folder is enough.
- The entry point is `main.ow`; libraries live in `lib.ow`.

### Imports

```
use Foo
```

This imports `Foo` from the corresponding file/folder. No paths, no aliasing required at the import site.

### Visibility

Everything is **public by default**. To mark a method as private, prefix it with `*`:

```
Type.*helper = () -> Noop {
    ...
}
```

## Type Inference

There is **no type inference**. Every type must be explicitly written.

Additionally, every declared type must be *used*: if a function returns `Result<T, Err>` but no `Err` ever flows through, this is a compile-time error. Declared types must match inferred shape exactly.

## Implementations

Every function is implemented on a type. The general form is:

```
Type.functionName = (params) -> ReturnType {
    ...
}
```

### The Entry Point

`main` is the single exception. It is a top-level free function — not a method on any type — and is the program's entry point. It typically takes the capabilities the program needs:

```
main = (Stdout) -> Noop {
    "hello".print(Stdout)
}
```

### Referring to the Receiver

Inside a method body, the receiver value is referenced by **the receiver type's name**:

```
String.print = (Stdout) -> Noop {
    Stdout.write(String)    // `String` here is the receiver value
}
```

The `Self` keyword is an alias for the receiver type's name, available everywhere. It is required when the receiver's type name collides with a parameter of the same type:

```
Int.add = (Int) -> Int {
    ...   // ambiguous: which `Int`?
}
```

The above is a compile error. Resolve it in one of two ways:

**(a) Use `Self` for the receiver:**

```
Int.add = (Int) -> Int {
    Self.plus(Int)
}
```

**(b) Introduce a newtype alias for the parameter:**

```
OtherInt = Int

Int.add = (OtherInt) -> Int {
    Int.plus(OtherInt)
}
```

Both are valid. `Self` is the lighter-weight choice for one-off uses; the alias is the choice when the distinction is meaningful enough to warrant a name.

### Example

```
String.print = () -> Noop {
    ...
}
```

### Declaration Order

Multiple methods on the same type must be declared in alphabetical order:

```
User.add    = (...) -> ...
User.export = (...) -> ...
User.remove = (...) -> ...
```

This is a compile-time requirement, not a convention.

### Optional Parameters via `Option<T>`

There is no special syntax for optional parameters. Optionality is expressed through the type system using `Option<T>`:

```
Color = Blue | Green | Red
Blue  = Hex(0000FF)
Green = Hex(00FF00)
Red   = Hex(FF0000)

String.print = (Option<Color>) -> Noop {
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

- `match` is an expression — it can be the final line of a body, or appear as a sub-expression.
- `while` and `for` are expressions of type `Noop`.
- Non-final lines whose results are discarded are valid (they exist for side effects or `?` propagation).

```
User.compare = (OtherUser) -> Ord {
    User.Birthday.compare(OtherUser.Birthday)
}

File.readConfig = (Path) -> Result<Config, IoError | ParseError> {
    File.read(Path)?
        .parse()?
        .validate()
}

Int.classify = () -> Sign {
    match Int.compare(Int(0)) {
        Equal   => Zero,
        Greater => Positive,
        Less    => Negative,
    }
}
```

Without `let`, the only way to thread a value through multiple operations is method chaining. That is the intended style.

## First-Class Functions

Methods are first-class values. You refer to a method by its qualified name `Type.method` and pass it where a matching trait signature is expected:

```
Numbers = ...Int

Numbers.doubleAll = () -> Numbers {
    Numbers.map(Int.double)
}
```

### Lambdas

For one-off operations, write a lambda literal with its **full signature**. There is no signature inference.

```
Numbers.tripleAll = () -> Numbers {
    Numbers.map((Int) -> Int { Int.mul(Int(3)) })
}
```

Lambda syntax mirrors method declaration syntax: `(params) -> ReturnType { body }`. The only difference is the absence of a `Type.name =` prefix.

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

Values are immutable by default. The `mut` keyword marks a **parameter** as mutable. There are no local variables, so there is nothing else `mut` can apply to.

```
Counter.add = (mut Counter) -> Noop {
    ...
}
```

`mut T` transpiles directly to `&mut T` in Rust: the caller's value is mutated in place.

## Recursive Types

Recursive type definitions are allowed and **boxed automatically** by the compiler — there is no user-visible `Box<T>`:

```
Tree   = Branch | Leaf
Branch = Left & Right & Value
Left   = Tree
Right  = Tree
Value  = Int
```

Whether the compiler boxes `Left` and `Right` individually or via some other indirection is an implementation choice; it is never spelled out in source.

## Control Flow

### Pattern Matching

There is no `if`/`else`. All branching is via `match` on a union:

```
match ord {
    Equal   => ...,
    Greater => ...,
    Less    => ...,
}
```

Match arms follow the union's variant order, which is itself alphabetical.

Both `Bool` and `Ord` are ordinary union types in the standard library:

```
Bool = False | True
Ord  = Equal | Greater | Less
```

### Loops

Standard imperative loop constructs are available: `while`, `for`, plus higher-order forms on collections (`map`, `fold`, etc.). The exact iteration protocol is TBD.

## Error Handling

Errors are values, carried by the standard `Result<T, E>` type. The error slot is a regular type, so it can be a union written inline:

```
File.read = (Path) -> Result<Bytes, IoError | NotFound | PermissionDenied> {
    ...
}
```

This is more ergonomic than Rust's approach, where each call site typically needs a dedicated error enum.

### The `?` Operator

The postfix `?` operator propagates failure. It works on both `Result<T, E>` and `Option<T>`:

- On `Result<T, E>`: short-circuits with the error, otherwise unwraps to `T`.
- On `Option<T>`: short-circuits with `None`, otherwise unwraps to `T`.

```
Type.functionName = (params) -> ReturnType {
    Foo.test()?
    Foo.test2()?
}
```

### Option vs Result

`Option<T>` and `Result<T, Empty>` are structurally similar but **kept distinct**: `None` means "absent", `Err(_)` means "failed". The semantic difference is worth the duplication.

## Side Effects and Capabilities

A function's type should not lie about what it does. `String.print = () -> Noop` claims "nothing happens", but writing to stdout is something.

Oneway models effects as **capabilities** — values that must be passed in to perform an effect. A function that prints requires a `Stdout` capability:

```
String.print = (Stdout) -> Noop {
    ...
}
```

The only place to obtain real-world capabilities is `main.ow`, which receives them and threads them down. A function that does not receive a capability cannot perform the corresponding effect.

This requires no new mechanism — capabilities are just types, passed as ordinary arguments — and it makes effects honest at the type level without monads or a separate effect system.

### Suspending vs Non-Suspending Capabilities

Capabilities are split into two kinds based on whether their effects can wait on the outside world:

- **Non-suspending**: `Clock`, `Random`, `Stderr`, `Stdin`, `Stdout`. Their methods complete without yielding to a scheduler.
- **Suspending**: `Filesystem`, `Network`. (Future: `Database`.) Their methods may park the caller while the OS or a remote system responds.

A function compiles to `async fn` in Rust if and only if it transitively requires a suspending capability or calls a `Rust.async` extern. Otherwise it compiles to a plain `fn`. This is invisible at the source level — the programmer writes ordinary methods and ordinary calls — but it is what carries the "color" of a function. The capability parameter is the color; no separate `async` keyword exists.

This is similar in spirit to Haskell's `IO` propagation, but implemented via ordinary capability values rather than a monadic wrapper, and split per-effect rather than collapsed into a single `IO`. A function's signature already had to declare every effect it uses; that signature now also tells the compiler whether to emit async machinery.

The propagation rule is the same as for any other capability: if you call something that needs `Network`, your function must declare `Network` in its parameters. The compiler verifies; it does not infer the signature for you. This matches Oneway's existing no-type-inference rule.

## Traits

A trait is a callable type signature. It is declared like a function type:

```
Print = <Error>() -> Result<Noop, Error>
```

Because traits are types, they are written in `PascalCase`.

### Multi-Method Traits

A trait with multiple methods is just a product of single-method traits:

```
Show = Debug & PrintString
```

### Default Implementations

A trait declaration can carry a default body marked `{ impl }`:

```
Greet = () -> String { impl }
```

Implementing types may then either override or inherit the default.

### Implementing a Trait

A trait is implemented on a type by assigning to `Type.TraitName`:

```
User.Print = () -> Result<Noop, IoError> {
    ...
}
```

This is distinguished from a regular method (`Type.print`) by case alone.

### Using a Trait as a Parameter

A trait can be used directly as a parameter type. The parameter binds the trait implementation, which is then invocable:

```
Type.needsPrint = (Print) -> Noop {
    Print()
}
```

### `Self`

`Self` always refers to the receiver type's name from inside a method or trait implementation. It is an alias, not a separate identity — `Self` and the type's literal name (`String`, `Int`, …) are interchangeable. The only reason `Self` exists is to disambiguate when a parameter shares the receiver's type. See [Referring to the Receiver](#referring-to-the-receiver).

## Concurrency

Oneway has no `async` keyword and no `.await` at the source level. Functions are uniform: any function may call any function, and the syntax of the call is the same in either case.

Underneath, the compiler reads the capability set to decide how to emit each function. A function that transitively requires a [suspending capability](#suspending-vs-non-suspending-capabilities) (or calls a `Rust.async` extern) compiles to `async fn` in Rust, with `.await` inserted at every relevant call site. A function that only uses non-suspending capabilities — or none at all — compiles to a plain `fn`. The "color" is real, but it lives in the capability parameter the programmer was already threading through; there is no separate keyword for it.

`main` is compiled to `#[tokio::main] async fn main()` if any reachable function is async; otherwise it stays a plain `fn main()`. Pure-compute programs link no async runtime, pay no per-call state-machine overhead, and produce small binaries — the cost of async is paid only by programs that actually take a suspending capability.

The concurrency model is Go-flavored: lightweight tasks and channels. `Task.spawn` maps to `tokio::spawn` when the program is async-colored and to `std::thread::spawn` otherwise. Channels map to `tokio::sync::mpsc` or `std::sync::mpsc` accordingly. A spawned task's body declares the capabilities it uses, same rule as any other function.

## Interop With the Host Ecosystem

Oneway is **batteries-included**. Beyond the small type-system core, Oneway ships opinionated binding packages for the major application domains — HTTP server and client, filesystem, database, JSON, crypto, logging — each wrapping a chosen Rust crate (`axum`, `tokio::fs`, `sqlx`, `serde_json`, etc.). The user gets a single curated import path per domain (`use HttpServer`, `use Filesystem`, `use Database`) without having to evaluate the Rust crate ecosystem.

The community is free to publish additional bindings, but the headline batteries ship with the language. This is the same pragmatic choice Go made (large stdlib + go.mod) and Python made (`stdlib` + PyPI): a curated default that lets new users be productive in minutes, with an open ecosystem behind it.

Under the hood, every binding is implemented via `extern Rust` declarations — there is no privileged path. Anyone could write the same bindings; Oneway just ships them so users don't have to.

### `extern Rust` Declarations

A type or method can be declared as backed by a Rust item. The transpiler emits direct calls — no runtime glue, no marshalling.

```
extern Rust("std::io::stdout")
Stdout

extern Rust("std::println")
String.print = (Stdout) -> Noop

extern Rust("axum::Router")
HttpRouter

extern Rust("axum::Router::route")
HttpRouter.route = (Handler & Path) -> HttpRouter
```

#### Async Extern Items

To bind an `async fn` from Rust, use `extern Rust.async`. The compiler inserts `.await` at every call site, and the calling Oneway function is compiled as `async fn`:

```
extern Rust.async("axum::serve")
HttpServer.serve = (HttpRouter & TcpListener) -> Result<Noop, IoError>
```

A `Rust.async` extern is valid only on a method whose receiver or parameters include a suspending capability — typically `Network` or `Filesystem`. This keeps the capability set honest: async effects must still be reflected in the type, not slipped in through an extern declaration.

### Dependency Manifest

Each Oneway project carries a manifest listing the Rust crates it depends on. The transpiler emits a `Cargo.toml` that mirrors it, and `oneway build` is a thin wrapper around `cargo build`.

```
[deps]
axum       = "0.7"
serde_json = "1"
sqlx       = "0.7"
```

### Binding Packages

Idiomatic Oneway code does not call `extern Rust` directly. Instead, it imports from the shipped binding packages:

```
use HttpServer    # wraps axum
use HttpClient    # wraps reqwest
use Filesystem    # wraps tokio::fs
use Database      # wraps sqlx
use Json          # wraps serde_json
```

Each binding is a few hundred lines of Oneway declarations plus minimal ergonomic glue, written once and shipped with the language. The community can publish additional or alternative bindings; the shipped set is just the curated default.

### What Oneway Ships Itself

Two layers ship with the language:

**Core** — the small set of language-level primitives, owned by the compiler and not bindable to Rust crates:

- Type system primitives: `Off`, `On`, `Bit`, `Byte`, `Bytes`
- Numeric and text: `Float`, `Hex`, `Int`, `String`
- Generic containers: `List<T>`, `Map<K, V>`, `Option<T>`, `Result<T, E>`
- Standard unions: `Bool`, `Ord`
- Capability types: `Clock`, `Filesystem`, `Network`, `Random`, `Stderr`, `Stdin`, `Stdout` (see [Suspending vs Non-Suspending Capabilities](#suspending-vs-non-suspending-capabilities))

**Batteries** — opinionated binding packages over major Rust crates, written in ordinary Oneway with `extern Rust` declarations:

- `HttpServer` (axum), `HttpClient` (reqwest)
- `Filesystem` (tokio::fs), `Database` (sqlx)
- `Json` (serde_json), `Crypto` (ring / rustls), `Log` (tracing)

Anything outside these two layers is a third-party binding the community publishes — same mechanism, no privileged path.

### Tradeoffs

- **Error messages may leak Rust types** when crossing the FFI boundary. Unavoidable to some degree; mitigated by good bindings.
- **Async-flavored crates** are bound via `Rust.async` externs and the suspending-capability mechanism, so the no-keyword promise is preserved while still using async crates natively. The cost — tokio in the dep tree, state-machine codegen — is paid only by programs that actually take a suspending capability.
- **Oneway is permanently coupled to Rust** unless a second backend is later added. A real strategic dependency, accepted in exchange for sharing the entire Rust ecosystem.

## Disambiguating Same-Typed Parameters

Oneway has no named parameters — types serve as the documentation. When two parameters would share the same type, create a newtype alias.

Newtypes are **distinct but compatible**: a value of the original type can flow into a parameter of the alias, but the two are not interchangeable for disambiguation purposes.

Consider comparing two users by birthday:

```
User = Birthday & Username

User.compare = (User) -> Ord {
    User.Birthday.compare(User.Birthday)
}
```

This doesn't work — there is no way to tell the two `User` values apart. Introduce a distinct alias for the second one:

```
User      = Birthday & Username
OtherUser = User

User.compare = (OtherUser) -> Ord {
    User.Birthday.compare(OtherUser.Birthday)
}
```

This is a deliberate design choice: types lie less than names.

## Strings

A `String` is `...Byte` interpreted as UTF-8. Indexing yields bytes, not codepoints. Higher-level operations (grapheme iteration, etc.) are stdlib functions, not language built-ins.

## Comments

There are no comments. Code must speak for itself through types and naming.

## Operator Precedence

### Type-level (tightest first)

1. `T[N]` — postfix repetition
2. `...T` — prefix spread
3. `T<...>` — generic application
4. `&` — product
5. `|` — union

So `A | B & C[3]` parses as `A | (B & (C[3]))`.

### Expression-level (tightest first)

1. `.` — method call / field access
2. `()` — function application
3. `?` — postfix error propagation
4. `&` — value-level product (only inside a constructor argument)

So `foo.bar()?` is `((foo.bar)())?`.

## Glossary of Operators and Sigils

| Symbol     | Meaning                                  |
|------------|------------------------------------------|
| `\|`       | Union                                    |
| `&`        | Product                                  |
| `Type[N]`  | Fixed repetition (N copies)              |
| `...Type`  | Unbounded repetition                     |
| `<T>`      | Generic parameter                        |
| `<T: Tr>`  | Generic with trait constraint            |
| `.`        | Method call / field access               |
| `?`        | Propagate `Result` / `Option` failure    |
| `*name`    | Private method (file-local)              |
| `"..."`    | String literal sugar                     |
| `mut`      | Mutable binding                          |
