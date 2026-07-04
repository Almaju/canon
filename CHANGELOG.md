# Changelog

All notable changes to Canon are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Path-carried package identity (2026-07)

The `package` keyword is gone (PACKAGES.md slices 7 + 8a). A vendored
package's identity now lives in its directory name —
`deps/<ns>/<name>@<version>/` — so the files are pure source: no
provenance directive, and version agreement across a package is
structural (a directory has one name) instead of checked. `canon
install` writes the versioned layout, removes any previously vendored
version in the same operation, and records a published package's
dependency list from directory names. Two `@`-versioned siblings, an
unversioned vendor directory, or a malformed version are loader
errors.

Binding files are now recognized by *shape*: a body-less camelCase
declaration in a file directly under the package directory binds to
the WIT interface its path spells (`deps/wasi/random@0.3.0/random.can`
→ `wasi:random/random@0.3.0`), so `canon install` stops emitting the
`bindings` header on generated files. The `bindings` directive remains
solely as the escape hatch for URNs no path can spell (hand-written
`canon:builtins/*` wrappers, one-shot `#fn` renames); deleting the
keyword outright is PACKAGES.md slice 8b, blocked on choosing that
escape hatch's replacement spelling.

### Release channels & toolchains (2026-07)

Releases moved to a **nightly + stable** model that never pushes to `main`
(so it no longer trips the branch ruleset). Every push to `main` publishes a
rolling `nightly` prerelease; stable `vX.Y.Z` releases are cut on demand by the
`promote` workflow.

The CLI manages toolchains with two concepts (rustup's five, collapsed): one
install holds both channels under `~/.canon/toolchains/`, and the `canon` on
`PATH` is a launcher. `canon use nightly` makes the current directory (and
everything below) use nightly — installing it first if needed; run it at `~`
and it's the global default. `canon nightly <cmd>` / `canon stable <cmd>` run
one command with that toolchain, the channel as first word like a dispatch
arm. Resolution: explicit word → nearest `use` ancestor → `stable`. No project
config file, no default/override machinery. `canon upgrade` updates the active
toolchain. The x86_64 macOS binary is cross-built on the arm64 runner (the
Intel `macos-13` runner is gone), and `install.sh` fails with a clear message
when no release exists instead of building a bogus tag.

### V1 milestone (2026-07)

The V1 roadmap (`V1.md`) is complete, with resources/streams for the
CLI world explicitly deferred to V1.1. Highlights:

**HTTP services are standard components.** A free `(Request) ->
Response` function compiles to a `wasi:http/service` component: the
user body is fully compiled (dispatch, helpers, prints), status codes
are runtime values, string bodies ride a real contents stream behind
an async-stackful `handle` lift, and `Request.path()` enables routing.
`canon run` serves HTTP entries on `127.0.0.1:8080` by default.
Flagship example: `examples/notes-api`.

**Language correctness.** Nested constructors no longer corrupt
products; `Float` prints and flows through unions/products; N-variant
dispatch is pinned; `list.map` really applies its lambda (Int and
String elements, cross-type) and `list.get(i)` landed;
`Bool.and/or/not` chains work; `?` short-circuits on `Err` *and*
`None` with string payloads intact; `Int(1)`/`Float(2.5)` explicit
constructors work; method lookup follows newtype alias chains.

**WASI surface.** WIT-informed extern lowering reads the vendored WIT
for every `wasi:*` import: narrow ints (u8–u32) wrap/extend at call
sites, `option<string>`/`list<string>`/record-of-scalars indirect
returns decode into Canon values. New stdlib: `cli/Exit` (real
`wasi:cli/exit`, exit codes propagate), `cli/Args`, `cli/Cwd`,
`time/Unix` (wall clock). The `canon:builtins/cli` bridge is deleted.

**Tooling & docs.** `canon test` exits nonzero on failure with
single-line `[FAIL]` banners; `TESTPLAN.md` maps every construct to
its pinning fixture with ranked holes; new "Serving HTTP" tour
chapter; stale doc paths fixed.

**Editor extensions.** New VS Code extension
(`editors/vscode-canon`): TextMate highlighting plus the `canon lsp`
language server (diagnostics, hover, go-to-definition, formatting),
with automatic download of prebuilt compiler binaries; published to
the VS Code Marketplace by CI and attached to every release as a
`.vsix`. The Zed extension is registry-ready (repository metadata,
v0.4.0). Publishing runbook in `editors/PUBLISHING.md`.

**JSON prelude.** JSON literals need no import — like `Option` and
`Result`, JSON is ambient. A static literal (`{"k":"v"}`) is a
compile-time constant with `String`'s methods, valid in every world
including HTTP handlers; the loader pulls in `canon/std/Json`
automatically when a program uses interpolation, the validating
`Json(...)` constructor, or `.ToJson()`. Interpolation inside an HTTP
handler now fails at build with a clear unsatisfiable-imports error
instead of silently serving an empty body.

**`parallel`/`race` are methods.** The combinators use the ordinary
commutative method-call shape — `a.parallel(b)` / `a.race(b)` — and
the bare `parallel(a, b)` call form (the only bare free-function call
in the language) is now a compile error steering to the method
spelling.

**Docs are compiler-checked prose.** Every ```` ```canon ```` block in
the book was run through the real `canon check`/`canon fmt`: product
constructors use the value-level `*` form everywhere (the comma form
never was part of the language), the JSON tutorial and `notes-api`
example build bodies from JSON literals instead of escape-heavy
strings, the stdlib reference's import paths match the loader
(`fs/`, `http/`, `time/` sub-namespaces), stale HTTP-capability
claims were corrected, and 31 non-canonically-formatted snippets now
match `canon fmt` output exactly.

### Language — Breaking Changes

**Domain-first value model replaces capability system.**

Canon no longer uses a capability-based effect system. Instead, effects emerge from the values you construct and thread. The changes:

- **`main = () -> Unit`** — the entry point takes no parameters and always runs under tokio. No capability declaration needed.
- **`T()` required for zero-data construction** — types with no underlying composition (`Unit`, `True`, `False`, union variants with no payload) must be constructed with `T()` in expression positions. Bare `T` after `.` without `()` is now always a field access.
- **Field access vs construction** — `value.Field` reads a field; `value.Field()` calls a constructor. The `()` unambiguously signals intent to produce.
- **`print` is a built-in** — `string.print` writes to stdout. No `Stdout` parameter. For redirectable output, construct `Fileout` from a `File`.
- **JSON via constructors** — `"[1,2,3]".JsonValue()?.JsonArray()?.length().print` replaces `Json.parse(...)?. asArray()`. The `Json` phantom capability is removed.
- **Filesystem via `File`** — `Path("./f").File()?.read()?.print` replaces `Filesystem.read(Path(...))`. The `Filesystem` capability is removed.
- **HTTP server via `Port`** — `Port(3000).HttpServer(state).get(...).serve()` replaces `HttpServer.router(state)...serve(Port(3000))`. The `router` step is removed; `HttpServer` is now constructed from `Port * S`.
- **HTTP client via `Url`** — `Url("https://...").get()` replaces `HttpClient.get(Url(...))`. No `HttpClient` capability.
- **Clock constructible** — `Clock(Unit()).now()` replaces `Clock.now()`.
- **`Ok(Unit())` not `Ok(Unit)`** — `Unit` in expression position now requires `()` like all zero-data types.

### Standard Library — Breaking Changes
- `json.can`: Removed `Json` type. `parse` renamed to `JsonValue` constructor. `asArray`, `asBool`, `asNull`, `asNumber`, `asObject`, `asString` renamed to `JsonArray`, `JsonBool`, `JsonNull`, `JsonNumber`, `JsonObject`, `JsonString` constructors. `emit` no longer takes a `Json` receiver.
- `filesystem.can`: Removed `Filesystem` type and `read = (Filesystem * Path)`. New `File` type with `File = (Path) -> Result<File, IoError>` and `read = (File) -> Result<String, IoError>`. New `Fileout` type for redirectable output.
- `http-server.can`: Removed `router`. `HttpRouter<S>` renamed to `HttpServer<S>`. New constructor `HttpServer = <S>(Port * S) -> HttpServer<S>`. `serve` no longer takes a `Port`.
- `http-client.can`: Removed `HttpClient` type. `get` now takes `Url` directly.
- `clock.can`: `Clock = Unit` (constructible newtype).

## [0.2.0]

### Language
- Lambdas as first-class values: `(Type) -> Ret { ... }`
- `List<T>` with `map`, `length`, and `first`
- Generic type parameters and trait constraints (`<T: Trait>`)
- Traits with default implementations (`{ impl }`)
- `?` operator for error propagation
- Validated constructors
- Multi-file modules via `use`

### Compiler
- Formatter (`canon fmt`) with `--check` mode for CI
- `canon upgrade` for in-place binary updates
- LSP server (`canon-lsp`) with real-time diagnostics

### Standard Library
- `clock` — `now` returning `Datetime`
- `datetime` — `Datetime` type, `toRfc3339`
- `filesystem` — async `read`
- `http_client` — async `get`
- `http_server` — `router`, `get`, `post`, `serve`
- `json` — generic `parse` via `Deserialize` trait
- `path` — `Path` newtype
- `url` — `Url` with validated constructor

### Tooling
- Tree-sitter grammar and Zed extension with syntax highlighting
- Cross-platform installer (`install.sh`) for macOS and Linux (arm64 + x86_64)
- `just` task runner covering build, run, emit, check, ast, tokens, fmt, examples
- Pre-commit hook via `githooks/`
