# Building and Running

All `oneway` commands operate on a single `.ow` file (or a module folder).

## Run a Program

```sh
oneway run hello.ow
```

Compiles the file and immediately runs it. The binary is placed in a
temporary `.oneway/` directory next to the source and is run in place.

## Build a Native Binary

```sh
oneway build hello.ow
```

Compiles to a native binary placed in `.oneway/hello/hello` next to the
source file.

## Inspect Generated Rust

```sh
oneway emit hello.ow
```

Prints the Rust source that the transpiler produces. This is the fastest
way to build a mental model of how Oneway constructs map to Rust.

## Show Tokens or AST

```sh
oneway tokens hello.ow
oneway ast    hello.ow
```

Diagnostic tools — useful when you want to understand exactly how the
lexer or parser sees your code.

## Check Sort Order

```sh
oneway check hello.ow
```

Validates the sort-order rules (alphabetical ordering of declarations,
dispatch arms, imports, etc.) without running codegen.

## Format

```sh
oneway fmt hello.ow
```

## All Commands

```sh
oneway help
```

## Workflow

There is no `oneway new` or project scaffolder. Single-file programs are
first class — drop a `.ow` file anywhere and `oneway run` it. For
multi-file projects, see [Modules](../tour/modules.md).
