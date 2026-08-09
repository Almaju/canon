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

There is no privileged mechanism for the stdlib: it vendors its WIT
sources under its `wit/` directory and runs the same `canon install`
any package would.

## Declaring imports with `wit/`

There is no manifest — putting a WIT source under the project's `wit/`
directory *is* the import declaration. Every immediate entry of `wit/`
is one source: a `.wit` file or a directory of them. (A `.wasm`
component is reported as skipped until the composition pipeline lands.)

```text
my-app/
  src/main.can
  wit/
    wasi/          # a directory of .wit files
      random.wit
```

Then materialize the bindings:

```sh
canon install          # writes bindgen/wasi/<pkg>@<version>/<interface>.can
```

(`canon build`, `run`, `check`, and `test` do this automatically when
the bindings are missing or stale.)

Each generated file is pure source, no header. The versioned directory
name carries the interface's package and version, and the loader
derives each declaration's binding from the path (a binding file is
recognized by shape). Every WIT function mints a result newtype named
after it and declares an anonymous constructor whose string body is
the WIT function name verbatim:

```canon
GetArguments = List<String>

Unit => GetArguments {
    "get-arguments"
}
```

Your code calls the constructor like any other: `GetArguments()`, or
piped. `canon bindgen <wit-or-wasm> -o <dir>` does the same one-shot,
outside any project.

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
| `record` | a product of per-field newtypes |
| `enum` / `variant` | a union |
| `resource` | a `Handle` newtype (opaque) |

Functions whose shape the compiler can't lower yet are **skipped with
a reason** at install time rather than emitted broken, notably
resource *methods* and streams. The skip list is printed by
`canon install`.

## What the compiled component imports

`canon build` writes a `.wit` file next to the `.wasm` describing the
world your component implements. Imports of the standard WASI 0.3
interfaces are fulfilled by any WASI 0.3 host; `canon run` fulfils
them with the embedded wasmtime. Anything else the component imports
(an installed package like `wasi:config`) is the composing host's job.
