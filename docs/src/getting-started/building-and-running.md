# Building and Running

All `oneway` commands operate on a single `.ow` file (or a module folder).

## Run a Program

```sh
oneway run hello.ow
```

Compiles the file to a WebAssembly component and immediately runs it
through the embedded `wasmtime` runtime. No artifact is left behind in
the source directory.

## Build a Component

```sh
oneway build hello.ow
```

Produces a portable WebAssembly Component at `.oneway/hello/hello.wasm`,
plus a sibling `.wit` describing the component's world (its imports and
exports). The component runs on any host that supports WASI Preview 3 and
satisfies its imports — `oneway run`, `wasmtime serve`, browser
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
oneway check hello.ow
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

There is no `oneway new` or project scaffolder. Single-file programs are
first class — drop a `.ow` file anywhere and `oneway run` it. For
multi-file projects, see [Modules](../tour/modules.md).
