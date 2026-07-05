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

`src/greeter.can` must declare `Greeter`; the file name says so:

```canon
{{#include ../../../examples/multifile/src/greeter.can}}
```

`src/main.can`:

```canon
{{#include ../../../examples/multifile/src/main.can}}
```

```sh
$ canon run examples/multifile
HELLO from greeter
```

## What the Example Pins Down

- **One type per file, file named after the type.** `greeter.can` ↔
  `Greeter`. The compiler enforces the correspondence, so "where is this
  type defined?" always has a mechanical answer.
- **Referencing `Greeter` imports the type *and its methods*.**
  `main.can` never writes an import line — mentioning `Greeter` loads
  `greeter.can`, and `.shout()` travels with its type.
- **No `mod`, no manifest of files.** The directory *is* the module
  structure; adding a file is adding a type.
- **Everything is public.** There is no visibility to configure; see
  [Modules and Packages](../spec/modules.md#visibility) for why.

The [modules section of the tour](../guide.md#modules) states the rule;
[Modules and Packages](../spec/modules.md) has the full resolution
algorithm.
