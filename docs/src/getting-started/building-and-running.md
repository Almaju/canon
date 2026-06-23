# Building and Running

Canon commands operate on a **target**, which is one of:

- a **package directory** (`canon.toml` + `src/main.can`),
- a **workspace directory** (`canon.toml` with a `[workspace]` table
  aggregating member packages), or
- a single **`.can` file** (anonymous single-file package).

When omitted, the target defaults to the current directory.

## Package Layout

```text
my-app/
  canon.toml      # name, version, dependencies
  src/
    main.can        # entry point
    helpers.can
  build/           # compiler output (gitignored)
    my-app.wasm
    my-app.wit
```

The artifact is named after the package, not the entry file.

## Workspace Layout

A workspace is a directory whose `canon.toml` carries a `[workspace]`
table instead of (or in addition to) `name`/`version`. Each member is a
full package; all members share the workspace's `build/`, Cargo-style.

```text
my-workspace/
  canon.toml      # [workspace] members = ["*"]
  build/           # shared artifacts
    foo.wasm
    bar.wasm
  foo/
    canon.toml
    src/main.can
  bar/
    canon.toml
    src/main.can
```

`members = ["*"]` is a shorthand for "every immediate subdirectory that
contains an `canon.toml`". Explicit lists (`members = ["foo", "bar"]`)
work too. Nested workspaces are not allowed.

## Run a Program

```sh
canon run                       # current directory as a package
canon run path/to/pkg           # another package
canon run hello.can              # single-file mode
canon run my-workspace -p foo   # one member of a workspace
```

Compiles the target to a WebAssembly component and immediately runs it
through the embedded `wasmtime` runtime. `canon run` on a workspace
without `-p` is ambiguous and errors with a hint listing the members.

## Build a Component

```sh
canon build                     # current directory as a package or workspace
canon build hello.can            # single-file mode
canon build my-workspace        # builds every member
canon build my-workspace -p foo # builds only `foo`
```

In package mode, produces `build/<name>.wasm` and `build/<name>.wit`
next to the package's `src/`. In workspace mode, every member's
artifacts land in the workspace's shared `build/` directory. In
single-file mode, the output goes to `build/<stem>/<stem>.wasm` next to
the file. The component runs on any host that supports WASI Preview 3
and satisfies its imports — `canon run`, `wasmtime serve`, browser
polyfills, edge runtimes, etc.

## Serve as an HTTP Handler

```sh
canon run --addr 127.0.0.1:8080            # current directory
canon run hello.can --addr 127.0.0.1:8080   # single-file mode
```

With `--addr`, the same `run` verb instantiates the component as a
`wasi:http/handler` instead of running it once as a command. The runtime
opens a TCP listener at the given address and dispatches each incoming
HTTP/1.1 request to the guest's `handle` export.

## Inspect Compiler Stages

```sh
canon inspect tokens hello.can   # lexer output
canon inspect ast    hello.can   # parser output (AST debug dump)
canon inspect wat    hello.can   # generated WebAssembly Text
```

One verb, three stages. The `wat` stage is the fastest way to see how
Canon constructs map to wasm — print statements, dispatch, heap
allocation, async lowering — without dragging in the component wrapping
layer. `tokens` and `ast` are diagnostic tools when you want to
understand exactly how the lexer or parser sees your code.

## Check Sort Order and Types

```sh
canon check                     # current package or workspace
canon check hello.can            # single-file mode
canon check my-workspace -p foo # only one member of a workspace
```

Runs the full checker (sort-order rules plus type checking) without
codegen. Fast — useful as an editor lint or pre-commit gate.

## Format

```sh
canon fmt hello.can
canon fmt hello.can --check     # exit 1 if not already formatted
```

## Run Tests

```sh
canon test mymod_test.can
```

Discovers every `() -> TestResult` function in the file and prints a
`[ ok ]` / `[FAIL]` line per test. See the
[testing notes in `CLAUDE.md`](https://github.com/Almaju/canon/blob/main/CLAUDE.md#testing)
for the full conventions.

## Language Server

```sh
canon lsp
```

Speaks LSP over stdio. The Zed extension wires this up automatically;
other editors can point at the same binary.

## All Commands

```sh
canon help
```

## Workflow

There is no `canon new` or project scaffolder yet. For quick
experimentation, drop a `.can` file anywhere and `canon run` it. For
proper projects, create an `canon.toml` next to a `src/main.can` and
run `canon build` / `canon run` from that directory. For multi-file
projects, see [Modules](../tour/modules.md).
