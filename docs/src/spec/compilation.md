# Compilation and the ABI

This chapter describes what the compiler does with a checked program and
what the resulting artifact is. The pipeline is **source → lexer →
parser → checker → codegen**. Codegen emits a WebAssembly core module
and wraps it into a **Component Model component** in-process:
`wasm-encoder` and `wit-component` produce the final `.wasm`, no
external toolchain is invoked, and `canon run` executes the result on an
embedded wasmtime.

## Artifacts

```sh
$ canon build my-app
Compiled to: my-app/build/my-app.wasm
WIT world : my-app/build/my-app.wit
```

- **`.wasm`**: a WASI Preview 3 component. Its imports are standard
  `wasi:*` interfaces, plus, transitionally, `canon:builtins/*` host
  bridges for surfaces whose canonical-ABI shape isn't ready; these run
  only under `canon run`.
- **`.wit`**: the component's world, a human- and tool-readable
  statement of everything it imports and exports.

The WASI rc version is embedded in every interface name, so components
either match their host exactly or fail loudly at instantiation.

## Worlds

The [entry-point rule](./functions.md#the-entry-point) selects the
world:

| Entry signature | World | Export |
|---|---|---|
| `Unit => Program` | `wasi:cli/command` | `wasi:cli/run.run` |
| `Request => Response` | `wasi:http/service` | `wasi:http/handler.handle` |
| `Model => Html` triple (+ `Unit => Init`, `Model * Msg => Update`) | browser ([web target](../reference/web-target.md)) | core module + JS host |

The CLI and HTTP entries are lifted async-stackful, letting nested
suspending calls yield through the canonical ABI. The web target emits a
plain core module, not a component (browsers instantiate core wasm
directly).

## Memory Model

Canon has **no garbage collector** and no user-visible ownership
syntax. The compiler performs Rust-style ownership analysis and lowers
every value to a concrete linear-memory layout:

| Source concept | Lowering |
|---|---|
| Function parameter | moved or borrowed value in wasm locals |
| Recursive type | heap cell in the bump heap (auto-boxed) |
| Shared ownership the analysis can't otherwise prove | reference-counted cell |

There are no lifetimes, no `&`/`&mut`, no `Box`/`Rc` in source. If no
valid ownership scheme exists for a program, that is a compile error,
reported in Canon terms.

## Binding Files

All interop happens at the Component Model boundary, declared in
**binding files**: `.can` files whose declarations are body-less,
sitting directly in a versioned package directory
(`wasi/cli@0.3.0-rc-2026-03-15/environment.can`) whose path spells the
WIT interface — no directive, no header:

```canon
getArguments = () => List<String>

getInitialCwd = () => Option<String>
```

The loader rewrites each alias into an external function bound to
`<urn>#<kebab-case-name>`, deriving the URN from the file's vendored
path:

Body-less camelCase declarations are only meaningful inside a binding
file — anywhere else they remain plain function-type aliases. Bound
functions are first-class values like any other function.

Bindings are produced mechanically:

- `canon bindgen <wit-or-wasm>` emits one binding file per WIT
  interface (deterministic, alphabetical, `canon fmt`-clean).
- `canon install` reads the manifest's `[imports]` table and
  materializes bindings into `bindgen/` in the same versioned layout.
  Functions whose shape the codegen can't lower yet are **skipped with
  a printed reason**, never emitted broken.

## The WIT ↔ Canon Mapping

| WIT | Canon |
|---|---|
| `bool` | `Bool` |
| `u8` … `s64` | `Int` (declared WIT width honoured at the ABI) |
| `f32`, `f64` | `Float` |
| `char`, `string` | `String` |
| `list<T>` | `List<T>` |
| `option<T>` | `Option<T>` |
| `result<T, E>` | `Result<T, E>` |
| `result<T>` / bare `result` | `Result<T, Unit>` / `Result<Unit, Unit>` |
| `tuple<A, B>` | product with positional field names `_0`, `_1`, … |
| `record { … }` | product (fields alphabetical in source, WIT order at the ABI) |
| `variant` / `enum` | union |
| `flags` | product of `Bool` |
| `resource` | newtype over `Handle` (opaque) |
| `func` | body-less function |
| `async func` | body-less function returning `Future<T>` |
| `stream<T>` / `future<T>` | `Stream<T>` / `Future<T>` |

Identifier case converts mechanically: WIT `kebab-case` becomes
`camelCase` for functions and `PascalCase` for types
(`get-resolution` → `getResolution`, `incoming-request` →
`IncomingRequest`).

**Resources.** A WIT `resource` is modelled as `Foo = Handle`, where
`Handle` is a non-copyable, non-printable language primitive with
linear ownership: it can only be passed to binding functions and
dropped by going out of scope (the compiler emits the matching
`resource.drop`). WIT's `own<T>` / `borrow<T>` distinction is read off
the WIT and handled by the compiler; source never mentions it.

The resource's members become ordinary body-less functions: a
`[method]foo.bar` maps to a function whose first parameter is `Foo`
(the prefix is implicit), a `[static]foo.bar` to one with no `Handle`
parameter, and a `[constructor]foo` to one returning `Foo`.

## The Stdlib Layering

The shipped standard library demonstrates the intended architecture and
has no privileged mechanism:

- **generated bindings** (under the stdlib's `bindgen/`): raw,
  machine-produced from vendored WASI WIT, regenerated and never
  hand-edited;
- **curated wrappers** (`canon/std/…`): hand-written Canon presenting
  one primary type per file with capability discipline.

Idiomatic code imports only `canon/std/…`. A direct import of a raw
binding works (everything is public) but gives up the curated naming
and discipline. Where a `wasi:*` interface isn't yet expressible through
the canonical ABI, the binding temporarily points at a
`canon:builtins/*` bridge fulfilled by `canon run`; the wrapper API
doesn't change when the bridge is later swapped for the real interface.

## Inspecting the Pipeline

```sh
canon inspect tokens hello.can   # lexer output
canon inspect ast hello.can      # parser output
canon inspect wat hello.can      # generated core-module WebAssembly Text
```

`inspect wat` is the fastest way to see how a Canon construct lowers
(dispatch, heap allocation, string handling, async plumbing) without
the component wrapper in the way.
