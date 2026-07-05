# Functions

A function is declared as:

```canon
name = (Components) => ReturnType {
    body
}
```

The components inside the parentheses form a product, the function's input. Any component can appear before the dot at the call site (commutative calling).

## A First Function

```canon,run=first-function
Greeting = String

Shout = String

Greeting => Shout {
    "HELLO"
}

() => Unit {
    Greeting("howdy")
        -> Shout
        -> Print
}
```

The constructor produces a `Shout` from a `Greeting`. It is reached with the `->` pipe via commutative calling: `Greeting("howdy") -> Shout` feeds the greeting in as the constructor's component.

## Function Bodies

A body is a **newline-separated sequence of expressions**. The last
expression is the return value. There are no semicolons.

- Dispatch (`.( )`) is an expression; it can be the final line of a body
  or appear as a sub-expression.
- Non-final lines whose results are discarded are valid (they exist for
  side effects or `?` propagation).

```canon
readConfig = (File * Path) => Result<Config, IoError + ParseError> {
    File
        .read(Path)?
        .parse()?
        .validate()
}
```

There are no local variables. The only way to thread a value through
multiple operations is method chaining. That is the intended style.

## Referring to Components

Inside a function body, each component is referenced by **its type name**:

```canon
format = (Greeting * Name) => String {
    Greeting
}
```

When two components share the same type, introduce a newtype alias;
product members must be distinct types:

```canon
OtherInt = Int

sum = (Int * OtherInt) => Int {
    Int -> Sum(OtherInt)
}
```

## Declaration Order

Functions in the same file must be declared in alphabetical order.
This is a compile-time requirement, not a convention:

```canon
add    = (User * ...) -> ...
export = (User * ...) => ...
remove = (User * ...) => ...
```

## Optional Parameters

There is no special syntax. Use `Option<T>`:

```canon
paint = (Option<Color> * String) => Unit {
    ...
}
```

This allows both forms at the call site:

```canon
"hello".paint()
"hello".paint(Red(0xFF0000))
```

Omitting a component is legal only when its type implements the
`Default` trait. Core ships exactly one implementation, `Option<T>`'s,
which fills the gap with `None()`.

## First-Class Functions

Functions are first-class values. Refer to one by its qualified name
and pass it where a matching signature is expected:

```canon
Numbers = Int^*

doubleAll = (Numbers) => Numbers {
    Numbers -> Mapped(Int.double)
}
```

## Lambdas

For one-off operations, write a lambda literal with its **full signature**.
There is no signature inference:

```canon
tripleAll = (Numbers) => Numbers {
    Numbers -> Mapped((Int) => Int { Int -> Product(Int(3)) })
}
```

Lambda syntax mirrors function declaration syntax: `(Components) -> ReturnType
{ body }`. The only difference is the absence of a `name =` prefix.

## Generic Functions

A function can be parameterized by a type. Declare type parameters with
`<...>` before the parameter list, optionally with a trait constraint:

```canon
print = <T: Print>(List<T>) => Unit {
    ...
}
```

When calling a generic function whose type parameter can't be inferred from
context, pin it with `::<...>` (turbofish) after the function name:

```canon
Json.parse::<List<Int>>("[1, 2, 3]")?
```

Turbofish is only required when the surrounding type context doesn't
already determine the parameter. A function with an explicit
`Result<List<Int>, _>` return type lets the compiler infer from the
return position without an annotation.

## The CLI Entry

The program's entry point takes no parameters and returns `Unit`; it is
selected by that signature — no name required — and lifted as the
component's `wasi:cli/run.run` export:

```canon
() => Unit {
    "hello" -> Print
}
```

For I/O, construct the value that carries the effect (`File`, `Url`,
`HttpServer`) from inside the entry and thread it through the chain. See
[Effects and Values](./effects.md) for the full domain-first story.
