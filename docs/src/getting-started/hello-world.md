# Hello, World

Create a file named `hello.can`:

```canon,run=hello-world
Args => Exit {
    "hello" -> Print
    0 -> Exit
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
Args => Exit {
```

This is the entry point, selected by its signature -- no name needed,
just as an HTTP handler is selected by `Request => Response`. Like every
arrow in Canon it has the shape `Input => ReturnType { body }`, and the
CLI entry mirrors the HTTP one: the command's **argument vector flows in
as `Args`, an exit status flows out as `Exit`**.

`Args` (`= List<String>`, from `canon/std`) is the program's `argv` -- the
compiler binds it from `wasi:cli/environment`, so you never fetch it, it
is handed to you. `Exit` (`= Int`) is the exit status; the whole arrow is
what the compiler lifts as the component's `wasi:cli/run.run` export.
This program ignores its arguments (a later chapter reads them), but the
shape is always there -- the same way an HTTP handler names `Request` even
when it ignores it.

```canon
    "hello" -> Print
    0 -> Exit
}
```

`"hello"` is sugar for `String("hello")`. A function body is a sequence
of expressions separated by newlines; the last one is the return value --
here `Exit(0)`, a successful exit. `Exit(0)` is success (process exit 0);
any nonzero `Exit` reports failure. (Nothing to report and no arguments
to read? The arg-less shorthand `Unit => Program { ... }` still works and
needs no explicit exit.)

`"hello" -> Print` is a pipe call. `Print` takes a single `String`
component and writes it to stdout:

```canon
(String) => Unit
```

There is no `Stdout` capability to thread through. The compiler lowers
`Print` against the standard `wasi:cli/stdout` interface, so the
resulting `.wasm` runs on any Component Model host.

For redirectable output (a file, a log sink, a test buffer), construct
an explicit destination value such as a `File` or a `Fileout` and pass
it as an additional component. Plain `-> Print` is sugar for "I want
stdout".

## Try Breaking Things

- **Add a second `-> Print` line.** Each call writes its argument followed
  by a newline.
- **Add a comment** (`// hi`). The lexer rejects it; comments are not
  allowed.
- **Drop the `0 -> Exit` line.** The body's last expression must match the
  declared return type (`Exit`), so ending on a `Print` (which yields
  `Unit`) is a checker error.
- **Inspect the compiled component.** `canon build hello.can` writes
  `build/hello/hello.wasm` and a sibling `.wit` describing the
  component's world.
