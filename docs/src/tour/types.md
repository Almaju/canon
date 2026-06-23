# Types

Every type in Canon is built by composing two operators — `+` for "or",
`*` for "and" — over a small core of primitives.

## Naming

- **Types** and **traits**: `PascalCase`
- **Methods**: `camelCase`

The case difference distinguishes a method from a trait implementation
declared on the same type:

```canon
Type.print  // method
Type.Print  // implementation of the `Print` trait
```

## Unions (`+`)

A union expresses "this or that":

```canon
Bit  = Off + On
Bool = False + True
Ord  = Equal + Greater + Less
```

Variants must be listed in alphabetical order. There is no separate `enum`
keyword.

## Products (`*`)

A product expresses "this and that". A value of the resulting type has all
of its components:

```canon
User = Birthday * Username
```

Components must be in alphabetical order. There is no separate `struct`
keyword.

### Field Access

A product's components are addressed by their type name:

```canon
user.Birthday
user.Username
```

For repeated components or anonymous sequences, use 1-based positional
indices:

```canon
Byte = Bit^8

byte.1   // first Bit
byte.2   // second Bit
```

## Newtypes

Aliasing a type creates a distinct new type that wraps the original:

```canon
Birthday = String
Username = String
```

`Birthday` and `Username` cannot be used interchangeably. They share
storage, but they are different types — which is exactly the point. See
[Philosophy](./philosophy.md) on why types are the documentation.

## Fixed and Unbounded Repetition

For a fixed count of the same type, use `Type^N`:

```canon
Byte = Bit^8
```

For unbounded sequences, use `Type^*`:

```canon
Bytes = Byte^*
```

Higher-level types like `Int`, `Float`, and `String` are defined from
`Byte` / `Bytes`.

## Generics

Type parameters use angle brackets:

```canon
List<T>
Option<T>
Result<T, E>
Map<String, Int>
```

Constraints on type parameters use `:`, naming a trait the parameter must
implement:

```canon
print = <T: Print>(List<T>) -> Unit {
    ...
}
```

## Singleton Types

A type with no underlying composition has exactly one value, referenced by
writing the type name itself:

```canon
main = () -> Unit {
    Unit
}
```

`Unit` in return position is the type; `Unit` in expression position is its
sole value. No constructor call is needed (and would not work — there is
no data to pass).

## Recursive Types

Recursive type definitions are allowed and **boxed automatically**:

```canon
Branch = Left * Right * Value
Left   = Tree
Right  = Tree
Tree   = Branch + Leaf
Value  = Int
```

There is no user-visible `Box<T>`. The compiler chooses an indirection
scheme; it is never spelled out in source.

## Type Inference

There is **none**. Every type must be explicitly written. If a function
declares it returns `Result<T, Err>` but no `Err` ever flows through, that
is a compile-time error — declared types must match inferred shape
exactly.
