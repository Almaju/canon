# Changelog

## 0.2.0

- The `use` keyword was removed from Canon (imports are automatic:
  referencing `Foo` loads `foo.can`); the grammar no longer highlights
  it as a keyword
- Highlight the `bindings` and `package` file-level directives

## 0.1.0

- Initial release
- Syntax highlighting for `.can` files
- Language server integration (`canon lsp`): diagnostics, hover,
  go-to-definition, formatting
- Automatic download of prebuilt `canon` binaries from GitHub releases
