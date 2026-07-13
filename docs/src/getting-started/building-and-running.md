# Building and Running

Canon commands operate on a **target**, one of:

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

`members = ["*"]` means every immediate subdirectory containing an
`canon.toml`. Explicit lists (`members = ["foo", "bar"]`) work too.
Nested workspaces are not allowed.

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
to the package's `src/`. Workspace mode puts every member's artifacts
in the workspace's shared `build/`. Single-file mode writes
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
[Serving HTTP](../guide.md#serving-http).

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
[Testing](../guide.md#testing) for the conventions.

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
projects, see [Modules](../guide.md#modules).
