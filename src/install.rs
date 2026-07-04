//! `canon install` — materialize external bindings declared in
//! `[imports]`.
//!
//! For each `[imports]` entry of type `.wit`, this generates one Canon
//! file per interface within the WIT package, writing them under the
//! project's `bindgen/` directory at `<namespace>/<package>/<iface>.can`.
//! The output is the same Canon source `canon bindgen` produces, only
//! relocated: `canon bindgen` was designed for one-shot point-at-a-WIT
//! generation; `canon install` is the manifest-driven flow a user will
//! actually run from inside a project.
//!
//! Wasm-component entries (`*.wasm` bundled deps) are recorded as
//! deferred — the composition pipeline that satisfies their imports
//! lands in a later slice.
//!
//! The manifest key (`"wasi"`, `"wasi/random"`, …) acts as a *prefix*
//! guard: every emitted file's path (with kebab→snake normalization,
//! and the bindgen's internal `src/` segment stripped) must start with
//! the key. A key of `"wasi"` matches `wasi/cli/stdout.can`,
//! `wasi/clocks/monotonic_clock.can`, etc.; a key of `"wasi/random"`
//! matches only files under `wasi/random/`. Mismatches surface at install
//! time, not at the eventual `use` site, so the error names both the
//! key and the file that failed to match.
//!
//! `bindgen/` is the conventional output directory; it's expected to be
//! gitignored in user projects (the binding files are derivable from
//! the manifest plus the WIT sources, same way `target/` is derivable
//! from Cargo manifests). Compiler-internal packages may commit their
//! `bindgen/` so `cargo build` works on a fresh clone.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::bindgen;
use crate::manifest::{self, ImportSource, Manifest};

/// Name of the sidecar file `canon install` writes alongside the
/// generated bindings. Maps each emitted `.can` file's `bindgen/`-relative
/// path to the WIT interface URN it was generated from.
///
/// The loader consults this file to patch `extern Wasm` declarations
/// that omit the URN string — the bindgen emits them in that bare form
/// so the source file doesn't repeat the URN per function. With the
/// index, the loader can reconstruct `"<urn>#<fn-kebab>"` for each
/// function it sees in a bindgen-originated module.
///
/// Format is a TOML subset matching `canon.toml`: one `"<rel>" =
/// "<urn>"` line per file, in alphabetical order. The first two lines
/// are a fixed comment header so anyone opening the file knows it's a
/// derived artifact.
pub const INSTALL_INDEX_FILENAME: &str = "_install.toml";

/// The parsed contents of `bindgen/_install.toml`.
///
/// `entries` keys are `.can` file paths relative to the `bindgen/`
/// directory (e.g. `"wasi/clocks/monotonic_clock.can"`); values are
/// WIT interface URNs of the form `"<ns>:<pkg>/<iface>@<version>"`
/// (without a trailing `#<fn>`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallIndex {
    pub entries: BTreeMap<String, String>,
}

impl InstallIndex {
    /// Look up the URN for a `bindgen/`-relative path.
    pub fn urn_for(&self, rel_path: &str) -> Option<&str> {
        self.entries.get(rel_path).map(String::as_str)
    }
}

/// Parse an `_install.toml` file. The format is intentionally a strict
/// subset of TOML: comment lines starting with `#`, blank lines, and
/// `"key" = "value"` pairs. Anything else is an error.
pub fn parse_install_index(source: &str) -> Result<InstallIndex, String> {
    let mut entries = BTreeMap::new();
    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let eq = line.find('=').ok_or_else(|| {
            format!("{INSTALL_INDEX_FILENAME}:{line_no}: expected `key = value`, got `{line}`")
        })?;
        let key = unquote(line[..eq].trim())
            .map_err(|m| format!("{INSTALL_INDEX_FILENAME}:{line_no}: key: {m}"))?;
        let value = unquote(line[eq + 1..].trim())
            .map_err(|m| format!("{INSTALL_INDEX_FILENAME}:{line_no}: value: {m}"))?;
        if entries.contains_key(&key) {
            return Err(format!(
                "{INSTALL_INDEX_FILENAME}:{line_no}: duplicate entry `{key}`"
            ));
        }
        entries.insert(key, value);
    }
    Ok(InstallIndex { entries })
}

fn unquote(raw: &str) -> Result<String, String> {
    if raw.len() < 2 || !raw.starts_with('"') || !raw.ends_with('"') {
        return Err(format!("expected quoted string, got `{raw}`"));
    }
    let inner = &raw[1..raw.len() - 1];
    if inner.contains('"') || inner.contains('\\') {
        return Err("embedded escapes / quotes not supported".to_string());
    }
    Ok(inner.to_string())
}

/// Top-level install error. Wraps every failure mode (manifest parse,
/// missing WIT, bindgen failure, IO) in one type so the CLI surface has
/// a single error to print.
#[derive(Debug)]
pub struct InstallError(pub String);

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InstallError {}

/// Outcome of a successful install.
#[derive(Debug)]
pub struct InstallOutcome {
    /// Project-relative paths of every binding file that was written.
    /// Sorted alphabetically.
    pub written: Vec<PathBuf>,
    /// Human-readable lines describing items the install couldn't yet
    /// realize: wasm-component entries (pending composition), interfaces
    /// inside a WIT whose shape the bindgen skips (resources, etc.), and
    /// any other "we noticed this but did nothing" cases. Surfaced to
    /// stderr by the CLI.
    pub skipped: Vec<String>,
}

/// Read `<project_root>/canon.toml` and install every entry in
/// `[imports]`. Returns the list of files written and any deferred items.
pub fn install(project_root: &Path) -> Result<InstallOutcome, InstallError> {
    let manifest_path = project_root.join("canon.toml");
    let source = fs::read_to_string(&manifest_path)
        .map_err(|e| InstallError(format!("could not read `{}`: {e}", manifest_path.display())))?;
    let manifest = manifest::parse(&source).map_err(|e| InstallError(e.to_string()))?;
    install_from_manifest(project_root, &manifest)
}

/// Walk up from `start` looking for the nearest directory containing
/// `canon.toml`. Returns `None` if the walk reaches the filesystem
/// root without finding one. `start` may be a file or a directory.
///
/// Used by `ensure_installed` to anchor the staleness check against
/// the manifest; the loader and LSP have their own analogous walks
/// for the directories they care about (the duplication is small and
/// stable enough that consolidating it isn't yet worth the indirection).
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cur: PathBuf = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        if cur.join("canon.toml").is_file() {
            return Some(cur);
        }
        cur = cur.parent()?.to_path_buf();
    }
}

/// Result of an `ensure_installed` call.
#[derive(Debug)]
pub enum EnsureOutcome {
    /// No project root found, or the manifest has no `[imports]`
    /// entries. Nothing was done; the binding tree (if any) is left as-is.
    NoProject,
    /// `bindgen/` was already in sync with the manifest and the WIT
    /// sources — no install needed. This is the steady-state path,
    /// so it stays silent.
    UpToDate,
    /// An install was run because the binding tree was missing or
    /// stale. The wrapped `InstallOutcome` lists what was written so
    /// the caller can surface it (e.g. on stderr).
    Installed(InstallOutcome),
}

/// Run `install` on the project containing `start_path` if any of its
/// `[imports]` entries appear out-of-date relative to the materialized
/// `bindgen/_install.toml` index. This is the auto-installer hook the
/// CLI calls from `canon run` / `canon check` / `canon build` /
/// `canon test`, so users don't have to remember a separate step.
///
/// Staleness rules (any of):
///   * `bindgen/_install.toml` doesn't exist (never installed).
///   * `canon.toml`'s mtime is newer than the index (manifest changed).
///   * Any WIT source declared in `[imports]` has an mtime newer than
///     the index. For directory sources, we take the max mtime over
///     every `.wit` file inside.
///
/// Up-to-date short-circuits fast — no parse beyond the manifest, no
/// bindgen, no IO into `bindgen/`. The cost on the steady-state path is
/// one `read_to_string` of the manifest plus a `metadata()` per
/// imported source.
pub fn ensure_installed(start_path: &Path) -> Result<EnsureOutcome, InstallError> {
    let project_root = match find_project_root(start_path) {
        Some(p) => p,
        None => return Ok(EnsureOutcome::NoProject),
    };

    let manifest_path = project_root.join("canon.toml");
    let manifest_src = fs::read_to_string(&manifest_path)
        .map_err(|e| InstallError(format!("could not read `{}`: {e}", manifest_path.display())))?;
    let manifest = manifest::parse(&manifest_src).map_err(|e| InstallError(e.to_string()))?;

    if manifest.imports.is_empty() {
        return Ok(EnsureOutcome::NoProject);
    }

    if !needs_install(&project_root, &manifest, &manifest_path) {
        return Ok(EnsureOutcome::UpToDate);
    }

    let outcome = install_from_manifest(&project_root, &manifest)?;
    Ok(EnsureOutcome::Installed(outcome))
}

/// Decide whether the materialized bindings are stale relative to the
/// manifest and the WIT sources. Returns `true` when an install should
/// be run.
fn needs_install(project_root: &Path, manifest: &Manifest, manifest_path: &Path) -> bool {
    let bindgen_root = project_root.join("bindgen");
    let index_path = bindgen_root.join(INSTALL_INDEX_FILENAME);

    // No index file yet — install has never been run, or the user
    // deleted `bindgen/`.
    let Ok(index_meta) = fs::metadata(&index_path) else {
        return true;
    };

    // Pre-slice-8 layout: index keys without a versioned package
    // directory (`wasi/clocks/…` instead of `wasi/clocks@0.3.0/…`).
    // The loader only resolves the versioned layout, so an old tree
    // must be regenerated regardless of mtimes.
    if let Ok(src) = fs::read_to_string(&index_path) {
        if let Ok(index) = parse_install_index(&src) {
            let unversioned = |key: &str| {
                key.split('/')
                    .rev()
                    .skip(1) // the file segment carries no version
                    .all(|seg| !seg.contains('@'))
            };
            if index.entries.keys().any(|k| unversioned(k)) {
                return true;
            }
        }
    }
    let Ok(index_mtime) = index_meta.modified() else {
        // Filesystem doesn't expose mtime; conservatively reinstall.
        return true;
    };

    // Manifest changed since the last install.
    if mtime_newer_than(manifest_path, index_mtime) {
        return true;
    }

    // Any imported WIT source newer than the index.
    for source in manifest.imports.values() {
        let ImportSource::Wit(rel) = source else {
            continue;
        };
        let abs = project_root.join(rel);
        if max_wit_mtime_newer_than(&abs, index_mtime) {
            return true;
        }
    }

    false
}

/// True if `path`'s mtime is strictly newer than `cutoff`. Returns
/// false on any IO failure — we prefer false-negatives (skip an
/// install we should have done) only at the cost of slightly stale
/// errors, never false-positives (reinstall unnecessarily, blocking
/// the user).
fn mtime_newer_than(path: &Path, cutoff: SystemTime) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t > cutoff)
}

/// True if any WIT source under `path` (file or directory) has an
/// mtime newer than `cutoff`.
fn max_wit_mtime_newer_than(path: &Path, cutoff: SystemTime) -> bool {
    if path.is_file() {
        return mtime_newer_than(path, cutoff);
    }
    if path.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|s| s.to_str()) == Some("wit")
                    && mtime_newer_than(&p, cutoff)
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Same as [`install`], but with a pre-parsed manifest. Useful for tests
/// and for callers (e.g. a future `canon build`) that already parsed the
/// manifest for their own reasons.
pub fn install_from_manifest(
    project_root: &Path,
    manifest: &Manifest,
) -> Result<InstallOutcome, InstallError> {
    let bindgen_root = project_root.join("bindgen");
    let mut written = Vec::new();
    let mut skipped = Vec::new();
    let mut index = InstallIndex::default();

    for (import_key, source) in &manifest.imports {
        match source {
            ImportSource::Wit(rel_path) => {
                let abs_wit = project_root.join(rel_path);
                let entry_result = install_wit_entry(import_key, &abs_wit, &bindgen_root)?;
                written.extend(entry_result.written);
                skipped.extend(entry_result.skipped);
                for (rel, urn) in entry_result.index_entries {
                    index.entries.insert(rel, urn);
                }
            }
            ImportSource::Wasm(rel_path) => {
                skipped.push(format!(
                    "import `{}` from `{}`: bundled wasm components are not yet supported by `canon install`",
                    import_key, rel_path
                ));
            }
        }
    }

    written.sort();

    // Write the install index alongside the generated bindings, but only
    // if anything actually got installed. Skipping the file when there
    // are no entries keeps zero-import projects from having a stray
    // `bindgen/_install.toml`.
    if !index.entries.is_empty() {
        let index_path = bindgen_root.join(INSTALL_INDEX_FILENAME);
        write_install_index(&index_path, &index)?;
        written.push(index_path);
        written.sort();
    }

    Ok(InstallOutcome { written, skipped })
}

/// Result of installing a single `[imports]` entry. Internal helper —
/// the public outcome flattens these.
struct EntryResult {
    written: Vec<PathBuf>,
    skipped: Vec<String>,
    /// One entry per generated `.can` file: `(rel_path, urn)`. Aggregated
    /// across entries to produce the install index.
    index_entries: Vec<(String, String)>,
}

/// Serialize an `InstallIndex` to disk in the canonical `_install.toml`
/// format. Entries are written alphabetically (BTreeMap iteration order)
/// to keep the file diff-stable across re-runs.
fn write_install_index(path: &Path, index: &InstallIndex) -> Result<(), InstallError> {
    let mut out = String::new();
    out.push_str("# Generated by `canon install`. Do not edit.\n");
    out.push_str(
        "# Each entry maps a `bindgen/`-relative `.can` file to the WIT interface URN it was generated from.\n\n",
    );
    for (rel, urn) in &index.entries {
        out.push_str(&format!("{:?} = {:?}\n", rel, urn));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| InstallError(format!("could not create `{}`: {e}", parent.display())))?;
    }
    fs::write(path, out.as_bytes())
        .map_err(|e| InstallError(format!("could not write `{}`: {e}", path.display())))?;
    Ok(())
}

/// Install a single WIT entry: parse the WIT (file or directory),
/// validate every emitted interface's path against the manifest-key
/// prefix, and write the generated source under `<bindgen_root>/`.
///
/// The bindgen emits the vendored-package layout directly
/// (`<ns>/<pkg>@<version>/<iface>.can`, see PACKAGES.md): the
/// directory name carries the pin and the loader derives each
/// binding's URN from the path — the files carry no directive.
fn install_wit_entry(
    import_key: &str,
    wit_path: &Path,
    bindgen_root: &Path,
) -> Result<EntryResult, InstallError> {
    if !wit_path.exists() {
        return Err(InstallError(format!(
            "import `{}` references `{}`, which does not exist",
            import_key,
            wit_path.display()
        )));
    }

    let emitted = bindgen::generate_from_path(wit_path).map_err(|e| {
        InstallError(format!(
            "import `{}`: bindgen failed for `{}`: {e}",
            import_key,
            wit_path.display()
        ))
    })?;
    if emitted.is_empty() {
        return Err(InstallError(format!(
            "import `{}`: `{}` produced no interfaces",
            import_key,
            wit_path.display()
        )));
    }

    let key_prefix = format!("{}/", import_key);

    let mut written = Vec::new();
    let mut skipped = Vec::new();
    let mut index_entries = Vec::new();
    for file in emitted {
        // Skip interfaces the bindgen couldn't represent (resources,
        // async-only fns, …) — mirrors the `canon bindgen` CLI behavior.
        if has_no_decls(&file.content) {
            skipped.extend(file.skipped);
            continue;
        }

        let rel = file.relative_path.clone();

        // Prefix guard: every emitted file's path must sit under the
        // manifest key. This is what makes the manifest key meaningful
        // — a `"wasi"` import can install all WASI interfaces, but a
        // `"wasi/random"` import can't accidentally pull in `wasi/cli`.
        // The key is written without versions (`"wasi/clocks"`), so the
        // comparison strips the `@<version>` suffixes the emitted path
        // carries.
        if !strip_version_suffixes(&rel).starts_with(&key_prefix) {
            return Err(InstallError(format!(
                "import `{}`: WIT at `{}` produced interface at `{}`, which is not under `{}/`. The manifest key must be a path prefix of every emitted file.",
                import_key,
                wit_path.display(),
                rel.trim_end_matches(".can"),
                import_key,
            )));
        }

        let target = bindgen_root.join(&rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                InstallError(format!("could not create `{}`: {e}", parent.display()))
            })?;
        }

        // Run the emitted source through the canonical formatter, same as
        // `canon bindgen` does. Keeps the on-disk artifact stable across
        // future bindgen whitespace tweaks.
        let content =
            crate::formatter::format(&file.content).unwrap_or_else(|_| file.content.clone());
        fs::write(&target, content.as_bytes())
            .map_err(|e| InstallError(format!("could not write `{}`: {e}", target.display())))?;

        index_entries.push((rel.clone(), file.urn.clone()));
        written.push(target);
        skipped.extend(file.skipped);
    }

    Ok(EntryResult {
        written,
        skipped,
        index_entries,
    })
}

/// Drop the `@<version>` suffix from every path segment:
/// `wasi/clocks@0.3.0/monotonic_clock.can` →
/// `wasi/clocks/monotonic_clock.can`. Used to compare version-carrying
/// vendored paths against version-less keys (manifest `[imports]`
/// keys, use paths) — source never states versions.
pub(crate) fn strip_version_suffixes(rel: &str) -> String {
    rel.split('/')
        .map(|seg| seg.split_once('@').map(|(l, _)| l).unwrap_or(seg))
        .collect::<Vec<_>>()
        .join("/")
}

/// True when a generated file has no real Canon declarations — only
/// blank lines and `use` directives. Same predicate `canon bindgen`
/// uses when deciding whether to skip writing a file; duplicated here
/// because the bindgen module marks it private. If a future refactor
/// promotes it, we'll switch to that.
pub(crate) fn has_no_decls(content: &str) -> bool {
    content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .all(|l| l.starts_with("use "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_version_suffixes_drops_at_segments() {
        assert_eq!(
            strip_version_suffixes("wasi/clocks@0.3.0-rc-2026-03-15/monotonic_clock.can"),
            "wasi/clocks/monotonic_clock.can",
        );
        assert_eq!(
            strip_version_suffixes("wasi/clocks/monotonic_clock.can"),
            "wasi/clocks/monotonic_clock.can",
        );
    }
}
