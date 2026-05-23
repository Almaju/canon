# Traits

A trait is a callable type signature. It is declared like a function type:

```oneway
Show = () -> String
```

Because traits are types, they are written in `PascalCase`. The case
difference is how the compiler distinguishes a trait implementation
(`Print`) from a regular function (`print`).

## Implementing a Trait

A trait is implemented for a type by declaring a function with the trait's name and the implementing type as a component:

```oneway
Show = () -> String

Greeting = String
Name     = String

Show = (Greeting) -> String {
    "HELLO!"
}

Show = (Name) -> String {
    "Alice"
}

main = () -> Unit {
    Greeting("hi").Show().print()
    Name("Alice").Show().print()
}
```

`Greeting.Show()` and `Name.Show()` both have the same signature
(`() -> String`) and are called the same way.

## Multi-Method Traits

A trait with multiple methods is just a product of single-method traits:

```oneway
Show = Debug * PrintString
```

## Default Implementations

A trait declaration can carry a default body marked `{ impl }`:

```oneway
Greet = () -> String { impl }
```

Implementing types may then either override or inherit the default.

## Using a Trait as a Parameter

A trait can be used directly as a parameter type. The parameter binds the
trait implementation, which is then invocable:

```oneway
needsPrint = (Print) -> Unit {
    Print()
}
```

## Generic Constraints

Constraints on generic parameters use `:`, naming a trait the parameter
must implement:

```oneway
print = <T: Print>(List<T>) -> Unit {
    ...
}
```
