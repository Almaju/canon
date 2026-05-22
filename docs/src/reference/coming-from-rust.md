# Coming from Rust

Oneway transpiles to Rust and inherits its execution model, but the
surface syntax is quite different. If you already know Rust, this page is
the fastest path in.

## Cheat Sheet

| Rust                                       | Oneway                                  |
|--------------------------------------------|-----------------------------------------|
| `struct User { birthday: ..., username: ... }` | `User = Birthday * Username`        |
| `enum Bool { False, True }`                | `Bool = False + True`                  |
| `type Name = String;` (newtype via `struct Name(String);`) | `Name = String`         |
| `impl User { fn greet(&self) -> String { ... } }` | `greet = (User) -> String { ... }` |
| `fn main() { ... }`                        | `main = (Stdout) -> Unit { ... }`       |
| `trait Show { fn show(&self) -> String; }` | `Show = () -> String`                   |
| `impl Show for User { ... }`               | `Show = (User) -> String { ... }`      |
| `Result<T, E>`                             | `Result<T, E>` (same name; inline union for `E`) |
| `Option<T>`                                | `Option<T>`                             |
| `?` operator                               | `?` operator (same semantics)           |
| `match x { ... }`                          | `x.( ... )`                             |
| `let x = ...;`                             | No equivalent — declare a newtype       |
| `if cond { a } else { b }`                 | `cond.( False => b, True => a )`        |
| `pub fn`                                   | Everything is public   |
| `mod foo;`                                 | No `mod` — `foo.ow` declares `Foo`      |
| `use crate::foo::Foo;`                     | `use Foo`                               |
| `fn(...) -> T` (function type)             | `(params) -> T` (also a trait declaration) |
| `&T` / `&mut T` / `Box<T>` / `Rc<T>`       | Inferred by the transpiler              |

## Things Rust Has That Oneway Doesn't

- **Lifetimes and borrow sigils** (`'a`, `&`, `&mut`). Ownership is
  inferred from usage.
- **Comments.** Use names and types.
- **`if`/`else`.** Use dispatch on `Bool`.
- **`let` and local variables.** Method chaining only; newtype an
  intermediate value if you really need to name it.
- **Named arguments.** Use newtypes for disambiguation.
- **Macros and `format!`.** No comparable mechanism yet.
- **`async`/`await`.** Concurrency is uniform — see DESIGN.md.

## Things Oneway Has That Rust Doesn't

- **Mandatory alphabetical declaration order.** Compiler-enforced.
- **Effects as capabilities.** Side effects flow through ordinary
  arguments rather than `unsafe`, globals, or library wrappers.
- **Inline error unions.** `Result<Bytes, IoError + NotFound>` without
  declaring a wrapper enum at every call site.
- **No-comments policy.** The compiler rejects them.

## When in Doubt

- Look at the [`examples/`](https://github.com/Almaju/oneway/tree/main/examples)
  directory in the repo.
- Read [`DESIGN.md`](https://github.com/Almaju/oneway/blob/main/DESIGN.md)
  — it's the authoritative spec.
- `just emit path/to/file.ow` prints the Rust the transpiler produces.
