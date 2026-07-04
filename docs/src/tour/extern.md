# Binding Files

Canon compiles to a **WebAssembly Component**, so foreign functions bind
to **Component Model imports** declared by their fully-qualified path,
not to a host language. There is no FFI keyword: a **binding file** is
recognized by *shape and path*. It is an ordinary `.can` file sitting
directly in a vendored package directory —
`<ns>/<name>@<version>/<iface>.can` — whose function declarations are
body-less:

```
deps/wasi/random@0.3.0-rc-2026-03-15/random.can
```

```canon
getRandomU64 = () -> Int
```

The path spells the interface; the declaration's kebab-case form names
the WIT function. Together they reconstruct the Component Model path:

```
namespace : package / interface @ version # function
wasi      : random  / random    @ 0.3.0-… # get-random-u64
```

The compiler emits the matching `(import …)` and lowers each call
through the canonical ABI: no source-level marshalling, no manual
buffer management, no directive — the file's location carries
everything a header ever said.

- The `namespace:package/interface` part identifies a WIT interface: a
  group of related functions, types, and resources.
- The `@version` is optional and only present on versioned interfaces
  (every WASI 0.3-rc interface, for example).
- The `#function` selects one function inside the interface.

The Canon signature declares how the function is called from Canon. The
compiler resolves Canon types against the WIT-level types: `Int` maps
to the WIT integer at its declared width (`u8` through `s64`; for
`wasi:*` imports the vendored WIT is consulted, so narrow widths are
honoured at the ABI), `String` is `string`, `Result<T, E>` is
`result<T, E>`, and so on.

Generated and hand-vendored binding files share one shape: bare
function-type aliases in a versioned package directory. There is no
explicit form — the path is the declaration; see
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

Url = (String) -> Result<Url, InvalidUrl> {
    String.parse()
}
```

The type declaration on the first line is an ordinary Canon newtype.
The constructor on the second line is an ordinary Canon function whose
body calls `parse` — a raw binding declared in the
`canon:builtins/url@0.1.0` binding file. Idioms are always plain
wrappers over raw bindings; the binding layer itself never renames
anything (kebab-case ↔ camelCase is a round trip).

## Async Externs

Async-ness is not declared — it is read off the signature. A binding
whose return type is `Future<T>` is a suspending import:

```canon
serve = (HttpServer) -> Future<Result<Unit, IoError>>
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

## Installing Bindings

`canon install <ns>:<pkg>[@ver]` fetches a WIT package from its
registry and vendors the generated binding files under
`deps/<ns>/<pkg>@<version>/` (see PACKAGES.md). The compiled
component's import list is then fully determined by the binding
declarations the program actually uses, and resolved at
component-instantiation time by the host. `canon build` produces a
`.wasm` plus a sibling `.wit` describing the component's world.

## Generating Bindings from WIT

Writing binding declarations by hand is rarely necessary. The
compiler can read a WIT file (or a `.wasm` component, whose embedded
WIT is extracted automatically) and emit one Canon binding file per
interface:

```sh
canon bindgen path/to/my-component.wit -o .
```

This writes `<ns>/<pkg>@<ver>/<iface>.can` for each interface,
alphabetically ordered, ready to `use`. The mapping is mechanical: WIT
records become
products, variants become unions, `list<T>` becomes `List<T>`, kebab-case
becomes Canon camelCase / PascalCase, and so on. See `DESIGN.md` for the
full table.

The `wasi/…` bindings shipped with the compiler are produced this way
from the WIT files vendored under `wit-vendor/wasi/`. Regenerate them
with `just regen-bindings`.

## Binding Packages

Idiomatic Canon code does not touch binding files directly. Instead, it
imports individual types from the embedded standard library:

```canon
use canon/std/Random
use canon/std/fs/File
use canon/std/http/Url
use canon/std/time/Instant
use canon/std/time/Now
```

(`Instant()`: monotonic clock via `wasi/clocks/monotonic_clock`.
`Random()`: random `Int` via `wasi/random/random`. `File`/`read`:
`canon:builtins/filesystem`. `Now()`: RFC 3339 wall clock.
`Url` + `get`: `canon:builtins/url` + `canon:builtins/http`.)

Each `use canon/std/X` brings in the named type along with its constructor and
methods. Behind the scenes those modules are written in ordinary Canon
on top of the generated `wasi/…` bindings; there is no privileged path.

See the [Standard Library](../reference/stdlib.md) reference for the
complete list and the WIT interfaces behind each module.

## `wasi:*` vs `canon:builtins/*`

Two namespaces appear in binding-file paths:

- `wasi:*`: standard [WASI](https://github.com/WebAssembly/WASI)
  interfaces. Any compliant host satisfies them.
- `canon:builtins/*`: temporary host bridges that `canon run`
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
  for native WASI. The remaining set is tracked in `V1.md`.
- **Hosts must support WASI Preview 3.** `canon run` embeds `wasmtime`
  with the P3 + component-model-async feature gates; other hosts will
  need equivalent support.
