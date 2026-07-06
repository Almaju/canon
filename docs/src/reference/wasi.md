# Using WASI Interfaces

Every Canon program is a WebAssembly component, so talking to the
outside world means importing **WIT interfaces**. Three layers are
involved; you normally only touch the top one.

## The layers

1. **The `canon/std` wrappers**: curated wrappers with Canon idioms
   (`Random()`, `Args()`, `1 -> Exited`). This is the layer
   programs reach -- referencing a name like `Random` resolves to it
   automatically. See the [Standard Library](./stdlib.md).
2. **The `wasi/...` bindings**: machine-generated, one file per WIT
   interface, produced by `canon install` from vendored WIT. The
   stdlib wrappers consume these; user packages can too.
3. **The WIT itself**: the contract any component-model host
   understands.

There is no privileged mechanism for the stdlib: it declares its WIT
sources in its `canon.toml` and runs the same `canon install` any
package would.

## Declaring imports in `canon.toml`

The `[imports]` table maps a path prefix to a WIT source: a `.wit`
file, a directory of them, or a `.wasm` component to extract types
from.

```toml
name    = "my-app"
version = "0.1.0"

[imports]
"wasi" = "./wit/wasi"
```

Then materialize the bindings:

```sh
canon install          # writes bindgen/wasi/<pkg>@<version>/<interface>.can
```

Each generated file is pure source -- plain function-type aliases, no
header. The versioned directory name carries the interface's package
and version, and the loader derives each declaration's binding from
the path (a binding file is recognized by shape):

```canon
getArguments = () => List<String>

getInitialCwd = () => Option<String>
```

Your code imports and calls them like any Canon function. `canon bindgen <wit-or-wasm> -o <dir>` does the same
one-shot, without a manifest.

## Type mapping

| WIT | Canon |
|---|---|
| `bool` | `Bool` |
| integers (`u8`...`s64`) | `Int` (the compiler honours the exact WIT width at the ABI) |
| `f32`/`f64` | `Float` |
| `string` | `String` |
| `list<T>` | `List<T>` |
| `option<T>` | `Option<T>` |
| `result<O, E>` | `Result<O, E>` |
| `record` | a product of `Int`/`String`-newtype fields |
| `enum` / `variant` | a union |
| `resource` | a `Handle` newtype (opaque) |

Functions whose shape the compiler can't lower yet are **skipped with
a reason** at install time rather than emitted broken, notably
resource *methods* and streams. The skip list is printed by
`canon install`.

## What the compiled component imports

`canon build` writes a `.wit` file next to the `.wasm` describing the
world your component implements. Imports of `wasi:*` interfaces are
fulfilled by any WASI 0.3 host; `canon run` fulfils them with the
embedded wasmtime.
