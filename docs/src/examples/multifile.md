# A Multi-File Project

[`examples/multifile`](https://github.com/Almaju/canon/tree/main/examples/multifile):
the smallest possible demonstration of Canon's module system.

```text
multifile/
  canon.toml
  src/
    greeter.can
    main.can
```

`src/greeter.can` must declare `Greeter`; the file name says so. It also
declares `Shout` — the result of shouting a greeting — and a constructor
that produces one from a `Greeter`:

```canon
Greeter = String

Shout = String

Greeter => Shout {
    "HELLO from greeter"
}
```

`src/main.can`:

```canon
Unit => Program {
    Greeter("hi")
        -> Shout
        -> Print
}
```

```sh
$ canon run examples/multifile
HELLO from greeter
```

## What the Example Pins Down

- **One type per file, file named after the type.** `greeter.can` ↔
  `Greeter`. The compiler enforces the correspondence, so "where is this
  type defined?" always has a mechanical answer.
- **Referencing `Greeter` imports the type *and its constructors*.**
  `main.can` never writes an import line — mentioning `Greeter` loads
  `greeter.can`, and `Shout` travels with it, so `-> Shout` resolves.
- **No `mod`, no manifest of files.** The directory *is* the module
  structure; adding a file is adding a type.
- **Everything is public.** There is no visibility to configure; see
  [Modules and Packages](../spec/modules.md#visibility) for why.

The [tutorial's modules chapter](../tutorial/04-modules.md) applies this
same move to a real service.
