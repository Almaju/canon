//! `canon add` — vendor a package into the project's `deps/` tree.
//!
//! A dependency is a directory: `deps/<ns>/<name>@<version>/` holds the
//! package's sources and the directory name is the pin (modules &
//! packages, docs/src/spec/modules.md). There is nothing else to write —
//! no manifest, no lockfile — so adding a package is copying it there,
//! and the argument's shape says where from: a bare `<ns>/<name>` is a
//! package that ships in the compiler binary beside the prelude, at the
//! toolchain's version; `<git-url>@<tag>` is a repository cloned at that
//! tag, whose last two path segments name the package. Adding a package
//! already vendored at another version replaces it: the loader rejects
//! two versions of one package, so the tree only ever holds one.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::loader::{BundledPackage, BUNDLED_PACKAGES, PRELUDE};

/// The version every bundled package carries — the toolchain's.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Outcome of a successful add.
#[derive(Debug)]
pub struct AddOutcome {
    /// `<ns>/<name>` of what was vendored.
    pub package: String,
    pub version: String,
    /// The vendored package directory, `deps/<ns>/<name>@<version>/`.
    pub dir: PathBuf,
    /// Number of source files written into it.
    pub files: usize,
    /// The version this add replaced, when one was vendored already.
    pub replaced: Option<String>,
}

/// The bundled packages `canon add` can vendor: everything but the
/// prelude, which every program already has.
pub fn addable() -> impl Iterator<Item = &'static BundledPackage> {
    BUNDLED_PACKAGES.iter().filter(|p| p.name != PRELUDE)
}

/// Vendor `spec` — a bundled package name or `<git-url>@<tag>` — under
/// `<project_root>/deps/`.
pub fn add(project_root: &Path, spec: &str) -> Result<AddOutcome, String> {
    match spec.rsplit_once('@') {
        Some((url, tag)) if !url.is_empty() && !tag.is_empty() => add_git(project_root, url, tag),
        _ => add_bundled(project_root, spec),
    }
}

fn add_bundled(project_root: &Path, name: &str) -> Result<AddOutcome, String> {
    if name == PRELUDE {
        return Err(format!(
            "`{PRELUDE}` is the prelude — every program already has it"
        ));
    }
    let Some(pkg) = addable().find(|p| p.name == name) else {
        let known: Vec<&str> = addable().map(|p| p.name).collect();
        return Err(format!(
            "no bundled package named `{name}` (this toolchain ships: {}); a git package is `<git-url>@<tag>`",
            known.join(", ")
        ));
    };
    let Some((ns, leaf)) = name.split_once('/').filter(|(_, l)| !l.contains('/')) else {
        return Err(format!(
            "`{name}` has no `<namespace>/<name>` shape a `deps/` directory can spell"
        ));
    };
    let sources: Vec<(String, String)> = pkg
        .files
        .iter()
        .map(|f| (f.path.to_string(), f.source.to_string()))
        .collect();
    vendor(project_root, ns, leaf, VERSION, &sources)
}

/// Clone `url` at `tag` and vendor its `src/`. The tag is the version
/// (a leading `v` dropped); the URL's last two path segments are the
/// namespace and name.
fn add_git(project_root: &Path, url: &str, tag: &str) -> Result<AddOutcome, String> {
    let version = tag.trim_start_matches('v');
    let mut segments = url
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit(['/', ':'])
        .filter(|s| !s.is_empty());
    let (Some(name), Some(ns)) = (segments.next(), segments.next()) else {
        return Err(format!(
            "`{url}` names no `<owner>/<name>` — the last two path segments are the package's"
        ));
    };
    let clone = std::env::temp_dir().join(format!("canon-add-{}", std::process::id()));
    let _ = fs::remove_dir_all(&clone);
    let cloned = Command::new("git")
        .args(["clone", "--quiet", "--depth", "1", "--branch", tag, url])
        .arg(&clone)
        .output();
    match cloned {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            return Err(format!(
                "could not clone `{url}` at `{tag}`:\n{}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("`canon add` clones with `git`, which is not on PATH".to_string());
        }
        Err(e) => return Err(format!("failed to run `git`: {e}")),
    }
    let src = clone.join("src");
    let mut sources = Vec::new();
    let read = collect_sources(&src, &src, &mut sources);
    let _ = fs::remove_dir_all(&clone);
    read?;
    if sources.is_empty() {
        return Err(format!(
            "`{url}` is not a Canon package: no `.can` files under `src/` at `{tag}`"
        ));
    }
    vendor(project_root, ns, name, version, &sources)
}

/// Every `.can` file under `dir`, as `(path relative to root, contents)`.
fn collect_sources(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) -> Result<(), String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(root, &path, out)?;
        } else if path.extension().is_some_and(|e| e == "can") {
            let rel = path
                .strip_prefix(root)
                .expect("file under root")
                .to_string_lossy()
                .replace('\\', "/");
            let source = fs::read_to_string(&path)
                .map_err(|e| format!("could not read `{}`: {e}", path.display()))?;
            out.push((rel, source));
        }
    }
    Ok(())
}

/// Write `sources` to `deps/<ns>/<name>@<version>/`, removing any other
/// version of the package first.
fn vendor(
    project_root: &Path,
    ns: &str,
    name: &str,
    version: &str,
    sources: &[(String, String)],
) -> Result<AddOutcome, String> {
    let ns_dir = project_root.join("deps").join(ns);
    let dir = ns_dir.join(format!("{name}@{version}"));
    let mut replaced = None;
    for entry in fs::read_dir(&ns_dir).into_iter().flatten().flatten() {
        let file_name = entry.file_name();
        let Some((existing, old)) = file_name.to_str().and_then(|n| n.split_once('@')) else {
            continue;
        };
        if existing != name {
            continue;
        }
        if old != version {
            replaced = Some(old.to_string());
        }
        fs::remove_dir_all(entry.path())
            .map_err(|e| format!("could not remove `{}`: {e}", entry.path().display()))?;
    }
    for (rel, source) in sources {
        let target = dir.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("could not create `{}`: {e}", parent.display()))?;
        }
        fs::write(&target, source)
            .map_err(|e| format!("could not write `{}`: {e}", target.display()))?;
    }
    Ok(AddOutcome {
        package: format!("{ns}/{name}"),
        version: version.to_string(),
        dir,
        files: sources.len(),
        replaced,
    })
}
