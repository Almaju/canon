# Modules

Oneway's module system is file-based and conventionally driven. There is
no `mod` declaration, no manifest of what's in scope.

## File Rules

- Files are named `kebab-case.ow` — `http-server.ow` declares `HttpServer`,
  `user-role.ow` declares `UserRole`, `foo.ow` declares `Foo`.
- A file's name **must match** the type it declares.
- A **module is a folder**. There is no `mod` keyword.
- The entry point is `main.ow`. A library's root is `lib.ow`.

## Imports

To use a type defined in a sibling file, write:

```oneway
use Foo
```

This imports `Foo` from `foo.ow` (or from the corresponding folder if
`Foo` is a module). No paths, no aliasing.

To import from the embedded standard library, prefix the path with
`std/`:

```oneway
use std/File
use std/Url
```

See the [Standard Library](../reference/stdlib.md) reference for the
full list.

Multiple `use` statements at the top of a file must be in alphabetical
order.

## Example: Multi-File Project

```
examples/multifile/
├── greeter.ow
└── main.ow
```

`greeter.ow`:

```oneway
Greeter = String

shout = (Greeter) -> String {
    "HELLO from greeter"
}
```

`main.ow`:

```oneway
use Greeter

main = () -> Unit {
    Greeter("hi").shout().print()
}
```

Run it with:

```sh
just example multifile
```

## Visibility

Everything is **public**. There is no private visibility modifier.
