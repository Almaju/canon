# The canon CLI

Canon commands operate on a **target**: a package directory (one
containing `src/main.can` — or `src/web.can` + `src/server.can` for a
[fullstack app](../reference/web-target.md#fullstack-packages)), a
workspace directory (one whose immediate subdirectories are packages),
or a single `.can` file. When omitted, the target is the current
directory. There is no manifest — the directory structure is the whole
declaration:

```text
my-app/
  src/
    main.can        # entry point — this file makes the directory a package
    helpers.can
  build/           # compiler output (gitignored)
    my-app.wasm
    my-app.wit
```

A workspace is nothing but a directory of packages — no member list;
adding a package subdirectory adds it to the workspace, and each
member builds into its own `build/`.

## Run and Build

```sh
canon run                       # current directory
canon run hello.can             # single-file mode
canon run my-workspace -p foo   # one member of a workspace
canon build                     # compile only: build/<name>.wasm + .wit
```

`canon run` compiles to a WebAssembly component and executes it on the
embedded wasmtime. A program whose entry is `Request => Response` is
**served** instead of run once, as are web apps (the Elm triple) and
fullstack packages — the latter serve frontend and backend from one
process on one address:

```sh
canon run                       # serves on 127.0.0.1:8080
canon run --addr 0.0.0.0:9000   # explicit address
```

The built component runs on any host that supports WASI Preview 3 —
see [Deploying](../reference/deploying.md).

## Check and Format

```sh
canon check                     # types + sort order + formatting, no codegen
canon check --fix               # fix what's mechanical, then check
```

Formatting is not a separate concern: a file that isn't in canonical
form is a **compile error**, and `--fix` is the one way to resolve it —
it rewrites sources into canonical form (spacing, call shape, every
sort-order rule) and reports whatever it couldn't fix. There is no
separate formatter command.

## Test

```sh
canon test mymod_test.can       # one file
canon test tests/               # every *_test.can under a directory
```

Discovers every test by shape and prints a `[ ok ]` / `[FAIL]` line
per test; exits `1` on any failure. See [Testing](../learn/testing.md).

## Everything Else

```sh
canon inspect tokens hello.can  # lexer output
canon inspect ast hello.can     # parser output
canon lsp                       # language server over stdio (used by the editor extensions)
canon help                      # all commands
```

There is no `canon new`: drop a `.can` file anywhere and `canon run`
it, or create a `src/main.can` for a real project.
