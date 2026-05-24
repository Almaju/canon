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
oneway run hello.ow              # compile and run
oneway run --addr 127.0.0.1:8080 # serve as a wasi:http/handler
oneway build hello.ow            # compile to a WASM component (.wasm)
oneway check hello.ow            # check sort order and types
oneway test hello_test.ow        # run `() -> TestResult` functions
oneway fmt hello.ow              # format source (use --check to verify only)
oneway inspect wat hello.ow      # print generated WAT
oneway inspect ast hello.ow      # print the parsed AST
oneway inspect tokens hello.ow   # print lexer tokens
oneway bindgen file.wit          # generate Oneway bindings from WIT
oneway upgrade                   # update to the latest release
oneway --version
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
| [`std/`](std/) | Standard library (`.ow` interfaces + Rust FFI) |
| [`docs/`](docs/) | Documentation site (mdBook) |
| [`examples/`](examples/) | Example `.ow` programs |
| [`tests/`](tests/) | Integration tests |
| [`editors/`](editors/) | Tree-sitter grammar and Zed extension |
| [`DESIGN.md`](DESIGN.md) | Language specification |

---

## Building from Source

For contributors. End users should install the prebuilt binary via the script above.

```sh
just build        # build a debug binary
just install      # build and install the release binary to ~/.cargo/bin/oneway
just test         # run the test suite
just examples     # compile and run every example
just example hello  # run a single example by name
just ci           # run all CI checks locally
```

---

## Releases

Releases are fully automated via GitHub Actions. To cut a release, trigger the **bump** workflow from the GitHub UI (`Actions → bump → Run workflow`) and enter the new version (e.g. `0.4.0`).

The workflow:
1. Updates `Cargo.toml` and `Cargo.lock`
2. Commits and tags `vX.Y.Z`
3. Pushes — which triggers `release.yml`, which cross-builds for macOS and Linux and publishes the tarballs to the GitHub release

---

## Status

Experimental. Phase 18 of the v2 rewrite — lambdas and `List<T>` with `map` / `length` / `first` are in. The compiler is far from complete; the design is the artifact.
