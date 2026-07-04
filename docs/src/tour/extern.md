# Extern Wasm

Canon compiles to a **WebAssembly Component**, so foreign functions
aren't bound to a host language — they're bound to **Component Model
imports** declared by their fully-qualified path. The mechanism is
`extern Wasm`, and it works the same whether the import is a standard
WASI interface (`wasi:random/random#get-random-u64`) or a per-application
bridge (`canon:builtins/url@0.1.0#parse`).

The compiler emits the matching `(import …)` and lowers each call through
the canonical ABI — no source-level marshalling, no manual buffer
management.

## Declaring an Extern Function

```canon
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

The Canon signature declares how the function is called from Canon. The
compiler resolves Canon types against the WIT-level types: `Int` maps
to the WIT integer at its declared width (`u8` through `s64` — for
`wasi:*` imports the vendored WIT is consulted, so narrow widths are
honoured at the ABI), `String` is `string`, `Result<T, E>` is
`result<T, E>`, and so on.

Hand-written `extern Wasm("<urn>")` is the explicit form. Generated
binding files use the equivalent `bindings "<urn>"` directive at the
top of the file, with each function below it declared as a bare
function-type alias — see
[Using WASI Interfaces](../reference/wasi.md).

## Extern Types

A type that wraps a foreign value is declared the same way, with no body:

```canon
HttpError = String
```

There is no need to mark the *type* as `extern`; types in Canon are
always plain shapes. What matters is whether the *constructor* is an
extern function:

```canon
Url = String

extern Wasm("canon:builtins/url@0.1.0#parse")
Url = (String) -> Result<Url, InvalidUrl>
```

The type declaration on the first line is an ordinary Canon newtype.
The `extern Wasm` block on the second line replaces the implicit total
constructor — so `Url("https://example.com")` calls the host-provided
parser and yields a `Result<Url, InvalidUrl>`.

## Async Externs

Suspending Component imports are bound with `extern Wasm.async`:

```canon
extern Wasm.async("canon:builtins/http-server@0.1.0#serve")
serve = (HttpServer) -> Result<Unit, IoError>
```

The compiler lowers the call site through the *async* canonical ABI
(`canon lower … async`), which means:

- The host can yield before producing a result.
- The Canon function that contains the call is marked **suspending** and
  lifted as `async func(…)` in the emitted component.
- Every transitive caller of a suspending function is automatically
  marked suspending too.

You write no `async` keyword, no `await`, no `.await`. A `Future<T>`
returned by a suspending call is implicitly awaited when used in a
position that expects `T`.

## The Manifest's Role

A package's `canon.toml` declares WIT *sources* under `[imports]`
(a `.wit` file, a directory, or a `.wasm` component) and
`canon install` materializes them into `bindgen/` as Canon binding
files. The compiled component's import list is then fully determined
by the `extern Wasm`/`bindings` declarations the program actually
uses, and resolved at component-instantiation time by the host.
`canon build` produces a `.wasm` plus a sibling `.wit` describing the
component's world.

## Generating Bindings from WIT

Writing `extern Wasm` declarations by hand is rarely necessary. The
compiler can read a WIT file (or a `.wasm` component, whose embedded
WIT is extracted automatically) and emit one Canon binding file per
interface:

```sh
canon bindgen path/to/my-component.wit -o .
```

This writes `<ns>/<pkg>/<iface>.can` for each interface, alphabetically
ordered, ready to `use`. The mapping is mechanical: WIT records become
products, variants become unions, `list<T>` becomes `List<T>`, kebab-case
becomes Canon camelCase / PascalCase, and so on. See `DESIGN.md` for the
full table.

The `wasi/…` bindings shipped with the compiler are produced this way
from the WIT files vendored under `wit-vendor/wasi/`. Regenerate them
with `just regen-bindings`.

## Binding Packages

Idiomatic Canon code does not write `extern Wasm` directly. Instead, it
imports individual types from the embedded standard library:

```canon
use canon/std/Random
use canon/std/fs/File
use canon/std/http/Url
use canon/std/time/Instant
use canon/std/time/Now
```

(`Instant()` — monotonic clock via `wasi/clocks/monotonic_clock`;
`Random()` — random `Int` via `wasi/random/random`; `File`/`read` —
`canon:builtins/filesystem`; `Now()` — RFC 3339 wall clock;
`Url` + `get` — `canon:builtins/url` + `canon:builtins/http`.)

Each `use canon/std/X` brings in the named type along with its constructor and
methods. Behind the scenes those modules are written in ordinary Canon
on top of the generated `wasi/…` bindings; there is no privileged path.

See the [Standard Library](../reference/stdlib.md) reference for the
complete list and the WIT interfaces behind each module.

## `wasi:*` vs `canon:builtins/*`

You'll see two namespaces in `extern Wasm` paths:

- `wasi:*` — standard [WASI](https://github.com/WebAssembly/WASI)
  interfaces. Any compliant host satisfies them.
- `canon:builtins/*` — temporary host bridges that `canon run`
  implements internally. Each one will move to the corresponding `wasi:*`
  interface as that interface's canonical-ABI shape (async, streams,
  resources) becomes available.

From a user's perspective both look identical: `use canon/std/Foo` and call
methods. The bridge swap is invisible.

## Tradeoffs

- **No direct OS handles.** A Canon program cannot embed a raw
  `std::fs::File` or a `tokio::net::TcpStream`; it sees the corresponding
  `wasi:*` resource handle instead. This is the price of portability.
- **Some interfaces are still bridged.** Where a `wasi:*` interface
  isn't yet usable from the canonical ABI (resources + streams, e.g.
  filesystem descriptors), Canon ships a `canon:builtins/*` stand-in.
  The user-facing API doesn't change when the bridge is later swapped
  for native WASI — the remaining set is tracked in `V1.md`.
- **Hosts must support WASI Preview 3.** `canon run` embeds `wasmtime`
  with the P3 + component-model-async feature gates; other hosts will
  need equivalent support.
