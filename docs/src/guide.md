# A Tour of Canon

One page, the whole language. Each section is the short version; the
[Language Specification](./spec/index.md) is the authoritative long
version, and every heading here links to its spec chapter for the
precise rules. Canon is mid-migration to **[Types-Only
Canon](./spec/types-only.md)** — the forms shown here are the direction
the language is heading, and what the examples compile with today.

> *There is one way to do everything.* Most languages give you ten ways
> and ask you to pick. Canon picks for you, and the compiler enforces
> the choice.

## Philosophy

Three commitments explain almost every design decision. See
[Introduction](./introduction.md) for the same three with more prose.

**One way to do everything.** Wherever ordering is discretionary,
declarations must be alphabetical: product fields, union variants,
declarations, dispatch arms. There is no `if`/`else` *and* `match` —
there is dispatch. No `while` *and* `for` *and* recursion — there are
collection operations and recursion. Reordering is never a meaningful
change; two programmers writing the same program produce the same bytes.

**Types are the only names.** Canon has no local variables, no `let`, no
comments, no parameter names — and no function names either. Every named
callable is a `PascalCase` type, and an operation is identified by *what
it produces*. A value is referred to by its type; to disambiguate two
values of the same type, introduce a newtype — the newtype is the
documentation.

**Having a value is having the capability.** No service singletons, no
permission system. Reading a file requires a `File`, obtainable only
from a `Path`, obtainable only from a `String`. The type chain *is* the
access control.

Three symbols carry three non-overlapping jobs: **`=>` declares** (every
constructor, shape, lambda, and dispatch arm), **`->` executes** (pipes
a value through an operation), and **`.` reads** (field access only).

## Types

Every type composes two operators — `+` ("or") and `*` ("and") — over a
small core of primitives. Types are `PascalCase`. Full rules:
[Types](./spec/types.md).

```canon
Bool = False + True            # union: variants alphabetical, no `enum`
User = Birthday * Username     # product: fields alphabetical, no `struct`
Birthday = String              # newtype: distinct type, shared storage
```

Products are read by component type (`user.Birthday`) or, for repeats,
by 1-based index (`byte.1`). Generics use angle brackets (`List<T>`,
`Result<T, E>`), constraints use `:` (`<T: Print>`). Recursive types are
boxed automatically — there is no `Box<T>`. There is **no type
inference**: every type is written.

Values are built by a **constructor** — there is no `new`, no
`true`/`false` keyword. Literals desugar (`123` → `Int(123)`, `"hi"` →
`String("hi")`, `{"k":v}` → a `Json` value). Zero-data values take empty
parens: `Unit()`, `True()`, `None()`. **Conversion is construction**:
`String(42)`, `Int("42")` — no `parse`, `toString`, or `from`/`into`
family. A constructor is just an arrow to the type it builds, and a
*validated* one returns `Result`/`Option`:

```canon
Url = String

String => Result<Url, InvalidUrl> {
    String -> Parsed
}
```

A fallible constructor forces `?` at the call site
(`"https://example.com" -> Url? -> Get`), and external callers cannot
bypass it — the raw inner value is reachable only inside the type's own
file.

## Constructors

A constructor is `(Input) => Output { body }`, and it is **named after
the type it produces** — so it needs no name of its own. A body is
newline-separated expressions; the last is the value. There are no
semicolons and no local variables — you thread values with the `->`
pipe. Full rules: [Functions and Traits](./spec/functions.md).

```canon
Greeter = String

Shout = String

Greeter => Shout {
    "HELLO"
}

Unit => Program {
    "hi" -> Greeter -> Shout -> Print
}
```

`Unit` is the name of "no input", so a nullary constructor is `Unit =>
X`. The CLI entry is `Unit => Program`, selected by its return type
exactly as an HTTP handler is selected by returning `Response`. The pipe
is **commutative**: any component of the input product can be the value
piped in, the rest ride in parens (`alice -> Compare(bob)`). One-off
operations are lambdas — the same arrow with no top-level type: `List(1
* 2 * 3) -> Mapped((Int) => Int { Int -> Product(2) })`.

Operations that transform a value keep its type, so they take a **result
newtype** named for what they produce — `Joined = String`, `Inserted =
Map` — and newtype substitutability makes chaining free.

## Dispatch

There is no `if`/`else` or `match` keyword. All branching is **dispatch**
on a union: pipe the value into `-> ( )`, with one arm per variant in the
union's (alphabetical) order, every variant spelled out. Full rules:
[Expressions and Dispatch](./spec/expressions.md).

```canon
True() -> (
    * False => Unit { "no" -> Print }
    * True  => Unit { "yes" -> Print }
)
```

Payload-carrying variants name the payload type, which is then in scope:
`* Some<Int> => Unit { Int -> Print }`. Dispatch also works by
**equality on `String`/`Int` scrutinees**, where arms are literals and a
mandatory catch-all naming the scrutinee's type is always last — this is
how routing works, with no route DSL:

```canon
Path -> (
    * "/notes" => Body { Index() }
    * String   => Body { NotFound() }
)
```

Why no `if`? `if c then a else b` is dispatch on `Bool`; you already
need dispatch for unions, so a second construct would be a second way to
do one thing.

## Loops and recursion

There are no loop keywords. Iteration is operations on collections
(`Mapped`, `At`, `Length`, `First`, `Joined`) or plain recursion with
dispatch supplying the base case. `Map` and `Set` in the stdlib are
built this way — recursive unions walked by recursive constructors.

```canon
Chain => Int {
    Chain -> (
        * Link => Int { Link.Next -> Len -> Sum(1) }
        * Stop => Int { 0 }
    )
}
```

## Effects and values

Canon has no separate effect or capability system. Effects emerge from
the values you construct and thread. Constructing a `File` (from a
`Path`) *is* opening it; you cannot read something that is not a `File`.
Full model: [Effects and the Async Model](./spec/effects-and-async.md).

```canon
Unit => Program {
    "./data.json" -> Path -> File? -> Read? -> Print
}
```

`Print` is the one operation that needs no threaded value. Every other
effect appears in the signature because the work needs the value: a
constructor over `(Database * User)` *is* the access — no
`UserRepository`, no `DatabaseManager`. An effect can also produce
**evidence**: a write yields a `Written` value, and a downstream
constructor that takes `(Written)` requires proof the write happened.

## Async

Async is a property of **types, never of syntax**. There is no `async`
keyword and no `.await`; you write straight-line code and the compiler
infers everything. Async enters through one door — a binding whose WIT
function is `async`, giving it a `Future<T>` return. Wherever a
`Future<T>` is used where `T` is expected, the compiler inserts the
await, and suspension propagates up the call graph automatically. Full
rationale and comparison table: [Effects and the Async
Model](./spec/effects-and-async.md).

```canon
Unit => Program {
    "https://example.com" -> Url? -> Get? -> Body? -> Print
}
```

Parallelism is combinators over futures, entered through the pipe:
`a -> Parallel(b)` fans out and awaits both; `a -> Race(b)` returns the
first and cancels the loser. `Stream<T>` is to `List<T>` as `Future<T>`
is to `T`; a `Stream<T>` used in an iterating position becomes a poll
loop, with no `for await`.

## Modules

The module system is file-based. A file is `kebab-case.can` and **must**
be named after the type it declares (`http-server.can` ↔ `HttpServer`); a
module is a folder; the entry point is `main.can`. There is **no import
statement** — referring to `Foo` loads `foo.can` (or `foo/main.can`)
automatically, and the same rule reaches the embedded stdlib. Referencing
a name that is genuinely ambiguous at the use site is a compile error;
there is no private visibility. Full rules: [Modules and
Packages](./spec/modules.md).

## Shapes and traits

When an operation's meaning spans types — `Length` over `Map`, `Set`,
`String`, `List` — it is a **shape**: a named signature with no body,
`PascalCase` because it is a type, implemented per type by a bodied
declaration of the same name.

```canon
Length = (String) => Int

Show = () => String
```

A shape can be a parameter type directly, or a generic constraint
(`<T: Print>`). This is what traits were; under Types-Only, shapes and
constructor families are one concept — *a `PascalCase` name is a family
of implementations selected by input product*. See [Functions and
Traits](./spec/functions.md).

## Errors

Errors are values carried by `Result<T, E>`; the error slot is a regular
type, so it can be an inline union — more ergonomic than a dedicated
enum per call site:

```canon
File * Path => Result<Bytes, IoError + NotFound + PermissionDenied> {
    File -> Read(Path)? -> Decoded
}
```

Postfix `?` propagates failure on both `Result` (short-circuits the
error) and `Option` (short-circuits `None`), so pipelines read
top-down. Keep `Option` (absent) and `Result` (failed) distinct. Name
errors by *what failed* — `InvalidUrl`, `MalformedJson` — not by who
emitted them.

## Testing

The test framework is a union type, one constructor, and a CLI verb — no
attributes, no macros, no runner config. A test is any constructor
returning `TestResult`; discovery is by signature, not by name.

```canon
testAddPositive = () => TestResult {
    1 -> Sum(2) -> Eq(3) -> TestResult("1 + 2 should be 3")
}
```

`TestResult` is built from a `Bool` and a message (`Pass` on `True`,
`Fail` carrying the message on `False`). `canon test` runs every such
declaration; the exit code is honest (`0` all-pass, `1` on any
failure), so it drops straight into CI. Put logic in constructors that
take and return values, keep the entry thin, and the testable surface
falls out for free.

## Serving HTTP

A program becomes an HTTP service by declaring **one arrow that returns
`Response`** — `Request => Response`, no server object, no router, no
port in the program. The same entry-point-by-return-type rule that makes
`Unit => Program` a CLI program makes this a service; exactly one arrow
may return a world type, and helpers return ordinary values.

```canon
Request => Response {
    Request.path() -> (
        * None => Response { 400 -> Status -> Response(Headers() * NotFound()) }
        * Some<String> => Response {
            String -> (
                * "/notes" => Response { 200 -> Status -> Response(Headers() * Index()) }
                * String   => Response { 404 -> Status -> Response(Headers() * NotFound()) }
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
(`<ns>/<name>@<version>/<iface>.can`) whose declarations are body-less.
The path spells the interface; the declaration's kebab-case form names
the WIT function. **Binding files are the one place `camelCase` is
legal** — camelCase in a Canon program means exactly "this identifier is
foreign".

```canon
getRandomU64 = () => Int
```

You rarely write these by hand — `canon bindgen <file.wit>` emits one
binding file per interface, and idioms (`Url`, `File`, `Now`) are plain
Canon constructors over the raw bindings. Two namespaces appear:
standard `wasi:*` interfaces, and `canon:builtins/*` temporary host
bridges that migrate to `wasi:*` as each interface's canonical ABI
lands. Full mapping: [Compilation and the
ABI](./spec/compilation.md#the-wit--canon-mapping) and [Using WASI
Interfaces](./reference/wasi.md).
