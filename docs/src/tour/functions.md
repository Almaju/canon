# Functions

Every function is implemented on a type. The general form is:

```oneway
Type.functionName = (params) -> ReturnType {
    body
}
```

The only exception is `main`, the program's entry point.

## A First Method

```oneway
Greeting = String

Greeting.shout = () -> String {
    "HELLO"
}

main = (Stdout) -> Noop {
    Greeting("howdy").shout().print(Stdout)
}
```

`Greeting.shout` is a method on `Greeting`. It is called with dot syntax:
`Greeting("howdy").shout()`.

## Method Bodies

A body is a **newline-separated sequence of expressions**. The last
expression is the return value. There are no semicolons.

- Dispatch (`.( )`) is an expression — it can be the final line of a body
  or appear as a sub-expression.
- Non-final lines whose results are discarded are valid (they exist for
  side effects or `?` propagation).

```oneway
File.readConfig = (Path) -> Result<Config, IoError | ParseError> {
    File.read(Path)?
        .parse()?
        .validate()
}
```

There are no local variables. The only way to thread a value through
multiple operations is method chaining. That is the intended style.

## Referring to the Receiver

Inside a method body, the receiver value is referenced by **the receiver
type's name**:

```oneway
String.print = (Stdout) -> Noop {
    Stdout.write(String)    // `String` here is the receiver value
}
```

The `Self` keyword is an alias, available everywhere. It is required only
when the receiver's type name collides with a parameter of the same type:

```oneway
Int.add = (Int) -> Int {
    ...   // ambiguous: which `Int`?
}
```

Resolve it either by using `Self`:

```oneway
Int.add = (Int) -> Int {
    Self.plus(Int)
}
```

…or by introducing a newtype for the parameter:

```oneway
OtherInt = Int

Int.add = (OtherInt) -> Int {
    Int.plus(OtherInt)
}
```

`Self` is the lighter-weight choice. The alias is right when the
distinction is meaningful enough to warrant a name.

## Declaration Order

Multiple methods on the same type must be declared in alphabetical order.
This is a compile-time requirement, not a convention:

```oneway
User.add    = (...) -> ...
User.export = (...) -> ...
User.remove = (...) -> ...
```

## Visibility

Everything is **public by default**. Prefix a method with `*` to make it
private to its declaring file:

```oneway
Type.*helper = () -> Noop {
    ...
}
```

## Optional Parameters

There is no special syntax. Use `Option<T>`:

```oneway
String.print = (Option<Color>) -> Noop {
    ...
}
```

This allows both forms at the call site:

```oneway
"hello".print()
"hello".print(Red)
```

## First-Class Functions

Methods are first-class values. Refer to one by its qualified name
`Type.method` and pass it where a matching trait signature is expected:

```oneway
Numbers = Int^*

Numbers.doubleAll = () -> Numbers {
    Numbers.map(Int.double)
}
```

## Lambdas

For one-off operations, write a lambda literal with its **full signature**.
There is no signature inference:

```oneway
Numbers.tripleAll = () -> Numbers {
    Numbers.map((Int) -> Int { Int.mul(Int(3)) })
}
```

Lambda syntax mirrors method declaration syntax: `(params) -> ReturnType
{ body }`. The only difference is the absence of a `Type.name =` prefix.

## Generic Methods

A method can be parameterized by a type. Declare type parameters with
`<...>` before the parameter list, optionally with a trait constraint:

```oneway
List.print = <T: Print>() -> Noop {
    ...
}
```

When calling a generic method whose type parameter can't be inferred from
context, pin it with `::<...>` (turbofish) after the method name:

```oneway
Json.parse::<List<Int>>("[1, 2, 3]")?
```

Turbofish is only required when the surrounding type context doesn't
already determine the parameter. A function with an explicit
`Result<List<Int>, _>` return type lets the compiler infer from the
return position without an annotation.

## The `main` Function

`main` is the single exception to "every function is on a type". It is a
top-level free function and the program's entry point. It typically takes
the capabilities the program needs:

```oneway
main = (Stdout) -> Noop {
    "hello".print(Stdout)
}
```

See [Capabilities](./capabilities.md) for how this connects to side
effects.
