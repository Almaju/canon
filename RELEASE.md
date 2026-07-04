# Release Process

Canon releases on two channels. **No release workflow pushes to `main`** —
`main` has a ruleset requiring the `ci` status check, so a bot push of a
version-bump commit is rejected (`GH013`). Versions come from git tags, and
`Cargo.toml`'s version is stamped into the build in CI (never committed).

## Nightly (automatic)

Every push to `main` (except docs-, editors-, or markdown-only changes) runs
`.github/workflows/nightly.yml`, which cross-builds all platforms and
publishes/updates a rolling **`nightly`** tag as a GitHub *prerelease*. Nothing
to do by hand — merging to `main` is the release.

The nightly version label is `<next-patch>-nightly.<date>.g<sha>`, e.g.
`0.3.1-nightly.20260703.g1a2b3c4`.

## Stable (manual promotion)

When a nightly is worth blessing as stable, promote it:

1. **Actions → promote → Run workflow.**
2. Pick the **bump** (`patch` / `minor` / `major`) and, optionally, the
   commit to promote (`ref`, defaults to the current `nightly`).
3. The workflow computes the next `vX.Y.Z` from the existing `v*` tags, creates
   that tag on the chosen commit, and publishes a normal (non-prerelease)
   release. Because GitHub's `/releases/latest` ignores prereleases, this
   becomes the repo's **Latest** and the target of the stable install channel.

That's it — no local commands, no editing `Cargo.toml`, no pushing to `main`.

## Reusable core

Both entry points call `.github/workflows/release.yml` via `workflow_call`
(inputs: `ref`, `tag`, `version`, `prerelease`, `make_latest`, `move_tag`). A
`workflow_dispatch` trigger on `release.yml` is kept for manual emergency
rebuilds of a specific tag.

## Update the changelog

Changelog upkeep is still manual. When promoting a stable release, move the
`[Unreleased]` section to the new version with today's date and open a fresh
`[Unreleased]` above it:

```markdown
## [0.3.1] — 2026-07-04
```

## Verify the installer

After a stable promotion, confirm both channels download and run:

```sh
curl -fsSL https://raw.githubusercontent.com/almaju/canon/main/install.sh | sh
canon --version                                    # e.g. "canon 0.3.1 (stable)"

CANON_CHANNEL=nightly curl -fsSL https://raw.githubusercontent.com/almaju/canon/main/install.sh | sh
canon --version                                    # e.g. "canon 0.3.2-nightly.… (nightly)"
```

## Version scheme

Canon uses [Semantic Versioning](https://semver.org) loosely while pre-1.0:

| Bump | When |
|---|---|
| Patch `0.x.Y` | Bug fixes, no language changes |
| Minor `0.X.0` | New language features; breaking changes are expected pre-1.0 |
| Major `1.0.0` | First stable release with backward-compatibility guarantees |
