# Canon Editor Support

Both extensions bundle syntax highlighting **and** the language server in
one install. They auto-resolve the `canon` binary (PATH first, then a
GitHub release download) and start it as `canon lsp` — there is no
separate LSP binary to install. You get:

- Lex errors (invalid characters, attempted comments)
- Parse errors
- Sort-order violations (unsorted fields, functions, imports, …)
- Type-check errors
- Hover, go-to-definition, and formatting via `canon fmt`

For the one-time steps that put these on the marketplaces, see
[PUBLISHING.md](PUBLISHING.md).

## VS Code

**Marketplace** (once published — see PUBLISHING.md): search for
**Canon** in the Extensions view, or:

```sh
code --install-extension almaju.canon-lang
```

**From a GitHub release**: every release ships a `canon-lang-<version>.vsix`
asset. Download it and run:

```sh
code --install-extension canon-lang-<version>.vsix
```

**From source**:

```sh
just build-vscode-extension
code --install-extension editors/vscode-canon/canon-lang-<version>.vsix
```

If `canon` isn't on your PATH, the extension offers to download a prebuilt
binary from GitHub releases on first activation (also available as the
**Canon: Download Language Server** command).

## Zed

**Extension registry** (once published — see PUBLISHING.md): open
`zed: extensions` and search for **Canon**.

**As a dev extension** (works today, no registry needed):

1. Install the `canon` compiler so the extension can find it on `PATH`
   (optional — the extension can also download it from GitHub releases):
   ```sh
   just install      # builds and installs ~/.cargo/bin/canon
   ```
2. In Zed, open the command palette and run **`zed: install dev extension`**.
3. Pick the `editors/zed-canon` directory.
4. Open any `.can` file.

## Rebuilding the extension artifacts

The Zed grammar and extension WASMs are committed so dev-extension users
don't need a local toolchain. If you change `tree-sitter-canon/grammar.js`
or `zed-canon/src/lib.rs`, rebuild them with:

```sh
just build-extension
```

This task requires Docker (for `tree-sitter build --wasm`) and the
`wasm32-wasip1` Rust target. Commit the updated `*.wasm` files alongside
the source changes. If you change the grammar, also update the `commit`
hash under `[grammars.canon]` in `zed-canon/extension.toml` once the
grammar change has landed on `main` — the Zed registry builds the grammar
from that exact commit.

The VS Code extension is packaged with:

```sh
just build-vscode-extension
```

which only needs Node 20+ (`npm` fetches `vsce` and `esbuild`).
