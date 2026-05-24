# Extern Wasm

Oneway compiles to a **WebAssembly Component**, so foreign functions
aren't bound to a host language — they're bound to **Component Model
imports** declared by their fully-qualified path. The mechanism is
`extern Wasm`, and it works the same whether the import is a standard
WASI interface (`wasi:random/random#get-random-u64`) or a per-application
bridge (`oneway:builtins/url@0.1.0#parse`).

The compiler emits the matching `(import …)` and lowers each call through
the canonical ABI — no source-level marshalling, no manual buffer
management.

## Declaring an Extern Function

```oneway
extern Wasm("wasi:random/random@0.3.0-rc-2026-03-15#get-random-u64")
getRandomU64 = () -> Int

main = () -> Unit {
    getRandomU64().print()
}
```

The string follows Component Model path syntax:

```
namespace : package / interface @ version # function
```

- The `namespace:package/interface` part identifies a WIT interface — a
  group of related functions, types, and resources.
- The `@version` is optional and only present on versioned interfaces
  (every WASI 0.3-rc interface, for example).
- The `#function` selects one function inside the interface.

The Oneway signature declares how the function is called from Oneway. The
compiler resolves Oneway types against the WIT-level types: `Int` is
`s64`, `String` is `string`, `Result<T, E>` is `result<T, E>`, and so on.

## Extern Types

A type that wraps a foreign value is declared the same way, with no body:

```oneway
HttpError = String
```

There is no need to mark the *type* as `extern`; types in Oneway are
always plain shapes. What matters is whether the *constructor* is an
extern function:

```oneway
Url = String

extern Wasm("oneway:builtins/url@0.1.0#parse")
Url = (String) -> Result<Url, InvalidUrl>
```

The type declaration on the first line is an ordinary Oneway newtype.
The `extern Wasm` block on the second line replaces the implicit total
constructor — so `Url("https://example.com")` calls the host-provided
parser and yields a `Result<Url, InvalidUrl>`.

## Async Externs

Suspending Component imports are bound with `extern Wasm.async`:

```oneway
extern Wasm.async("oneway:builtins/http-server@0.1.0#serve")
serve = (HttpServer) -> Result<Unit, IoError>
```

The compiler lowers the call site through the *async* canonical ABI
(`canon lower … async`), which means:

- The host can yield before producing a result.
- The Oneway function that contains the call is marked **suspending** and
  lifted as `async func(…)` in the emitted component.
- Every transitive caller of a suspending function is automatically
  marked suspending too.

You write no `async` keyword, no `await`, no `.await`. A `Future<T>`
returned by a suspending call is implicitly awaited when used in a
position that expects `T`.

## No Project Manifest

There is no `Cargo.toml`, no `package.json`, no per-project dependency
file. The set of imports a program needs is fully determined by its
`extern Wasm` declarations and resolved at component-instantiation time
by the host. `oneway build` produces a `.wasm` plus a sibling `.wit`
describing the component's world.

## Generating Bindings from WIT

Writing `extern Wasm` declarations by hand is rarely necessary. The
compiler can read a WIT file (or a `.wasm` component, whose embedded
WIT is extracted automatically) and emit one Oneway binding file per
interface:

```sh
oneway bindgen path/to/my-component.wit -o .
```

This writes `<ns>/<pkg>/<iface>.ow` for each interface, alphabetically
ordered, ready to `use`. The mapping is mechanical: WIT records become
products, variants become unions, `list<T>` becomes `List<T>`, kebab-case
becomes Oneway camelCase / PascalCase, and so on. See `DESIGN.md` for the
full table.

The `wasi/…` bindings shipped with the compiler are produced this way
from the WIT files vendored under `wit-vendor/wasi/`. Regenerate them
with `just regen-bindings`.

## Binding Packages

Idiomatic Oneway code does not write `extern Wasm` directly. Instead, it
imports individual types from the embedded standard library:

```oneway
use std/Instant       # Instant()  — monotonic clock     — wasi/clocks/monotonic_clock
use std/File          # File / read                       — oneway:builtins/filesystem
use std/HttpServer    # HttpServer / get / post / serve   — oneway:builtins/http-server
use std/Now           # Now()      — RFC 3339 wall-clock — oneway:builtins/clock
use std/Random        # Random()   — random Int          — wasi/random/random
use std/Url           # Url + get on Url                   — oneway:builtins/url + oneway:builtins/http
```

Each `use std/X` brings in the named type along with its constructor and
methods. Behind the scenes those modules are written in ordinary Oneway
on top of the generated `wasi/…` bindings; there is no privileged path.

See the [Standard Library](../reference/stdlib.md) reference for the
complete list and the WIT interfaces behind each module.

## `wasi:*` vs `oneway:builtins/*`

You'll see two namespaces in `extern Wasm` paths:

- `wasi:*` — standard [WASI](https://github.com/WebAssembly/WASI)
  interfaces. Any compliant host satisfies them.
- `oneway:builtins/*` — temporary host bridges that `oneway run`
  implements internally. Each one will move to the corresponding `wasi:*`
  interface as that interface's canonical-ABI shape (async, streams,
  resources) becomes available.

From a user's perspective both look identical: `use std/Foo` and call
methods. The bridge swap is invisible.

## Tradeoffs

- **No direct OS handles.** A Oneway program cannot embed a raw
  `std::fs::File` or a `tokio::net::TcpStream`; it sees the corresponding
  `wasi:*` resource handle instead. This is the price of portability.
- **Phase-5 interfaces are scaffolded.** Where a `wasi:*` interface isn't
  yet usable from the canonical ABI, Oneway ships an
  `oneway:builtins/*` stand-in. The user-facing API doesn't change when
  the bridge is later swapped for native WASI.
- **Hosts must support WASI Preview 3.** `oneway run` embeds `wasmtime`
  with the P3 + component-model-async feature gates; other hosts will
  need equivalent support.
