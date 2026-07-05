# Modules

The module system is file-based. There is no `mod` declaration, no
manifest of what's in scope.

## File Rules

- Files are named `kebab-case.can`: `http-server.can` declares `HttpServer`,
  `user-role.can` declares `UserRole`, `foo.can` declares `Foo`.
- A file's name **must match** the type it declares.
- A **module is a folder**. There is no `mod` keyword.
- The entry point is `main.can`. A library's root is `lib.can`.

## Imports

There is no import statement. To use a type defined in a sibling file,
just refer to it — the reference to `Foo` loads `foo.can` (or
`foo/main.can` if `Foo` is a module) automatically. No paths, no
aliasing, nothing to write at the top of the file.

The same rule reaches the embedded standard library: referencing
`File` or `Url` in a project that doesn't define them finds the
stdlib's `file.can` / `url.can`. The loader searches your project's
own files, then its `bindgen/` tree, then vendored packages under
`deps/`, then `canon/std` — and a name that resolves in more than one
place is a compile error naming every candidate. There is no
shadowing: names are globally unique across a project, its
dependencies, and the standard library.

The [Standard Library](../reference/stdlib.md) reference has the full
list of stdlib names.

## Example: Multi-File Project

```
examples/multifile/
├── greeter.can
└── main.can
```

`greeter.can`:

```canon
Greeter = String

shout = (Greeter) => String {
    "HELLO from greeter"
}
```

`main.can`:

```canon
main = () => Unit {
    Greeter("hi")
        .shout()
        -> Print
}
```

The reference to `Greeter` in `main.can` is the entire import: the
loader finds the sibling `greeter.can` by name.

Run it with:

```sh
just example multifile
```

## Visibility

Everything is **public**. There is no private visibility modifier.
