# Hello, World

Create a file named `hello.can`:

```canon
main = () -> Unit {
    "hello".print()
}
```

Run it:

```sh
canon run hello.can
```

You should see:

```
hello
```

That's the whole program. Let's walk through it.

## Line by Line

```canon
main = () -> Unit {
```

`main` is the program's entry point. Like every other binding in Canon
it has the shape `name = (parameters) -> ReturnType { body }`. The empty
`()` says it takes nothing; the compiler lifts `main` as the component's
`wasi:cli/run.run` export.

`Unit` is a singleton type — a type with exactly one value, named after
itself. Returning `Unit` is the language's way of saying "this function
produces nothing useful".

```canon
    "hello".print()
}
```

`"hello"` is sugar for `String("hello")`. The body of a function is a
sequence of expressions separated by newlines; the last one is the return
value.

`"hello".print()` is a method call. `print` takes a single `String`
component and writes it to stdout:

```canon
print = (String) -> Unit
```

There is no `Stdout` capability to thread through. The compiler lowers
`.print` against the standard `wasi:cli/stdout` interface, so the
resulting `.wasm` runs on any Component Model host.

For *redirectable* output — writing to a file, a log sink, a test
buffer — you'd construct an explicit destination value (a `File`, a
`Fileout`) and pass it as an additional component. Plain `.print()` is
sugar for "I just want stdout".

## Try Breaking Things

Some small experiments to build intuition:

- **Add a second `.print()` line.** Each call writes its argument followed
  by a newline.
- **Add a comment** (`// hi`). The lexer rejects this — comments are not
  allowed.
- **Return something other than `Unit`.** The body's last expression must
  match the declared return type.
- **Inspect the compiled component.** `canon build hello.can` writes
  `build/hello/hello.wasm` and a sibling `.wit` describing the
  component's world.
