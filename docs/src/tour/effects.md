# Effects and Values

Canon does not have a separate effect or capability system. Effects
emerge from the values you construct and thread through your program.

## Domain-First Design

The guiding principle: **start with real domain objects and transform
them toward what you need**. There are no service singletons, no manager
objects. Access to a resource is represented by holding a value of the
corresponding type.

```canon
Unit => Program {
    Path("./data.json")
        -> File?
        .read()?
        -> Print
}
```

`File` is a type. Constructing one (from a `Path`) *is* the act of
opening the file. You cannot call `.read()` on something that is not a
`File`. The type chain enforces access without any separate permission
system.

## Print

`print = (String) => Unit` is a built-in that writes to stdout. No
capability token, no parameter to thread; any `String` can be printed:

```canon
"hello".print()
42.print()
```

`.print` is lowered against the standard `wasi:cli/stdout` interface, so
the compiled `.wasm` runs on any Component Model host. The trailing
newline is emitted automatically.

For redirectable output (a file, a log sink, a test buffer) you'd
construct an explicit destination value (a `File` once its write side
lands, or a `Fileout` newtype) and pass it as an additional component.
The mechanism is the same as any other effect: thread the value.

## Threading Effects

When a function performs a meaningful effect (reading a file, talking
to a database, listening on a socket), the relevant value appears in
its signature. No capability type system enforces this; it is the
natural consequence of needing the value to do the work:

```canon
save = (Database * User) => Result<Unit, DbError>
```

`user.save(database)` and `database.save(user)` are both valid
(commutative calling). No `UserRepository`. No `DatabaseManager`. The
`Database` value *is* the access. You receive it because you had to
construct it (from a connection string or config) and thread it to
functions that need it.

## Async

There is no `async` keyword and no `.await` in Canon source. Both are
inferred by the compiler. A function is **suspending** if it (1) is
declared `extern Wasm.async(…)`, (2) consumes a `Future<T>` or iterates
a `Stream<T>`, or (3) transitively calls a suspending function. The
compiler propagates this through the call graph and lifts the affected
functions as `async func(…)` in the emitted component world.

Where a `Future<T>` value is used in a position that expects `T`, the
compiler inserts an implicit `await`. You write neither keyword.

## Domain Examples

Current time (RFC 3339) and Unix seconds:

```canon
Now().print()
Unix().print()
```

A random integer:

```canon
Random().print()
```

An HTTP GET starts with a `Url`:

```canon
Url("http://example.com")? -> Fetched?.print()
```

Reading a file starts with a `Path`:

```canon
Path("./Cargo.toml").File()?.read()?.print()
```

To serve HTTP, declare a `(Request) => Response` function; the
program *is* the server (see [Serving HTTP](./http.md)):

```canon
Request => Response {
    Response(Body("hello") * Headers() * Status(200))
}
```

The pattern is always the same: construct a real value, transform it,
use it. No singletons. No service locators. No permission tokens.
(The stdlib types in each snippet need no import — references resolve
automatically; Canon has no comments, so the prose lives out here.)
