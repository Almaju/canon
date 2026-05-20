# Oneway

Oneway is a new programming language. The reference implementation transpiles to Rust — Oneway inherits Rust's ownership model and zero-cost abstractions, while presenting a much smaller surface area to the programmer.

The guiding rule: wherever ordering is discretionary, the compiler enforces alphabetical order. Components of product types, variants of unions, function declarations, dispatch arms, imports — all alphabetical. Ordering is never a meaningful change.

See [`DESIGN.md`](DESIGN.md) for the language specification.

---

## What It Looks Like

```
Bool = False + True

main = (Stdout) -> Unit {
    List(1, 2, 3)
        .map((Int) -> Int { Int.mul(2) })
        .length()
        .print(Stdout)
}
```

Functions are defined over compositions of types — there is no privileged receiver. There is no `let`, no `if`/`else`, no comments, no local variables. Branching is dispatch on a union. Effects are passed in as capabilities (`Stdout`, `Filesystem`, …). Imports are file-based — `use Foo` imports the type declared in `foo.ow` from the current module folder.

---

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/almaju/oneway/main/install.sh | sh
```

The script downloads a prebuilt `oneway` binary for your platform (macOS arm64/x86_64, Linux arm64/x86_64) and installs it to `~/.oneway/bin/oneway`. Add that directory to your PATH as instructed by the installer.

Pin to a specific version:

```sh
curl -fsSL https://raw.githubusercontent.com/almaju/oneway/main/install.sh | sh -s v0.1.0
```

Update an existing install in place:

```sh
oneway upgrade            # install the latest release
oneway upgrade v0.2.0     # install a specific release
oneway upgrade --check    # only check whether a newer release is available
```

> **Note:** `oneway run` and `oneway build` shell out to `rustc` to compile the generated Rust. Install Rust from [rustup.rs](https://rustup.rs) if you don't already have it.

---

## Usage

```sh
oneway run hello.ow         # compile and run
oneway build hello.ow       # compile to a native binary next to the source
oneway emit hello.ow        # print generated Rust
oneway check hello.ow       # check sort order and types
oneway ast hello.ow         # print the AST
oneway tokens hello.ow      # print lexer tokens
oneway upgrade              # update to the latest release
oneway version
oneway help
```

A first program:

```sh
cat > hello.ow <<'EOF'
main = (Stdout) -> Unit {
    "hello".print(Stdout)
}
EOF
oneway run hello.ow
```

---

## Repository Layout

| Path | Description |
|------|-------------|
| [`src/`](src/) | The `oneway` compiler (lexer, parser, checker, codegen) |
| [`examples/`](examples/) | Example `.ow` programs |
| [`editors/`](editors/) | Tree-sitter grammar and Zed extension |
| [`DESIGN.md`](DESIGN.md) | Language specification |

---

## Building from Source

For contributors. End users should install the prebuilt binary via the script above.

```sh
just build                      # build the compiler
just run examples/hello.ow      # compile and run an example
just example list               # run examples/list.ow (or examples/list/main.ow)
just examples                   # run every example
just emit examples/hello.ow     # print generated Rust
just ast  examples/hello.ow     # print the AST
just test                       # run the compiler test suite
```

---

## Releases

Tagging a commit with `vX.Y.Z` triggers `.github/workflows/release.yml`, which cross-builds `oneway` for macOS (arm64, x86_64) and Linux (arm64, x86_64), uploads the tarballs and SHA256 checksums to the GitHub release, and makes the new version installable via the install script.

```sh
# Bump version in Cargo.toml, then:
git tag v0.1.0
git push origin v0.1.0
```

---

## Status

Experimental. Phase 18 of the v2 rewrite — lambdas and `List<T>` with `map` / `length` / `first` are in. The compiler is far from complete; the design is the artifact.
