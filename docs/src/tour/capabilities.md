# Capabilities

A function's type should not lie about what it does. `print = (Stdout * String) -> Unit` makes it clear that printing requires a capability and a value.

Oneway models effects as **capabilities** — values that must be passed in
to perform an effect.

## The Pattern

A function that prints requires `Stdout`:

```oneway
print = (Stdout * String) -> Unit {
    Stdout.write(String)
}
```

A function that reads files requires `Filesystem`. A function that uses
the clock requires `Clock`. And so on. The capability is just a type,
passed as an ordinary argument.

## Where Capabilities Come From

The only place to obtain real-world capabilities is `main.ow`, which
receives them as parameters and threads them down to anything that needs
them:

```oneway
main = (Stdout) -> Unit {
    "hello".print(Stdout)
}
```

If a function does not receive a capability, it cannot perform the
corresponding effect — it cannot even *call* something that does. Effects
propagate through the type system: if `f` calls something needing
`Stdout`, then `f` must take `Stdout` too.

## Multiple Capabilities

A function that needs several capabilities receives them as a single
product-typed parameter — the same `*` that composes product types
elsewhere in the language:

```oneway
use Filesystem

main = (Filesystem * Stdout) -> Result<Unit, IoError> {
    Filesystem.read(Path("Cargo.toml"))?.print(Stdout)
    Ok(Unit)
}
```

The components are accessed by their type names. The alphabetical-order
rule that applies to product members also applies here:
`(Filesystem * Stdout)` is valid; `(Stdout * Filesystem)` is a compile
error.

## Built-In Capabilities

The Oneway-owned core includes:

| Capability     | Effect                       | Kind            |
|----------------|------------------------------|-----------------|
| `Clock`        | Read the current time        | non-suspending  |
| `Filesystem`   | Read and write files         | suspending      |
| `Network`      | Open network connections     | suspending      |
| `Random`       | Generate random values       | non-suspending  |
| `Stderr`       | Write to standard error      | non-suspending  |
| `Stdin`        | Read from standard input     | non-suspending  |
| `Stdout`       | Write to standard output     | non-suspending  |

Binding packages add more capabilities of their own — `HttpClient`,
`HttpServer` (suspending), `Json` (non-suspending), etc.

## Suspending vs Non-Suspending

Capabilities split into two kinds based on whether their effects can wait
on the outside world:

- **Non-suspending** capabilities (`Stdout`, `Clock`, `Random`, …) complete
  without yielding to a scheduler.
- **Suspending** capabilities (`Filesystem`, `Network`, …) may park the
  caller while the OS or a remote system responds.

A function compiles to `async fn` in Rust **if and only if** it
transitively requires a suspending capability or calls an
[`extern Rust.async`](extern.md#async-externs) item. Otherwise it compiles
to a plain `fn`. `main` becomes `#[tokio::main]` only when the program is
async-colored.

This is invisible at the source level — Oneway has no `async` keyword and
no `.await` — but it carries the "color" of a function through the type
system. The capability parameter *is* the color. Pure-compute programs
that take no suspending capability link no async runtime, pay no
state-machine overhead, and produce small binaries.

The propagation rule is the same as for any other capability: if you call
something needing `Filesystem`, your function must declare `Filesystem`
in its parameters. The compiler verifies; it does not infer the signature
for you.

## Why Not Monads?

A capability-passing model gives you the same honest type signatures as a
monadic effect system, without introducing a separate kind of value or
forcing all effectful code into a `do`-style block. Effects are just
arguments. Composition is just method calls.
