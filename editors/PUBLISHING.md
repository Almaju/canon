# Publishing the editor extensions

Everything below is a **one-time setup** per registry. After that,
publishing is automatic (VS Code) or a small version-bump PR (Zed).

## VS Code Marketplace + Open VSX (automated)

The `publish-vscode-extension` workflow runs on every push to `main` that
touches `editors/vscode-canon/` (and on manual dispatch). It packages the
`.vsix`, uploads it as a build artifact, and publishes to each registry
**only when the version in `package.json` is new there**. Release builds
also attach the `.vsix` to every GitHub release, so users can install
manually even before the marketplace setup is done.

### One-time: VS Code Marketplace

1. Sign in at <https://marketplace.visualstudio.com/manage> with a
   Microsoft account and create a publisher with ID **`almaju`**
   (must match `"publisher"` in `editors/vscode-canon/package.json` —
   if you pick a different ID, update `package.json` and the
   `vsce show almaju.canon-lang` / `open-vsx.org/api/almaju/canon-lang`
   lookups in `.github/workflows/publish-vscode-extension.yml`).
2. Create a Personal Access Token at `https://dev.azure.com/<your-org>`
   → User settings → Personal access tokens → New token, with
   **Organization: All accessible organizations** and scope
   **Marketplace → Manage**.
3. Add it as the `VSCE_PAT` repository secret
   (GitHub → Settings → Secrets and variables → Actions).

### One-time: Open VSX (used by VSCodium, Gitpod, code-server, …)

1. Sign in at <https://open-vsx.org> with GitHub and sign the publisher
   agreement (profile → Settings → Publisher Agreement).
2. Create an access token (Settings → Access Tokens).
3. Create the namespace once:
   ```sh
   npx ovsx create-namespace almaju -p <token>
   ```
4. Add the token as the `OVSX_PAT` repository secret.

### Releasing a new extension version

Bump `"version"` in `editors/vscode-canon/package.json`, note it in
`editors/vscode-canon/CHANGELOG.md`, and merge to `main`. The workflow
does the rest. (Pushes that touch the extension without bumping the
version just re-package the artifact and skip publishing.)

## Zed extension registry

Zed extensions are distributed through the
[zed-industries/extensions](https://github.com/zed-industries/extensions)
registry, which references this repo as a git submodule — the extension
stays developed here.

### One-time: initial submission

1. Fork and clone `zed-industries/extensions`.
2. Add this repo as a submodule and register it (the extension lives in a
   subdirectory, hence `path`):
   ```sh
   git submodule add https://github.com/Almaju/canon.git extensions/canon
   ```
   Then add to `extensions.toml`:
   ```toml
   [canon]
   submodule = "extensions/canon"
   path = "editors/zed-canon"
   version = "0.4.0"
   ```
3. Run `pnpm sort-extensions` so the tables stay alphabetical (fitting),
   commit, and open a PR against `zed-industries/extensions`.

The registry builds `zed-canon/src/lib.rs` and the tree-sitter grammar
from source — the committed `extension.wasm` / `grammars/canon.wasm` are
only for local dev installs. The grammar is fetched from the repo/commit
pinned under `[grammars.canon]` in `extension.toml`, so that commit must
exist on `main`.

### Releasing a new extension version

1. Bump `version` in both `editors/zed-canon/extension.toml` and
   `editors/zed-canon/Cargo.toml`, land it on `main`.
2. In your `zed-industries/extensions` fork: update the `extensions/canon`
   submodule to the new commit, bump `version` in `extensions.toml` to
   match, and open a PR.
