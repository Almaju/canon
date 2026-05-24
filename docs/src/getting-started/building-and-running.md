# Building and Running

Oneway commands operate on a **target**, which is one of:

- a **package directory** (`oneway.toml` + `src/main.ow`),
- a **workspace directory** (`oneway.toml` with a `[workspace]` table
  aggregating member packages), or
- a single **`.ow` file** (anonymous single-file package).

When omitted, the target defaults to the current directory.

## Package Layout

```text
my-app/
  oneway.toml      # name, version, dependencies
  src/
    main.ow        # entry point
    helpers.ow
  build/           # compiler output (gitignored)
    my-app.wasm
    my-app.wit
```

The artifact is named after the package, not the entry file.

## Workspace Layout

A workspace is a directory whose `oneway.toml` carries a `[workspace]`
table instead of (or in addition to) `name`/`version`. Each member is a
full package; all members share the workspace's `build/`, Cargo-style.

```text
my-workspace/
  oneway.toml      # [workspace] members = ["*"]
  build/           # shared artifacts
    foo.wasm
    bar.wasm
  foo/
    oneway.toml
    src/main.ow
  bar/
    oneway.toml
    src/main.ow
```

`members = ["*"]` is a shorthand for "every immediate subdirectory that
contains an `oneway.toml`". Explicit lists (`members = ["foo", "bar"]`)
work too. Nested workspaces are not allowed.

## Run a Program

```sh
oneway run                       # current directory as a package
oneway run path/to/pkg           # another package
oneway run hello.ow              # single-file mode
oneway run my-workspace -p foo   # one member of a workspace
```

Compiles the target to a WebAssembly component and immediately runs it
through the embedded `wasmtime` runtime. `oneway run` on a workspace
without `-p` is ambiguous and errors with a hint listing the members.

## Build a Component

```sh
oneway build                     # current directory as a package or workspace
oneway build hello.ow            # single-file mode
oneway build my-workspace        # builds every member
oneway build my-workspace -p foo # builds only `foo`
```

In package mode, produces `build/<name>.wasm` and `build/<name>.wit`
next to the package's `src/`. In workspace mode, every member's
artifacts land in the workspace's shared `build/` directory. In
single-file mode, the output goes to `build/<stem>/<stem>.wasm` next to
the file. The component runs on any host that supports WASI Preview 3
and satisfies its imports — `oneway run`, `wasmtime serve`, browser
polyfills, edge runtimes, etc.

## Inspect Generated WAT

```sh
oneway emit hello.ow
```

Prints the **core** wasm module as WebAssembly Text. This is the fastest
way to see how Oneway constructs map to wasm — print statements, dispatch,
heap allocation, async lowering — without dragging in the component
wrapping layer.

## Show Tokens or AST

```sh
oneway tokens hello.ow
oneway ast    hello.ow
```

Diagnostic tools — useful when you want to understand exactly how the
lexer or parser sees your code.

## Check Sort Order and Types

```sh
oneway check                     # current package or workspace
oneway check hello.ow            # single-file mode
oneway check my-workspace -p foo # only one member of a workspace
```

Runs the full checker (sort-order rules plus type checking) without
codegen. Fast — useful as an editor lint or pre-commit gate.

## Format

```sh
oneway fmt hello.ow
oneway fmt hello.ow --check     # exit 1 if not already formatted
```

## Run Tests

```sh
oneway test mymod_test.ow
```

Discovers every `() -> TestResult` function in the file and prints a
`[ ok ]` / `[FAIL]` line per test. See the
[testing notes in `CLAUDE.md`](https://github.com/Almaju/oneway/blob/main/CLAUDE.md#testing)
for the full conventions.

## Language Server

```sh
oneway lsp
```

Speaks LSP over stdio. The Zed extension wires this up automatically;
other editors can point at the same binary.

## All Commands

```sh
oneway help
```

## Workflow

There is no `oneway new` or project scaffolder yet. For quick
experimentation, drop a `.ow` file anywhere and `oneway run` it. For
proper projects, create an `oneway.toml` next to a `src/main.ow` and
run `oneway build` / `oneway run` from that directory. For multi-file
projects, see [Modules](../tour/modules.md).
