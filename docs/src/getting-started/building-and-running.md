# The canon CLI

Canon commands operate on a **target**, one of:

- a **package directory** (one containing `src/main.can`),
- a **workspace directory** (one whose immediate subdirectories are
  packages), or
- a single **`.can` file** (anonymous single-file package).

When omitted, the target defaults to the current directory. There is
no manifest file: the directory structure is the whole declaration.

## Package Layout

```text
my-app/
  src/
    main.can        # entry point — this file makes the directory a package
    helpers.can
  build/           # compiler output (gitignored)
    my-app.wasm
    my-app.wit
```

The package's name is its directory name, and the artifact is named
after the package, not the entry file.

## Workspace Layout

A workspace is nothing but a directory of packages: any directory that
is not itself a package but whose immediate subdirectories include
packages. Each member builds into its own `build/`.

```text
my-workspace/
  foo/
    src/main.can
    build/foo.wasm
  bar/
    src/main.can
    build/bar.wasm
```

There is no member list to maintain — adding a package subdirectory
adds it to the workspace.

## Run a Program

```sh
canon run                       # current directory as a package
canon run path/to/pkg           # another package
canon run hello.can              # single-file mode
canon run my-workspace -p foo   # one member of a workspace
```

Compiles the target to a WebAssembly component and runs it through the
embedded `wasmtime` runtime. `canon run` on a workspace without `-p` is
ambiguous and errors with a hint listing the members.

## Build a Component

```sh
canon build                     # current directory as a package or workspace
canon build hello.can            # single-file mode
canon build my-workspace        # builds every member
canon build my-workspace -p foo # builds only `foo`
```

Package mode produces `build/<name>.wasm` and `build/<name>.wit` next
to the package's `src/`. Workspace mode builds every member into that
member's own `build/`. Single-file mode writes
`build/<stem>/<stem>.wasm` next to the file. The component runs on any
host that supports WASI Preview 3 and satisfies its imports:
`canon run`, `wasmtime serve`, browser polyfills, edge runtimes.

## Serve as an HTTP Handler

A program whose entry is `(Request) => Response` compiles to a
`wasi:http/service` component, and `canon run` serves it instead of
running it once:

```sh
canon run                                  # serves on 127.0.0.1:8080
canon run --addr 0.0.0.0:9000              # explicit address
canon run hello.can --addr 127.0.0.1:8080  # single-file mode
```

The runtime opens a TCP listener and dispatches each incoming HTTP/1.1
request to the guest's `handle` export. See
[Programs & Modules](../learn/programs-and-modules.md).

## Inspect Compiler Stages

```sh
canon inspect tokens hello.can   # lexer output
canon inspect ast    hello.can   # parser output (AST debug dump)
```

One verb, two stages. `tokens` and `ast` show how the lexer and parser
see your code.

## Check Sort Order and Types

```sh
canon check                     # current package or workspace
canon check hello.can            # single-file mode
canon check my-workspace -p foo # only one member of a workspace
```

Runs the full checker (canonical formatting, sort-order rules, type
checking) without codegen. Fast enough for an editor lint or a
pre-commit gate.

## Format

```sh
canon check --fix hello.can     # fix what's mechanical, then check
```

Formatting is not a separate concern in Canon: a file that isn't in
canonical form is a **compile error** — `canon check`, `build`, `run`,
and `test` all refuse it, pointing at the first line that diverges.
There is no separate formatter command, because a formatting error is
just a compiler error with a mechanical fix: `--fix` rewrites the
loaded sources into canonical form (spacing, call shape, and every
sort-order rule) and then reports whatever it couldn't fix.

## Run Tests

```sh
canon test mymod_test.can
```

Discovers every test in the file -- a result newtype `X = TestResult`
with a nullary `Unit => X` constructor -- and prints a
`[ ok ]` / `[FAIL]` line per test. The process exits `1` when any test
fails, so `canon test` slots straight into CI. See
[Testing](../learn/testing.md) for the conventions.

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
proper projects, create a `src/main.can` and run `canon build` /
`canon run` from that directory. For multi-file projects, see
[Programs & Modules](../learn/programs-and-modules.md).
