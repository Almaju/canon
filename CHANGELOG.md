# Changelog

All notable changes to Oneway are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

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
- Formatter (`oneway fmt`) with `--check` mode for CI
- `oneway upgrade` for in-place binary updates
- LSP server (`oneway-lsp`) with real-time diagnostics

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
