# Extern Rust

Oneway is **batteries-included**: opinionated binding packages for the major
application domains (filesystem, HTTP, database, JSON, …) ship with the
language. Each binding is implemented in ordinary Oneway via `extern Rust`
declarations over a chosen Rust crate. There is no privileged path — anyone
can write the same bindings; Oneway just ships a curated default so users
don't have to.

The mechanism behind every binding is `extern Rust`, which lets an Oneway
type or method be declared as backed by a Rust item. The transpiler emits
direct calls — no runtime glue, no marshalling.

## Declaring an Extern Method

```oneway
OtherInt = Int

extern Rust("std::cmp::min")
min = (Int * OtherInt) -> Int

main = (Stdout) -> Unit {
    5.min(3).print(Stdout)
}
```

The string is the fully qualified Rust path. The Oneway signature declares
how the method is called from Oneway.

A path that begins with `.` indicates a method call on the first component, not a
free function:

```oneway
extern Rust(".to_lowercase")
toLower = (String) -> String

extern Rust(".to_uppercase")
toUpper = (String) -> String

main = (Stdout) -> Unit {
    "Hello, World".toUpper().print(Stdout)
    "GoodBye".toLower().print(Stdout)
}
```

## Extern Types

A type alias to a Rust type is declared the same way, with no body:

```oneway
extern Rust("std::io::Error")
IoError

extern Rust("reqwest::Error")
HttpError
```

The Oneway-side name (`IoError`, `HttpError`) is a transparent alias for the
Rust type, suitable for use in `Result<T, E>` positions or anywhere else the
Rust type is meaningful.

## Async Externs

Async Rust functions are bound with `extern Rust.async`:

```oneway
extern Rust.async("tokio::fs::read_to_string")
read = (Filesystem * Path) -> Result<String, IoError>
```

The compiler inserts `.await` at every call site, and the calling Oneway
function is itself compiled as `async fn`. From the Oneway side, the call
looks like any other method invocation — there is no `async` keyword and
no `.await`. The async machinery is driven by the **suspending-capability**
mechanism (see [Capabilities](capabilities.md)): a function that receives
a suspending capability or calls a `Rust.async` extern is compiled to
async Rust automatically.

An `extern Rust.async` declaration is valid only on a method whose receiver
or parameters include a suspending capability — typically `Network` or
`Filesystem`. This keeps the capability set honest: async effects must be
reflected in the type, not slipped in through an extern declaration.

## Dependency Manifest

Each Oneway project carries a manifest listing the Rust crates it depends
on. The transpiler emits a `Cargo.toml` that mirrors it, and `oneway build`
is a thin wrapper around `cargo build`.

```
[deps]
axum       = "0.7"
serde_json = "1"
sqlx       = "0.7"
```

For programs that only use shipped binding packages, the manifest is empty
— each binding pulls in its own crate deps automatically when imported.

## Binding Packages

Idiomatic Oneway code does not call `extern Rust` directly. Instead, it
imports from the shipped binding packages:

```oneway
use Filesystem    # wraps tokio::fs
use HttpClient    # wraps reqwest
use HttpServer    # wraps axum
use Database      # wraps sqlx
use Json          # wraps serde_json
```

A binding package is a few hundred lines of Oneway declarations plus
minimal ergonomic glue, written once and shipped with the language. The
community can publish additional or alternative bindings; the shipped set
is just the curated default.

## Tradeoffs

- **Error messages may leak Rust types** when crossing the FFI boundary.
  Unavoidable to some degree; mitigated by good bindings.
- **Async-flavored crates** are bound via `Rust.async` externs and the
  suspending-capability mechanism, so the no-keyword promise is preserved
  while still using async crates natively. The cost — tokio in the dep
  tree, state-machine codegen — is paid only by programs that actually
  take a suspending capability.
- **Oneway is permanently coupled to Rust** unless a second backend is
  later added. A real strategic dependency, accepted in exchange for
  sharing the entire Rust ecosystem.
