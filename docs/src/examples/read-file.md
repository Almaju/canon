# Reading a File

[`examples/read-file`](https://github.com/Almaju/canon/tree/main/examples/read-file):
the canonical demonstration of Canon's *type chain as access control*.

```canon
Unit => Program {
    Path("Cargo.toml")
        -> File?
        .read()?
        -> Print
}
```

```sh
$ canon run examples/read-file
[package]
name = "canon"
…
```

## The Chain

Read it type by type:

```
String ──Path()──> Path ──File()?──> File ──read()?──> String ──print()──> Unit
```

- `Path = String`, a newtype: constructing it costs nothing but says
  what the string *is*.
- `Path.File()` is the **validated constructor** of `File`: it performs
  the open, so its signature is `(Path) => Result<File, IoError>`.
  Opening can fail, so the type says so, and the `?` is forced at the
  call site. You cannot forget to handle a missing file.
- `File.read()` is `(File) => Result<String, IoError>`. You can only
  read a `File` value, and the only way to get one was to successfully
  open a path.

There is no filesystem service to inject, no permission check to
remember. A function without a `File` (or a `Path` to make one from)
*cannot* do file I/O; the capability is the value. See
[Effects and Values](../tour/effects.md).

## Where the Errors Go

Both `?`s propagate `IoError` out of `main`. A missing file surfaces as
the program failing with the error value rather than printing. Point
the `Path` at a file that doesn't exist to see it.
