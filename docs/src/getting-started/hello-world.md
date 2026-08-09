# Hello, World

Create a file named `hello.can`:

```canon,run
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

This is the entry point, selected by its signature -- no name needed,
just as an HTTP handler is selected by `Request => Response`. Like every
arrow in Canon it has the shape `Input => ReturnType { body }`. `Unit`
is the single-value type, the name of "no input"; `Program` (`= Unit`,
from `canon/std`) is the CLI world -- the whole arrow is what the
compiler lifts as the component's `wasi:cli/run.run` export. (More on
entry points in [Programs & Modules](../learn/programs-and-modules.md).)

```text
    "hello" -> Print
}
```

`"hello"` is sugar for `String("hello")`. A function body is a sequence
of expressions separated by newlines; the last one is the return value.
A `Program` body needs no explicit exit -- reaching the end is success.

`"hello" -> Print` is a pipe call. `Print` takes a single `String`
component and writes it to stdout:

```text
(String) => Unit
```

There is no `Stdout` capability to thread through. The compiler lowers
`Print` against the standard `wasi:cli/stdout` interface, so the
resulting `.wasm` runs on any Component Model host.

## Reading Arguments

A program that reads its argument vector declares the variant entry
`Args => Exit` -- the command's **argument vector flows in as `Args`,
an exit status flows out as `Exit`**, mirroring the HTTP entry:

```canon
Args => Exit {
    Args
        -> Length
        -> String
        -> Print
    Exit(0)
}
```

`Args` (`= List<String>`, from `canon/std`) is the program's `argv`,
handed to the entry the way an HTTP handler is handed its `Request`.
`Exit` (`= Int`) is the exit status: `Exit(0)` is success (process
exit 0); any nonzero `Exit` reports failure.

## Try Breaking Things

- **Add a second `-> Print` line.** Each call writes its argument followed
  by a newline.
- **Add a comment** (`// hi`). The lexer rejects it; comments are not
  allowed.
- **Drop the `Exit(0)` line** from the arg-reading variant. The body's
  last expression must match the declared return type (`Exit`), so ending
  on a `Print` (which yields `Unit`) is a checker error.
- **Inspect the compiled component.** `canon build hello.can` writes
  `build/hello/hello.wasm` and a sibling `.wit` describing the
  component's world.

Next stop on the voyage: [Types & Values](../learn/types-and-values.md),
the first Learn chapter.
