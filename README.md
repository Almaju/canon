# Canon

Canon is a new programming language. The reference compiler emits **WebAssembly components** directly — every Canon program is a standard [WASI Preview 3](https://github.com/WebAssembly/WASI) component, runnable on any compliant host and automatically bound to the component-model ecosystem. The language itself presents a deliberately small surface area.

The guiding rule: wherever ordering is discretionary, the compiler enforces alphabetical order. Components of product types, variants of unions, function declarations, dispatch arms, imports — all alphabetical. Ordering is never a meaningful change.

See the [language specification](docs/src/spec/) for the full semantics.

---

## On Authorship and AI

Canon's design is human work. The language — its philosophy, its type
algebra, the "wherever ordering is discretionary, enforce it" rule, the
types-only doctrine, capabilities-as-values, the whole small opinionated
surface — comes from one person's head. Those ideas are not
AI-generated.

AI was used, under supervision, as an implementation aid for the
*compiler*: a tool for turning already-decided designs into Rust, the
way one might use any other power tool. Every design decision was made,
reviewed, and owned by a human. The core is handmade; the AI helped pour
the concrete, not draw the blueprint.

I mention this deliberately. A language dismissed as "AI slop" would be
judged on its provenance rather than its ideas — and the ideas here are
genuinely mine. Judge them on their merits.

---

## What It Looks Like

The only names are type names. Types compose with `+` (union) and `*`
(product) — `Bool = False + True`, `User = Birthday * Username`. A
declaration is an arrow between types (`=>`); a program is a pipeline that
pushes a value through them (`->`):

```
Unit => Program {
    List(1 * 2 * 3)
        -> Mapped((Int) => Int { Int -> Product(2) })
        -> Length
        -> Print
}
```

There is no `main` keyword — the CLI entry is selected by its shape: the
anonymous `Unit => Program` function. An HTTP service is just as anonymous
— a `Request => Response` function — and the compiler emits a standard
`wasi:http/service` component from it:

```
Greeting = Body

Unit => Greeting {
    "hello" -> Body
}

Request => Response {
    200 -> Status -> Response(Greeting() * Headers())
}
```

Functions are defined over compositions of types — there is no privileged receiver. There is no `let`, no `if`/`else`, no comments, no local variables. Branching is dispatch on a union. Imports are automatic and file-based — there is no `use` statement: referencing `Foo` loads the `foo.can` that declares it, whether that's a sibling file, a vendored dependency under `deps/`, or the bundled standard library.

---

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/almaju/canon/main/install.sh | sh
```

The script downloads a prebuilt `canon` binary for your platform (macOS arm64/x86_64, Linux arm64/x86_64) and installs it to `~/.canon/bin/canon`. Add that directory to your PATH as instructed by the installer.

Pin to a specific version:

```sh
curl -fsSL https://raw.githubusercontent.com/almaju/canon/main/install.sh | sh -s v0.1.0
```

### Toolchains: stable and nightly

One install holds both channels, and switching is a single word — no config
file in your project, no separate "default" and "override" concepts:

```sh
canon use nightly            # this directory (and below) now uses nightly —
                             # installs it first if needed
cd ~ && canon use nightly    # run it in your home directory and it's global
canon use                    # show the active toolchain and why

canon nightly run app.can    # one command with nightly, like a dispatch arm
canon stable test suite.can  # one command with stable
```

A bare `canon` resolves: explicit channel word → nearest `canon use` ancestor
→ `stable`. (Nightly is rebuilt on every push to `main`.)

Update the active toolchain in place, or pin a version:

```sh
canon upgrade            # update the active toolchain to its channel's latest
canon upgrade --check    # only check whether a newer stable release exists
curl -fsSL https://raw.githubusercontent.com/almaju/canon/main/install.sh | sh -s v0.2.0
```

> **Note:** no external toolchain is required — the compiler produces the final `.wasm` in-process and `canon run` executes it on the embedded wasmtime runtime.

---

## Usage

```sh
canon run hello.can              # compile and run
canon run --addr 127.0.0.1:8080 # serve an HTTP-entry program (default: 127.0.0.1:8080)
canon build hello.can            # compile to a WASM component (.wasm)
canon check hello.can            # check sort order and types
canon test hello_test.can        # run `() => TestResult` functions
canon fmt hello.can              # format source (use --check to verify only)
canon inspect wat hello.can      # print generated WAT
canon inspect ast hello.can      # print the parsed AST
canon inspect tokens hello.can   # print lexer tokens
canon bindgen file.wit          # generate Canon bindings from WIT
canon upgrade                   # update to the latest release
canon --version
canon help
```

A first program:

```sh
cat > hello.can <<'EOF'
Unit => Program {
    "hello" -> Print
}
EOF
canon run hello.can
```

---

## Repository Layout

| Path | Description |
|------|-------------|
| [`src/`](src/) | The `canon` compiler (lexer, parser, checker, codegen) |
| [`packages/canon/std/`](packages/canon/std/) | Standard library (Canon wrappers over generated WASI bindings) |
| [`docs/`](docs/) | Documentation site (mdBook) |
| [`examples/`](examples/) | Example `.can` programs |
| [`tests/`](tests/) | Integration tests |
| [`editors/`](editors/) | Tree-sitter grammar, Zed extension, VS Code extension |

---

## Building from Source

For contributors. End users should install the prebuilt binary via the script above.

```sh
just build        # build a debug binary
just install      # build and install the release binary to ~/.cargo/bin/canon
just test         # run the test suite
just examples     # compile and run every example
just example hello  # run a single example by name
just ci           # run all CI checks locally
```

---

## Releases

Two channels, both driven by GitHub Actions — no manual version bumps, and
nothing is ever pushed to `main`:

- **nightly** — every push to `main` publishes a rolling `nightly` prerelease
  (the `nightly` workflow). Install it with `CANON_CHANNEL=nightly`.
- **stable** — cut on demand: `Actions → promote → Run workflow` turns the
  current nightly into the next `vX.Y.Z` release (pick a patch/minor/major
  bump). This becomes the default install channel.

Both call the reusable `release.yml`, which cross-builds for macOS and Linux
and publishes the tarballs.

---

## Status

Experimental: programs the checker accepts run correctly; HTTP handlers compile to standard `wasi:http/service` components (see [`examples/notes-api`](examples/notes-api)); browser frontends compile from the `init`/`update`/`view` triple (see [the web target](docs/src/reference/web-target.md) and [`examples/todo-fullstack`](examples/todo-fullstack) — one language, both sides of the stack, shared types); the stdlib rides real `wasi:cli` / `wasi:clocks` / `wasi:random` interfaces; `canon test` reports honestly.
