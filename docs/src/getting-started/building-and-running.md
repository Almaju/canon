# Building and Running

The compiler is invoked through `just` recipes. The ones you'll actually
use, day to day:

## Run a Program

```sh
just run path/to/file.ow
```

Compiles the file to a native binary placed next to the source, runs it,
prints the output, and removes the binary afterward.

## Run an Example by Name

```sh
just example hello          # runs examples/hello.ow
just example multifile      # runs examples/multifile/main.ow
```

## Run Every Example

```sh
just examples
```

Compiles and runs every file in `examples/`, reporting which passed,
failed, or were skipped (skipped means the example does not yet compile).

## Inspect the Generated Rust

```sh
just emit path/to/file.ow
```

Prints the Rust source that the transpiler produces. This is the best way
to build a mental model of how Oneway constructs map to Rust.

## Show Tokens or AST

```sh
just tokens path/to/file.ow
just ast    path/to/file.ow
```

Both are diagnostic — useful when you want to know exactly how the
lexer/parser sees your code.

## Check Sort Order

```sh
just check path/to/file.ow
```

Validates only the sort-order rules (alphabetical ordering of declarations,
dispatch arms, imports, etc.) without doing the rest of compilation.

## Tests and Linting

```sh
just test            # cargo test the compiler
just fmt             # cargo fmt the compiler source
just clippy          # cargo clippy the compiler source
just clean           # remove build artifacts and compiled examples
```

## Workflow

There is no `oneway new` or project scaffolder. Single-file programs are
first class — drop a `.ow` file anywhere and `just run` it. For multi-file
projects, see [Modules](../tour/modules.md).
