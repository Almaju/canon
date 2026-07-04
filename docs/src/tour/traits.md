# Traits

A trait is a callable type signature. It is declared like a function type:

```canon
Show = () -> String
```

Because traits are types, they are written in `PascalCase`. The case
difference is how the compiler distinguishes a trait implementation
(`Print`) from a regular function (`print`).

## Implementing a Trait

A trait is implemented for a type by declaring a function with the trait's name and the implementing type as a component:

```canon
Greeting = String

Name = String

Show = () -> String

Show = (Greeting) -> String {
    "HELLO!"
}

Show = (Name) -> String {
    "Alice"
}

main = () -> Unit {
    Greeting("hi")
        .Show()
        .print()
    Name("Alice")
        .Show()
        .print()
}
```

`Greeting.Show()` and `Name.Show()` both have the same signature
(`() -> String`) and are called the same way.

## Multi-Method Traits

A trait with multiple methods is just a product of single-method traits:

```canon
Debug = () -> String

Presentable = Debug * PrintString

PrintString = () -> Unit
```

Implementing `Presentable` for a type means implementing both `Debug`
and `PrintString` for it.

## Using a Trait as a Parameter

A trait can be used directly as a parameter type. The parameter binds the
trait implementation, which is then invocable:

```canon
needsPrint = (Print) -> Unit {
    Print()
}
```

## Generic Constraints

Constraints on generic parameters use `:`, naming a trait the parameter
must implement:

```canon
print = <T: Print>(List<T>) -> Unit {
    ...
}
```
