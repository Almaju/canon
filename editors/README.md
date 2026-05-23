# Oneway Editor Support

## Zed

Install the dev extension and you get syntax highlighting **and** the
language server in one step. The extension auto-resolves the `oneway`
binary (PATH first, then a GitHub release download) and starts it as
`oneway lsp` — there is no separate LSP binary to install.

### Steps

1. Install the `oneway` compiler so the extension can find it on `PATH`:
   ```sh
   just install      # builds and installs ~/.cargo/bin/oneway
   ```
2. In Zed, open the command palette and run **`zed: install dev extension`**.
3. Pick the `editors/zed-oneway` directory.
4. Open any `.ow` file — you should see syntax highlighting plus real-time
   diagnostics for:
   - Lex errors (invalid characters, attempted comments)
   - Parse errors
   - Sort-order violations (unsorted fields, functions, imports, …)
   - Type-check errors

Hover, go-to-definition, and format-on-save are also wired up.

### Rebuilding the extension WASMs

The grammar and extension WASMs are committed so Zed users don't need a
local toolchain. If you change `tree-sitter-oneway/grammar.js` or
`zed-oneway/src/lib.rs`, rebuild them with:

```sh
just build-extension
```

This task requires Docker (for `tree-sitter build --wasm`) and the
`wasm32-wasip1` Rust target. Commit the updated `*.wasm` files alongside
the source changes.
