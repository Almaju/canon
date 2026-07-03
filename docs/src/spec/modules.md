# Modules, Packages, and Name Resolution

## Files and Modules

- Source files are named in **kebab-case**: `note.can`,
  `http-server.can`. A file declares one primary type, whose
  PascalCase name is the file stem's PascalCase form.
- There are no import statements. A file's referenced names resolve
  automatically (below); folders organize files without scoping them.

## Name Resolution

An unresolved PascalCase reference (a type position, a constructor
call, a PascalCase method or field name) resolves in this order:

1. **Declarations already in the program** — the file's own
   declarations and everything previously loaded.
2. **The entry's directory tree** — any `.can` file declaring the name
   (generated and output directories — `bindgen/`, `build/`,
   `target/` — are skipped). If two files declare it, the program does
   not compile until one is renamed or an alias disambiguates.
3. **The bundled standard library's curated (`src/`) modules** — by
   declaration name, or by the file-naming convention (`Stream` finds
   `stream.can` even though the file only declares combinators).

The most local wins: your package shadows std. Names that resolve
nowhere are reported by the checker as unknown types with proper
spans. Interpolated JSON literals implicitly reference `ToJson`, which
is how `canon/std/Json` loads without ever being named (the
[prelude](./expressions.md#json-literals)).

Loading a file brings **all** of its declarations (its type, its
functions, its trait impls) plus, transitively, whatever that file's
own references resolve to.

## Alias Declarations

`Local = seg/…/Name` is the one explicit import form — an ordinary
declaration whose right-hand side is a path:

```canon
HttpStatus = std/http/Status

now = wasi/clocks/monotonic_clock/now
```

- The final segment names the declaration; the segments before it
  locate the file (kebab-case of a PascalCase name, or the literal
  segment before a camelCase one).
- `std/…` addresses the bundled standard library. Other prefixes
  resolve against the project's `bindgen/` tree (from the manifest's
  `[imports]`) and bundled packages.
- A PascalCase alias may **rename**: `HttpStatus = std/http/Status`
  declares `HttpStatus` as an alias of the loaded `Status`. A
  camelCase (function) alias must keep its own name.
- **Bindgen output is reachable only through aliases.** Generated
  binding files never join name resolution — the collision-heavy,
  machine-written FFI layer stays in the basement.

Alias declarations sort alphabetically at the top of the file
(`canon fmt` does it).

## Packages

A package is a directory with a `canon.toml` manifest and a `src/`
tree. The package is the unit of name resolution and of type-name
uniqueness. The manifest's `[deps]` table declares which packages are
in scope — the manifest, not the source file, is where vocabulary is
granted (the same shape as the capability story: the entry signature
is the authority manifest, `canon.toml` is the vocabulary manifest).
`[imports]` declares WIT sources that `canon install` materializes
under `bindgen/`.

The bundled `canon/std` package is pre-installed and addressed as
`std` in alias paths. Its curated modules resolve by name like your
own files; its `bindgen/` tree follows the alias-only rule like
everyone else's.
