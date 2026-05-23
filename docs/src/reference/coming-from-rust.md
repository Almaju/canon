# Coming from Rust

Oneway's compiler is written in Rust and the language inherits Rust-style
ownership analysis, but it does **not** transpile to Rust. Programs
compile to WebAssembly Components (WASI Preview 3). If you already know
Rust, this page is the fastest path in — the cheat sheet is the
syntax-level mapping, not a runtime mapping.

## Cheat Sheet

| Rust                                       | Oneway                                  |
|--------------------------------------------|-----------------------------------------|
| `struct User { birthday: ..., username: ... }` | `User = Birthday * Username`        |
| `enum Bool { False, True }`                | `Bool = False + True`                  |
| `type Name = String;` (or `struct Name(String);`) | `Name = String`                  |
| `impl User { fn greet(&self) -> String { ... } }` | `greet = (User) -> String { ... }` |
| `fn main() { ... }`                        | `main = () -> Unit { ... }`             |
| `trait Show { fn show(&self) -> String; }` | `Show = () -> String`                   |
| `impl Show for User { ... }`               | `Show = (User) -> String { ... }`      |
| `Result<T, E>`                             | `Result<T, E>` (same name; inline union for `E`) |
| `Option<T>`                                | `Option<T>`                             |
| `?` operator                               | `?` operator (same semantics)           |
| `match x { ... }`                          | `x.( ... )`                             |
| `let x = ...;`                             | No equivalent — declare a newtype       |
| `if cond { a } else { b }`                 | `cond.( False => b, True => a )`        |
| `pub fn`                                   | Everything is public                    |
| `mod foo;`                                 | No `mod` — `foo.ow` declares `Foo`      |
| `use crate::foo::Foo;`                     | `use Foo`                               |
| `use serde_json::Value;` (third-party)     | `use std/Foo` for stdlib, plus `extern Wasm` for raw imports |
| `fn(...) -> T` (function type)             | `(params) -> T` (also a trait declaration) |
| `&T` / `&mut T` / `Box<T>` / `Rc<T>`       | Inferred by the compiler                |
| `async fn`, `.await`                       | Inferred — no source-level keyword      |

## Things Rust Has That Oneway Doesn't

- **Lifetimes and borrow sigils** (`'a`, `&`, `&mut`). Ownership is
  inferred from usage.
- **Comments.** Use names and types.
- **`if`/`else`.** Use dispatch on `Bool`.
- **`let` and local variables.** Method chaining only; newtype an
  intermediate value if you really need to name it.
- **Named arguments.** Use newtypes for disambiguation.
- **Macros and `format!`.** No comparable mechanism yet.
- **`async`/`await`.** Suspension is inferred from `extern Wasm.async`
  declarations and `Future<T>`/`Stream<T>` consumption — both keywords
  are absent at the source level.

## Things Oneway Has That Rust Doesn't

- **Mandatory alphabetical declaration order.** Compiler-enforced.
- **Domain-first I/O.** Effects flow through ordinary values (`File`,
  `Url`, `Database`) constructed from concrete inputs — no `unsafe`, no
  globals, no `lazy_static`, no service locators.
- **Inline error unions.** `Result<Bytes, IoError + NotFound>` without
  declaring a wrapper enum at every call site.
- **No-comments policy.** The compiler rejects them.
- **Portable build artifact.** `oneway build` emits a `.wasm` Component;
  no native linker, no per-platform binaries.

## Build Artifacts

| Command                         | Output                                                      |
|---------------------------------|-------------------------------------------------------------|
| `oneway run hello.ow`           | Runs through the embedded `wasmtime`, prints to stdout       |
| `oneway build hello.ow`         | `.oneway/hello/hello.wasm` + sibling `.wit` world           |
| `oneway emit hello.ow`          | WAT (WebAssembly Text) for the **core** module               |
| `oneway check hello.ow`         | Type + sort-order check, no codegen                          |

There is no `Cargo.toml`, no manifest, no `rustc`/`cargo` invocation
anywhere in the build path.

## When in Doubt

- Look at the [`examples/`](https://github.com/Almaju/oneway/tree/main/examples)
  directory in the repo.
- Read [`DESIGN.md`](https://github.com/Almaju/oneway/blob/main/DESIGN.md)
  — it's the authoritative spec.
- Read [`WASM.md`](https://github.com/Almaju/oneway/blob/main/WASM.md)
  for the WASM-backend status and known gaps.
- `oneway emit path/to/file.ow` prints the WAT the compiler produces.
