# Effects and Values

Oneway does not have a separate effect or capability system. Effects emerge naturally from the values you construct and thread through your program.

## Domain-First Design

The guiding principle: **start with real domain objects and transform them toward what you need**. There are no service singletons, no manager objects. Access to a resource is represented by holding a value of the corresponding type.

```oneway
Path("./data.json").File()?.read()?.print
```

`File` is a type. Constructing one (from a `Path`) *is* the act of opening the file. You cannot call `.read()` on something that is not a `File`. The type chain enforces access without any separate permission system.

## Print

`print = (String) -> Unit` is a built-in that writes to stdout. No capability token, no parameter to thread — any `String` can be printed:

```oneway
"hello".print
42.print
```

For **redirectable output** — writing to a log file, a test buffer, a named sink — construct a `Fileout` from a `File` and pass it to functions that need it:

```oneway
logFn = (Fileout) -> Unit {
    "event occurred".print(Fileout)
}

logFn(Path("./app.log").File()?.Fileout())
```

`print = (Fileout * String) -> Unit` is the overload that writes to the given output instead of stdout. Functions that need configurable output declare `Fileout` as a parameter; functions that just want stdout call `.print` directly.

## Threading Effects

When a function performs a meaningful effect — reading a file, talking to a database — the relevant value appears in its signature. This is not enforced by a capability type system; it is the natural consequence of needing the value to do the work:

```oneway
save = (Database * User) -> Result<Unit, DbError>
```

`user.save(database)` and `database.save(user)` are both valid (commutative calling). No `UserRepository`. No `DatabaseManager`. The `Database` value *is* the access. You receive it because you had to construct it (from a connection string or config) and thread it to functions that need it.

## Async

All Oneway programs compile to async Rust under tokio. There is no `async` keyword and no `.await` in Oneway source. The compiler handles async machinery uniformly — you write ordinary function calls and the compiler does the rest.

## Domain Examples

```oneway
# JSON — start with a String
"[1, 2, 3]".JsonValue()?.JsonArray()?.length().print

# HTTP request — start with a Url
Url("https://api.example.com/data")?.get()?.print

# HTTP server — start with a Port
Port(3000)
    .HttpServer(State(Unit()))
    .get(RoutePath("/"), handler)
    .serve()

# Database — thread the connection
save = (Database * User) -> Result<Unit, DbError>
```

The pattern is always the same: construct a real value, transform it, use it. No singletons. No service locators. No permission tokens.
