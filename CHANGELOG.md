# Changelog

All notable changes to Canon are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

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
