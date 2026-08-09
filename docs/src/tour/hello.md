# Hello, Canon

Nothing here is called `main`. The compiler reads the signature —
`Unit => Program` — and *that shape is the entry point*, the same way
`Request => Response` is an HTTP service.

`Unit` is the name of "no input". `Program` is the world the arrow
returns. `"hello, world"` is sugar for `String("hello, world")`, and
`->` pipes it into `Print`.

There is no import statement either: `Print` and `Program` resolve to
the standard library by name, on their own.

Press **run**. The Canon compiler is a WebAssembly module inside this
tab, so your program is compiled to a component and executed here —
nothing is sent to a server. Then change the message and run it again.

```canon,run
Unit => Program {
    "hello, world" -> Print
}
```
