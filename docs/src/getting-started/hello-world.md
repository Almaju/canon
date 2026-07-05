# Hello, World

Create a file named `hello.can`:

```canon,run=hello-world
Unit => Program {
    "hello" -> Print
}
```

Run it:

```sh
canon run hello.can
```

Output:

```
hello
```

## Line by Line

```canon
Unit => Program {
```

This is the entry point, selected by returning `Program`, the CLI world
type — no name needed, just as an HTTP handler is selected by returning
`Response`. Like every arrow in Canon it has the shape
`Input => ReturnType { body }`. The `Unit` on the left says it
takes nothing (`Unit` is the name of "no input"); returning `Program`
(`= Unit`, from `canon/std`) is what the compiler lifts as the
component's `wasi:cli/run.run` export.

`Program` is the CLI world type; because `Program = Unit`, the body can
end in `Unit` — one value, named after itself — and still satisfy the
return.

```canon
    "hello".print()
}
```

`"hello"` is sugar for `String("hello")`. A function body is a sequence
of expressions separated by newlines; the last one is the return value.

`"hello".print()` is a method call. `print` takes a single `String`
component and writes it to stdout:

```canon
print = (String) => Unit
```

There is no `Stdout` capability to thread through. The compiler lowers
`.print` against the standard `wasi:cli/stdout` interface, so the
resulting `.wasm` runs on any Component Model host.

For redirectable output (a file, a log sink, a test buffer), construct
an explicit destination value such as a `File` or a `Fileout` and pass
it as an additional component. Plain `.print()` is sugar for "I want
stdout".

## Try Breaking Things

- **Add a second `.print()` line.** Each call writes its argument followed
  by a newline.
- **Add a comment** (`// hi`). The lexer rejects it; comments are not
  allowed.
- **Return something other than `Unit`.** The body's last expression must
  match the declared return type.
- **Inspect the compiled component.** `canon build hello.can` writes
  `build/hello/hello.wasm` and a sibling `.wit` describing the
  component's world.
