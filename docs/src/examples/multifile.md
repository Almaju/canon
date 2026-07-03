# A Multi-File Project

[`examples/multifile`](https://github.com/Almaju/canon/tree/main/examples/multifile)
— the smallest possible demonstration of Canon's module system.

```text
multifile/
  canon.toml
  src/
    greeter.can
    main.can
```

`src/greeter.can` — the file is named `greeter.can`, so it must declare
`Greeter`:

```canon
Greeter = String

shout = (Greeter) -> String {
    "HELLO from greeter"
}
```

`src/main.can`:

```canon
use Greeter

main = () -> Unit {
    Greeter("hi")
        .shout()
        .print()
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
- **`use Greeter` imports the type *and its methods*.** `main.can`
  calls `.shout()` without importing it separately — methods travel
  with their type.
- **No `mod`, no manifest of files.** The directory *is* the module
  structure; adding a file is adding a type.
- **Everything is public.** There's no visibility to configure; see
  [Modules and Packages](../spec/modules.md#visibility) for why.

The [tutorial's modules chapter](../tutorial/04-modules.md) applies this
same move to a real service.
