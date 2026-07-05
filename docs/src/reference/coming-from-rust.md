# Coming from Rust

Canon's compiler is written in Rust and the language inherits
Rust-style ownership analysis, but it does **not** transpile to Rust.
Programs compile to WebAssembly Components (WASI Preview 3). The cheat
sheet below is a syntax-level mapping, not a runtime mapping.

## Cheat Sheet

| Rust                                       | Canon                                  |
|--------------------------------------------|-----------------------------------------|
| `struct User { birthday: ..., username: ... }` | `User = Birthday * Username`        |
| `enum Bool { False, True }`                | `Bool = False + True`                  |
| `type Name = String;` (or `struct Name(String);`) | `Name = String`                  |
| `impl User { fn greet(&self) => String { ... } }` | `greet = (User) => String { ... }` |
| `fn main() { ... }`                        | `main = () => Unit { ... }`             |
| `trait Show { fn show(&self) -> String; }` | `Show = () => String`                   |
| `impl Show for User { ... }`               | `Show = (User) => String { ... }`      |
| `Result<T, E>`                             | `Result<T, E>` (same name; inline union for `E`) |
| `Option<T>`                                | `Option<T>`                             |
| `?` operator                               | `?` operator (same semantics)           |
| `match x { ... }`                          | `x.( ... )`                             |
| `let x = ...;`                             | No equivalent; declare a newtype        |
| `if cond { a } else { b }`                 | `cond.( * (False) => R { b } * (True) => R { a } )` |
| `pub fn`                                   | Everything is public                    |
| `mod foo;`                                 | No `mod`; `foo.can` declares `Foo`       |
| `use crate::foo::Foo;`                     | Nothing — referencing `Foo` loads `foo.can` |
| `use serde_json::Value;` (third-party)     | Nothing — stdlib and `deps/` names resolve by reference; `extern Wasm` for raw imports |
| `fn(...) -> T` (function type)             | `(params) -> T` (also a trait declaration) |
| `&T` / `&mut T` / `Box<T>` / `Rc<T>`       | Inferred by the compiler                |
| `async fn`, `.await`                       | Inferred; no source-level keyword       |
| `String::from(x)` / `x.into()` / `x.to_string()` | `String(x)` / `x.String()` — conversion is construction |
| `s.parse::<i64>()?`                        | `Int(s)?` / `s.Int()?` (stdlib, loads automatically) |
| `HashMap::new()` + `.insert(k, v)`         | `Map().Inserted(k, v)` (stdlib; sorted, functional) |
| `BTreeSet::new()` + `.insert(x)`           | `Set().Inserted(x)` (stdlib) |

## Things Rust Has That Canon Doesn't

- **Lifetimes and borrow sigils** (`'a`, `&`, `&mut`). Ownership is
  inferred from usage.
- **Comments.** Use names and types.
- **`if`/`else`.** Use dispatch on `Bool`.
- **`let` and local variables.** Method chaining only; newtype an
  intermediate value if you need to name it.
- **Named arguments.** Use newtypes for disambiguation.
- **Macros and `format!`.** No comparable mechanism yet.
- **`async`/`await`.** Suspension is inferred from `extern Wasm.async`
  declarations and `Future<T>`/`Stream<T>` consumption; both keywords
  are absent at the source level.

## Things Canon Has That Rust Doesn't

- **Mandatory alphabetical declaration order.** Compiler-enforced.
- **Domain-first I/O.** Effects flow through ordinary values (`File`,
  `Url`, `Database`) constructed from concrete inputs. No `unsafe`, no
  globals, no `lazy_static`, no service locators.
- **Inline error unions.** `Result<Bytes, IoError + NotFound>` without
  declaring a wrapper enum at every call site.
- **No-comments policy.** The compiler rejects them.
- **Portable build artifact.** `canon build` emits a `.wasm` Component;
  no native linker, no per-platform binaries.

## Build Artifacts

| Command                         | Output                                                      |
|---------------------------------|-------------------------------------------------------------|
| `canon run`                    | Builds the current package and runs it through `wasmtime`    |
| `canon run hello.can`           | Single-file mode: same, but for one loose `.can` file         |
| `canon run my-ws -p foo`       | Runs workspace member `foo` (`cargo run -p foo`)             |
| `canon build`                  | `build/<name>.wasm` + sibling `.wit` for the current package |
| `canon build my-ws`            | Builds every member of a workspace into its shared `build/`  |
| `canon build hello.can`         | `build/hello/hello.wasm` + sibling `.wit` (single-file mode) |
| `canon inspect wat hello.can`   | WAT (WebAssembly Text) for the **core** module               |
| `canon check`                  | Type + sort-order check on the current package, no codegen   |

A package is `canon.toml` + `src/main.can` + `build/`: the same
three-sibling shape as `Cargo.toml` + `src/` + `target/`. A workspace
is `canon.toml` (`[workspace] members = ["*"]`) + member packages + a
shared `build/` at the workspace root, the same shape as Cargo
workspaces. There is no `rustc`/`cargo` invocation anywhere in the
build path; the output `.wasm` is a portable WebAssembly Component.

## When in Doubt

- Look at the [`examples/`](https://github.com/Almaju/canon/tree/main/examples)
  directory in the repo.
- Read the [language specification](../spec/index.md), the authoritative
  reference, and [Compilation and the ABI](../spec/compilation.md) for
  how programs lower to WebAssembly.
- `canon inspect wat path/to/file.can` prints the WAT the compiler produces.
