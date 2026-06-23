# Release Process

Steps to cut a new release of Canon. All commands run from the repo root.

## Checklist

### 1. Verify CI is green

Make sure the latest commit on `main` passes all CI checks before tagging.

### 2. Bump the version

Edit `Cargo.toml` and update the `version` field, then rebuild to sync the
lock file:

```sh
# Edit version in Cargo.toml, then:
cargo build
```

### 3. Update the changelog

Move the `[Unreleased]` section to the new version with today's date:

```markdown
## [0.3.0] — 2026-06-01
```

Add an empty `[Unreleased]` section above it for the next cycle.

### 4. Commit

```sh
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "release: v0.3.0"
git push origin main
```

### 5. Tag and push

```sh
git tag v0.3.0
git push origin v0.3.0
```

Pushing the tag triggers `release.yml`, which cross-builds binaries for all
platforms and creates a draft GitHub release automatically.

### 6. Publish the release

Go to the [GitHub releases page](https://github.com/almaju/canon/releases),
review the auto-generated notes, and click **Publish release**.

### 7. Verify the installer

Run the install script on a clean machine (or a fresh shell) to confirm the
new version is downloadable and works:

```sh
curl -fsSL https://raw.githubusercontent.com/almaju/canon/main/install.sh | sh
canon --version
```

## Version scheme

Canon uses [Semantic Versioning](https://semver.org) loosely while pre-1.0:

| Bump | When |
|---|---|
| Patch `0.x.Y` | Bug fixes, no language changes |
| Minor `0.X.0` | New language features; breaking changes are expected pre-1.0 |
| Major `1.0.0` | First stable release with backward-compatibility guarantees |
