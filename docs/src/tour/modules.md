# Modules and Name Resolution

Canon has no `use`, no `import`, no `mod`. **Referencing a type is
importing it.** An unknown PascalCase name resolves, in order, against:

1. **Your own files** — every `.can` file in the entry's directory
   tree. A folder is a shelf, not a scope: `Note` resolves from
   `note.can` wherever you shelved it, and type names are unique per
   package (two files declaring the same type is a compile error
   naming both).
2. **The standard library** — `Random()`, `Instant()`, `Url("…")` just
   work; the compiler loads the module the moment you reach for the
   name.

```canon
main = () -> Unit {
    Path("./notes.txt")
        .File()?
        .read()?
        .print()
}
```

That's a complete program. `Path` and `File` resolve from `std`'s `fs`
modules because the program uses them — the import line they would
have occupied carried no information the compiler didn't already have.

## Alias Declarations

The one explicit form is an ordinary declaration whose right side is a
path. It exists for two jobs:

**Disambiguation and renaming.** Your package beats std, so defining
your own `Status` never breaks — but if you also want the shadowed
one, or two names collide, say which:

```canon
HttpStatus = std/http/Status
```

**Reaching bindgen output.** Machine-generated FFI bindings (the
`bindgen/` trees, bundled or `canon install`-ed) never participate in
name resolution. An alias is the only doorway:

```canon
now = wasi/clocks/monotonic_clock/now
```

Alias declarations group at the top of the file, alphabetically —
`canon fmt` places them.

## Files and Folders

- One primary type per file; the file is the type's kebab-case name
  (`HttpServer` lives in `http-server.can`). You never write the
  mapping — it's how the compiler shelves and finds declarations.
- A package is a directory with a `canon.toml` and a `src/`. The
  package is the unit of name resolution and of type-name uniqueness.
- Visibility of *internals* stays file-scoped: resolving `Url` from
  anywhere doesn't bypass its validated constructor.

See the [specification](../spec/modules.md) for the precise rules.
