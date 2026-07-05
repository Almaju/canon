# A Tour of Canon

One page, the whole language. Each section is the short version; the
[Language Specification](./spec/index.md) is the authoritative long
version, and every heading here links to its spec chapter for the
precise rules.

> *There is one way to do everything.* Most languages give you ten ways
> and ask you to pick. Canon picks for you, and the compiler enforces
> the choice.

## Philosophy

Three commitments explain almost every design decision. See
[Introduction](./introduction.md) for the same three with more prose.

**One way to do everything.** Wherever ordering is discretionary,
declarations must be alphabetical: product fields, union variants,
function declarations, dispatch arms. There is no `if`/`else` *and*
`match` — there is dispatch. No `while` *and* `for` *and* recursion —
there are collection methods and recursion. Reordering is never a
meaningful change; two programmers writing the same program produce the
same bytes.

**Types are the only names.** Canon has no local variables, no `let`, no
comments, no parameter names. A value is referred to by *its type*. To
disambiguate two values of the same type, introduce a newtype — the
newtype becomes the documentation.

```canon
compare = (OtherUser * User) -> Ord {
    User.Birthday.compare(OtherUser.Birthday)
}
```

**Having a value is having the capability.** No service singletons, no
permission system. Reading a file requires a `File`, obtainable only
from a `Path`, obtainable only from a `String`. The type chain *is* the
access control. There is no `unsafe` and no global mutable state.

## Types

Every type composes two operators — `+` ("or") and `*` ("and") — over a
small core of primitives. Types and traits are `PascalCase`; methods are
`camelCase`. Full rules: [Types](./spec/types.md).

```canon
Bool = False + True            # union: variants alphabetical, no `enum`
User = Birthday * Username     # product: fields alphabetical, no `struct`
Birthday = String              # newtype: distinct type, shared storage
```

Products are addressed by component type (`user.Birthday`) or, for
repeats, by 1-based index (`byte.1`). Generics use angle brackets
(`List<T>`, `Result<T, E>`), constraints use `:` (`<T: Print>`).
Recursive types are boxed automatically — there is no `Box<T>`. There is
**no type inference**: every type is written, and a declared type that
doesn't match the inferred shape is a compile error.

Values are built by calling a constructor — there is no `new`, no
`true`/`false` keyword. Literals desugar to constructors (`123` →
`Int(123)`, `"hi"` → `String("hi")`, `{"k":v}` → a `Json` value).
Zero-data values take empty parens: `Unit()`, `True()`, `None()`.
**Conversion is construction**: `String(42)`, `Int("42")` — no `parse`,
`toString`, or `from`/`into` family. A type opts into *validated*
construction by declaring a constructor with its own name:

```canon
Url = String

Url = (String) -> Result<Url, InvalidUrl> {
    String.parse()
}
```

A fallible constructor forces `?` at the call site
(`Url("https://example.com")?.get()`), and external callers cannot
bypass it — the raw inner value is reachable only inside the type's own
file.

## Functions

`name = (Components) -> ReturnType { body }`. The components form a
product (the input); any component can be the dot-receiver at the call
site (**commutative calling**). A body is newline-separated expressions;
the last is the return value. There are no semicolons and no local
variables — you thread values by chaining. Full rules:
[Functions and Traits](./spec/functions.md).

```canon
shout = (Greeting) -> String {
    "HELLO"
}

main = () -> Unit {
    Greeting("howdy").shout().print()
}
```

Free functions in a file are declared alphabetically (enforced).
Optional parameters are just `Option<T>`. Functions are first-class
(pass one by qualified name), and one-off operations are lambdas written
with a full signature — there is no signature inference:
`Numbers.map((Int) -> Int { Int.mul(3) })`. `main` is the entry point,
lifted as the component's `wasi:cli/run.run` export.

## Dispatch

There is no `if`/`else` or `match` keyword. All branching is **dispatch**
on a union: the value is the receiver, the arms go inside `.( )`, in the
union's (alphabetical) variant order, every variant spelled out. Full
rules: [Expressions and Dispatch](./spec/expressions.md).

```canon
True().(
    * (False) -> Unit { "no".print() }
    * (True)  -> Unit { "yes".print() }
)
```

Payload-carrying variants name the payload type, which is then in scope:
`* (Some<Int>) -> Unit { Int.print() }`. Dispatch also works by
**equality on `String`/`Int` scrutinees**, where arms are literals and a
mandatory catch-all naming the scrutinee's type is always last — this is
how routing works, with no route DSL:

```canon
path.(
    * ("/notes") -> Body { index() }
    * (String)   -> Body { notFound() }
)
```

Why no `if`? `if c then a else b` is dispatch on `Bool`; you already
need dispatch for unions, so a second construct would be a second way to
do one thing.

## Loops and recursion

There are no loop keywords. Iteration is higher-order methods on
collections (`map`, `get`, `length`, `first`, `append`, `concat`) or
plain recursion with dispatch supplying the base case. `Map` and `Set`
in the stdlib are built this way — recursive unions walked by recursive
functions.

```canon
sumTo = (Int) -> Int {
    Int.eq(0).(
        * (False) -> Int { Int.add(Int.sub(1).sumTo()) }
        * (True)  -> Int { 0 }
    )
}
```

## Effects and values

Canon has no separate effect or capability system. Effects emerge from
the values you construct and thread. Constructing a `File` (from a
`Path`) *is* opening it; you cannot `.read()` something that is not a
`File`. Full model: [Effects and the Async Model](./spec/effects-and-async.md).

```canon
main = () -> Unit {
    Path("./data.json").File()?.read()?.print()
}
```

`print = (String) -> Unit` is the one built-in that needs no threaded
value (lowered against `wasi:cli/stdout`). Every other effect appears in
the signature because the work needs the value: `save = (Database *
User) -> Result<Unit, DbError>`. No `UserRepository`, no
`DatabaseManager` — the `Database` value *is* the access.

## Async

Async is a property of **types, never of syntax**. There is no `async`
keyword and no `.await`; you write synchronous-looking code and the
compiler infers everything. Async enters through one door — a binding
whose WIT function is `async`, giving it a `Future<T>` return. Wherever a
`Future<T>` is used where `T` is expected, the compiler inserts the
await, and suspension propagates up the call graph automatically. Full
rationale and comparison table: [Effects and the Async
Model](./spec/effects-and-async.md).

```canon
main = () -> Unit {
    Url("https://example.com")?.get()?.body()?.print()
}
```

Parallelism is combinators over futures, entered through the receiver
like any other call — never a bare call: `a.parallel(b)` fans out and
awaits both; `a.race(b)` returns the first and cancels the loser.
`Stream<T>` is to `List<T>` as `Future<T>` is to `T`; a method returning
`Stream<T>` used in an iterating position becomes a poll loop (`.each`,
`.map`), with no `for await`.

## Modules

The module system is file-based. A file is `kebab-case.can` and **must**
be named after the type it declares (`http-server.can` ↔ `HttpServer`); a
module is a folder; the entry point is `main.can`. There is **no import
statement** — referring to `Foo` loads `foo.can` (or `foo/main.can`)
automatically, and the same rule reaches the embedded stdlib. A name
that resolves in more than one place is a compile error; there is no
shadowing and no private visibility. Full rules:
[Modules and Packages](./spec/modules.md).

## Traits

A trait is a callable type signature, declared like a function type and
therefore `PascalCase`. The case is how the compiler tells an
implementation (`Print`) from a plain method (`print`). Implement it by
declaring a function with the trait's name over the implementing type;
compose multiple methods with `*`.

```canon
Show = () -> String

Show = (Greeting) -> String {
    "HELLO!"
}
```

A trait can be a parameter type directly (`(Print) -> Unit { Print() }`)
or a generic constraint (`<T: Print>`). See
[Functions and Traits](./spec/functions.md).

## Errors

Errors are values carried by `Result<T, E>`; the error slot is a
regular type, so it can be an inline union — more ergonomic than a
dedicated enum per call site:

```canon
read = (File * Path) -> Result<Bytes, IoError + NotFound + PermissionDenied> {
    File.read(Path)?.decode()
}
```

Postfix `?` propagates failure on both `Result` (short-circuits the
error) and `Option` (short-circuits `None`), so pipelines read
top-down. Keep `Option` (absent) and `Result` (failed) distinct. Name
errors by *what failed* — `InvalidUrl`, `MalformedJson` — not by who
emitted them.

## Testing

The test framework is a union type, one helper, and a CLI verb — no
attributes, no macros, no runner config. A test is any function
returning `TestResult`; discovery is by signature, not by name.

```canon
testAddPositive = () -> TestResult {
    1.add(2).eq(3).assert("1 + 2 should be 3")
}
```

`canon test` runs them; the exit code is honest (`0` all-pass, `1` on
any failure), so it drops straight into CI. Put logic in pure helpers
that take and return values, keep the entry thin, and the testable
surface falls out for free. Layers and fixtures: see the repository's
`tests/` and `CLAUDE.md`.

## Serving HTTP

A program becomes an HTTP service by declaring **one free function that
returns `Response`** — no server object, no router, no port in the
program. The same entry-point-by-return-type rule that makes `main` a
CLI program makes this a service; exactly one function may return a
world type, and helpers return ordinary values.

```canon
serve = (Request) -> Response {
    Request.path().(
        * (None) -> Response { Response(notFound() * Headers() * Status(400)) }
        * (Some<String>) -> Response {
            String.(
                * ("/notes") -> Response { Response(index() * Headers() * Status(200)) }
                * (String)   -> Response { Response(notFound() * Headers() * Status(404)) }
            )
        }
    )
}
```

Routing is dispatch, not a DSL. `Response` is the product `Body *
Headers * Status`. `canon build` emits a standard `wasi:http/service`
component (imports only `wasi:*`, exports `wasi:http/handler#handle`) —
nothing in the artifact is Canon-specific. The worked example is
[notes-api](./examples/notes-api.md).

## Binding files

Canon compiles to a WebAssembly Component, so foreign functions bind to
**Component Model imports** by path — there is no FFI keyword. A
*binding file* is recognized by shape and path: an ordinary `.can` file
in a versioned package directory
(`<ns>/<name>@<version>/<iface>.can`) whose function declarations are
body-less. The path spells the interface; the declaration's kebab-case
form names the WIT function.

```canon
getRandomU64 = () -> Int
```

You rarely write these by hand — `canon bindgen <file.wit>` emits one
binding file per interface, and idioms (`Url`, `File`, `Now`) are plain
Canon wrappers over the raw bindings. Two namespaces appear:
standard `wasi:*` interfaces, and `canon:builtins/*` temporary host
bridges that migrate to `wasi:*` as each interface's canonical ABI
lands. Full mapping: [Compilation and the
ABI](./spec/compilation.md#the-wit--canon-mapping) and [Using WASI
Interfaces](./reference/wasi.md).
