# Modules and Packages

## Files and Modules

- Source files are named in **kebab-case**: `note.can`,
  `http-server.can`, `io-error.can`.
- A file **must declare the type it is named after**: `http-server.can`
  declares `HttpServer`. The kebab-case name is the mechanical
  conversion of the PascalCase type name.
- A **module is a folder**. There is no `mod` declaration; directory
  structure is the module structure.
- A package's entry point is `src/main.can`; a library's root is
  `lib.can`.

## Imports

There is **no import statement**. A reference *is* the import:
mentioning `User` in a file that doesn't define it loads `user.can`.
Wherever choice is discretionary, Canon removes the concept -- and an
import line is pure ceremony once files are named after what they
declare.

For a reference to a name `Z` the current file does not define, the
loader searches:

| Location | Rule |
|---|---|
| the file's own directory tree | name -> file convention: `z.can` or `z/main.can` (kebab-case of `Z`), recursively, skipping `deps/` and `bindgen/` |
| the project's `bindgen/` tree | by declared name -- binding files declare functions whose names don't kebab back to their file (`getRandomU64` lives in `random.can`) |
| the project's `deps/` tree | vendored packages, by declared name |
| the bundled packages (`canon/std`) | by declared name; the stdlib's hand-written wrappers shadow its internal bindgen substrate |

**Ambiguity is a hard error, not a precedence.** A name that resolves
in more than one location fails the build naming every candidate --
there is no shadowing, so names are globally unique across a project,
its dependency closure, and the standard library. A name that resolves
nowhere is left for the checker, which reports the undefined name with
full type context.

A resolved reference brings the type **with its constructor and
methods** (no wildcards, no aliasing -- there is nothing to write).
This is why the naming convention has teeth: files are
`kebab-case.can` of the PascalCase type they declare, so "which file
defines `HttpServer`?" has exactly one mechanical answer, and the
compiler applies it so you never write it down.

Version pins live in the filesystem, never in source: a vendored
package occupies `deps/<ns>/<name>@<version>/`, the directory name is
the pin, and a reference to `Decoder` carries no `@version`.

## Visibility

Everything is **public**. There is no `pub`, no private modifier.
Encapsulation matters in one place, protecting a type's invariants, and
that is handled by [validated constructors](./types.md#validated-constructors):
declaring a constructor replaces the implicit total one, and only
functions in the type's own file can touch the raw representation.

## No Manifest

There is **no package manifest**. `canon.toml` left the language the
same way the import statement did: wherever a config file would
restate what the file structure already says, Canon keeps the file
structure and removes the file (the toolchain keeps its `use` registry
outside the project for the same reason). A project is defined
entirely by its layout:

| Path | Meaning |
|---|---|
| `src/main.can` | the package's entry point — its presence makes the directory a package; the directory's name is the package's name |
| `wit/` | external imports: every immediate entry is a WIT source — a `.wit` file, a directory of them, or a `.wasm` component |
| `bindgen/` | bindings `canon install` materialized from `wit/` (derived; conventionally gitignored) |
| `deps/` | vendored Canon-package dependencies, `deps/<ns>/<name>@<version>/` — the directory name is the pin |
| `build/` | compiler output (gitignored) |

The nearest ancestor directory carrying one of these markers is the
project root; `wit/`, `bindgen/`, and `deps/` are resolved there.

## External Imports (`wit/`)

Dropping a WIT source under `wit/` *is* the import declaration.
`canon install` (run explicitly, or implicitly by
`canon build`/`run`/`check`/`test` when the bindings are stale)
materializes every source into `bindgen/` as binding files
([Compilation and the ABI](./compilation.md#binding-files)), laid out
as `bindgen/<ns>/<pkg>@<version>/<iface>.can` — versions come from the
WIT sources themselves. A `.wasm` component under `wit/` is recorded
as deferred until build-time composition lands.

## Dependencies (`deps/`)

`canon install <ns>:<name>[@ver]` fetches a package from its registry
and vendors it under `deps/<ns>/<name>@<version>/`. **The directory
tree is the lockfile**: the version pin is the directory name, and the
recorded dependency list of a published package is read off its
`deps/` directory names. There is no `canon.lock`.

## Workspaces

A workspace is a directory of packages: any directory that is not
itself a package but whose immediate subdirectories include packages.
There is no member list — adding a package subdirectory adds a member.
`canon build` builds every member (each into its own `build/`);
`canon run -p foo` selects one.

## Bundled Packages

`canon/std` ships inside the compiler binary. It is pre-installed but
indistinguishable from any other package at the language level: it
vendors its WIT imports under `wit/wasi/`, and its bindings are
generated by the same `canon install` mechanism user packages use.
There is no privileged stdlib path.
