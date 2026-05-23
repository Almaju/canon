# Hello, World

Create a file named `hello.ow`:

```oneway
main = (Stdout) -> Unit {
    "hello".print(Stdout)
}
```

Run it:

```sh
oneway run hello.ow
```

You should see:

```oneway
hello
```

That's the whole program. Let's walk through it.

## Line by Line

```oneway
main = (Stdout) -> Unit {
```

`main` is the program's entry point. Unlike every other function in Oneway,
`main` is **not** implemented on a type — it's a top-level binding.

The signature `(Stdout) -> Unit` says: this function takes one parameter
whose type is `Stdout`, and returns a value of type `Unit`.

- `Stdout` is a **capability**. Real-world capabilities only exist in
  `main`, which receives them and threads them down to anything that needs
  to perform a side effect.
- `Unit` is a singleton type — a type with exactly one value, named after
  itself. Returning `Unit` is the language's way of saying "this function
  produces nothing useful".

```oneway
    "hello".print(Stdout)
}
```

`"hello"` is sugar for `String("hello")`. The body of a function is a
sequence of expressions separated by newlines; the last one is the return
value.

`"hello".print(Stdout)` is a method call.
The method `print` takes a `String` and a `Stdout` capability:

```oneway
print = (Stdout * String) -> Unit {
    ...
}
```

`print` needs a `Stdout` capability because writing to standard output is a
side effect. A function that does not receive `Stdout` cannot print, and
the type signature is honest about that.

## Try Breaking Things

Some small experiments to build intuition:

- **Remove `Stdout` from `main`'s parameters.** The compiler will complain
  when `print` is called without it.
- **Add a comment** (`// hi`). The lexer rejects this — comments are not
  allowed.
- **Return something other than `Unit`.** The body's last expression must
  match the declared return type.
