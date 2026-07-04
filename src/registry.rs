//! Registry-backed `canon install` — PACKAGES.md slice 2.
//!
//! `canon install <ns>:<pkg>[@<version>]` fetches a WIT package from a
//! package registry and vendors the generated Canon bindings under the
//! project's `deps/<ns>/<pkg>/` tree, each file stamped with the
//! `package` provenance directive (validated by the loader, see
//! PACKAGES.md) followed by the usual `bindings` directive.
//!
//! The transport is `wasm-pkg-client` (Bytecode Alliance
//! wasm-pkg-tools): package names are `<namespace>:<name>` and versions
//! are semver, resolved against OCI registries by default, with the
//! standard `wasm-pkg` config file supplying the namespace→registry
//! mapping (`$XDG_CONFIG_HOME/wasm-pkg/config.toml`, shared with
//! `wkg`). The `CANON_REGISTRY_CONFIG` env var points at an alternate
//! config file — that's also how tests drive the whole path offline
//! through a `local`-type registry rooted in a temp directory.
//!
//! What registries serve for a WIT package is the package encoded as a
//! wasm binary; `bindgen::generate_from_wasm_bytes` decodes it with the
//! same machinery `canon bindgen foo.wasm` uses. Only interfaces
//! belonging to the requested package are written — a registry artifact
//! may embed dependency packages in its encoding, and vendoring those
//! under the wrong `deps/<ns>/<pkg>/` directory would break the
//! loader's coordinate check (they're reported as skipped instead;
//! installing them is their own `canon install` invocation).
//!
//! Content is digest-verified by `wasm-pkg-client` itself (the stream
//! is validated against the release's content digest), so "no fetch
//! and trust" holds without a separate hashing step here. The global
//! content cache (`~/.canon/cache/`) arrives with binary component
//! deps in slice 5; WIT installs don't need it — the vendored source
//! *is* the artifact.
//!
//! # Canon source packages (`canon publish`, slice 3)
//!
//! A published Canon package is one wasm artifact — the only shape
//! every registry backend distributes — that carries the source in
//! custom sections: `canon:package` holds the coordinate, `canon:deps`
//! the newline-separated coordinates of the packages it was built
//! against (read from the publisher's own `deps/` directives — still
//! machine-recorded, never authored), and one `canon:src/<rel-path>`
//! section per `.can` file. Keeping the dependency list in-band instead
//! of in OCI annotations means it survives every backend (the `local`
//! registry has nowhere to put annotations) and stays inside the one
//! digest-verified blob. `canon install` recognizes the `canon:package`
//! section and vendors the embedded source instead of running bindgen.
//! Attaching the compiled component for entry-point packages is a
//! planned refinement of the same artifact (more custom sections on a
//! real component, instead of the minimal empty module used for
//! source-only packages).

use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};

use futures_util::TryStreamExt;
use wasm_pkg_client::{Client, Config, PackageRef, PublishOpts, Version, VersionInfo};

use crate::bindgen;
use crate::install::{has_no_decls, strip_src_segment, InstallError, InstallOutcome};

/// Custom section holding a source artifact's own coordinate.
const SECTION_PACKAGE: &str = "canon:package";
/// Custom section holding the newline-separated dependency coordinates.
const SECTION_DEPS: &str = "canon:deps";
/// Prefix of the per-file source sections; the remainder is the
/// package-relative path.
const SECTION_SRC_PREFIX: &str = "canon:src/";

/// Env var naming an alternate `wasm-pkg` config file. When unset, the
/// standard global config (with its built-in `wasi:` → wasi.dev etc.
/// defaults) applies.
pub const REGISTRY_CONFIG_ENV: &str = "CANON_REGISTRY_CONFIG";

/// A parsed `<ns>:<pkg>[@<version>]` install argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrySpec {
    pub namespace: String,
    pub name: String,
    /// `None` means "newest release". A full semver picks that exact
    /// release; a prefix (`0.3`, `1`) picks the newest release whose
    /// version starts with it.
    pub version: Option<String>,
}

impl RegistrySpec {
    /// The `deps/`-relative directory this package vendors into.
    fn deps_prefix(&self) -> String {
        format!("{}/{}/", self.namespace, self.name)
    }
}

/// Parse an install spec. The namespace/name grammar matches the
/// loader's `package` directive rule (lowercase kebab), so what install
/// accepts is exactly what the loader will re-validate on the next
/// build.
pub fn parse_spec(s: &str) -> Result<RegistrySpec, InstallError> {
    let malformed = || {
        InstallError(format!(
            "malformed package spec `{s}` (expected `<namespace>:<name>[@<version>]`)"
        ))
    };
    let (coord, version) = match s.split_once('@') {
        Some((c, v)) if !v.is_empty() => (c, Some(v.to_string())),
        Some(_) => return Err(malformed()),
        None => (s, None),
    };
    let (ns, name) = coord.split_once(':').ok_or_else(malformed)?;
    let seg_ok = |seg: &str| {
        !seg.is_empty()
            && seg
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    if !seg_ok(ns) || !seg_ok(name) {
        return Err(malformed());
    }
    Ok(RegistrySpec {
        namespace: ns.to_string(),
        name: name.to_string(),
        version,
    })
}

/// Fetch `spec` from its registry and vendor the generated bindings
/// under `<root>/deps/`. Returns the standard install outcome (paths
/// written, items skipped).
pub fn install_from_registry(
    spec: &RegistrySpec,
    root: &Path,
) -> Result<InstallOutcome, InstallError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| InstallError(format!("could not start async runtime: {e}")))?;
    let (version, bytes) = runtime.block_on(fetch(spec))?;
    match parse_source_artifact(&bytes) {
        Some(artifact) => vendor_source_package(spec, &version, &artifact, root),
        None => vendor_wit_package(spec, &version, &bytes, root),
    }
}

/// Load the registry config: the file named by `CANON_REGISTRY_CONFIG`
/// when set, the standard global `wasm-pkg` config otherwise.
async fn load_config() -> Result<Config, InstallError> {
    match std::env::var_os(REGISTRY_CONFIG_ENV) {
        Some(path) => Config::from_file(&path).await.map_err(|e| {
            InstallError(format!(
                "could not read registry config `{}`: {e}",
                PathBuf::from(&path).display()
            ))
        }),
        None => Config::global_defaults()
            .await
            .map_err(|e| InstallError(format!("could not load registry config: {e}"))),
    }
}

/// Resolve the version and download the package content.
async fn fetch(spec: &RegistrySpec) -> Result<(Version, Vec<u8>), InstallError> {
    let client = Client::new(load_config().await?);

    let package: PackageRef = format!("{}:{}", spec.namespace, spec.name)
        .parse()
        .map_err(|e| InstallError(format!("invalid package name: {e}")))?;

    let versions = client
        .list_all_versions(&package)
        .await
        .map_err(|e| InstallError(format!("could not list versions of `{package}`: {e}")))?;
    let version = pick_version(&versions, spec.version.as_deref()).ok_or_else(|| {
        InstallError(match &spec.version {
            Some(want) => format!(
                "no release of `{package}` matches `{want}` (available: {})",
                render_versions(&versions),
            ),
            None => format!("`{package}` has no releases"),
        })
    })?;

    let release = client
        .get_release(&package, &version)
        .await
        .map_err(|e| InstallError(format!("could not fetch `{package}@{version}`: {e}")))?;
    let mut stream = client
        .stream_content(&package, &release)
        .await
        .map_err(|e| InstallError(format!("could not fetch `{package}@{version}`: {e}")))?;

    let mut bytes = Vec::new();
    while let Some(chunk) = stream
        .try_next()
        .await
        .map_err(|e| InstallError(format!("download of `{package}@{version}` failed: {e}")))?
    {
        bytes.extend_from_slice(&chunk);
    }
    Ok((version, bytes))
}

/// Pick the release to install: the exact version when `want` is a full
/// semver, the newest whose rendering starts with `want` when it's a
/// prefix, the newest overall when `want` is `None`. Yanked releases
/// never match.
fn pick_version(versions: &[VersionInfo], want: Option<&str>) -> Option<Version> {
    let mut live: Vec<&Version> = versions
        .iter()
        .filter(|v| !v.yanked)
        .map(|v| &v.version)
        .collect();
    live.sort();
    match want {
        None => live.last().map(|v| (*v).clone()),
        Some(want) => {
            if let Ok(exact) = Version::parse(want) {
                return live.iter().find(|v| ***v == exact).map(|v| (*v).clone());
            }
            let prefix = format!("{want}.");
            live.iter()
                .rev()
                .find(|v| v.to_string().starts_with(&prefix))
                .map(|v| (*v).clone())
        }
    }
}

fn render_versions(versions: &[VersionInfo]) -> String {
    if versions.is_empty() {
        return "none".to_string();
    }
    versions
        .iter()
        .map(|v| v.version.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Run the wasm-encoded WIT package through bindgen and write every
/// interface belonging to `spec` under `<root>/deps/<ns>/<name>/`, each
/// file led by its `package` directive. Interfaces from other packages
/// embedded in the encoding are skipped with a note.
fn vendor_wit_package(
    spec: &RegistrySpec,
    version: &Version,
    bytes: &[u8],
    root: &Path,
) -> Result<InstallOutcome, InstallError> {
    let emitted = bindgen::generate_from_wasm_bytes(bytes).map_err(|e| {
        InstallError(format!(
            "bindgen failed for `{}:{}@{version}`: {e}",
            spec.namespace, spec.name
        ))
    })?;

    let coordinate = format!("{}:{}@{version}", spec.namespace, spec.name);
    let deps_root = root.join("deps");
    let prefix = spec.deps_prefix();

    let mut written: Vec<PathBuf> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for file in emitted {
        if has_no_decls(&file.content) {
            skipped.extend(file.skipped);
            continue;
        }
        // `<ns>/src/<pkg>/<iface>.can` → `<ns>/<pkg>/<iface>.can`, same
        // normalization as the manifest-driven install.
        let rel = strip_src_segment(&file.relative_path).ok_or_else(|| {
            InstallError(format!(
                "bindgen produced an unexpected path `{}` (expected `<ns>/src/<pkg>/<iface>.can`)",
                file.relative_path
            ))
        })?;
        // Only the requested package may land in `deps/<ns>/<name>/` —
        // the loader's coordinate check makes any other placement an
        // error on the next build, so filter here and say so.
        if !rel.starts_with(&prefix) {
            skipped.push(format!(
                "interface `{}` belongs to another package embedded in the artifact; install it explicitly",
                rel.trim_end_matches(".can"),
            ));
            continue;
        }

        // Stamp provenance first, then canonical-format the whole file —
        // the loader requires the `package` directive to be the first
        // declaration and the formatter keeps it there.
        let stamped = format!("package \"{coordinate}\"\n\n{}", file.content);
        let content = crate::formatter::format(&stamped).map_err(|e| {
            InstallError(format!(
                "generated bindings for `{rel}` do not parse: {e:?}"
            ))
        })?;

        let target = deps_root.join(&rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                InstallError(format!("could not create `{}`: {e}", parent.display()))
            })?;
        }
        fs::write(&target, content.as_bytes())
            .map_err(|e| InstallError(format!("could not write `{}`: {e}", target.display())))?;
        written.push(target);
        skipped.extend(file.skipped);
    }

    if written.is_empty() {
        return Err(InstallError(format!(
            "`{coordinate}` produced no installable interfaces{}",
            if skipped.is_empty() {
                String::new()
            } else {
                format!(" (skipped: {})", skipped.join("; "))
            },
        )));
    }

    written.sort();
    Ok(InstallOutcome { written, skipped })
}

/// A decoded Canon source artifact (see the module docs).
struct SourceArtifact {
    /// The coordinate the artifact claims for itself (`ns:name@ver`).
    coordinate: String,
    /// Dependency coordinates recorded at publish time.
    deps: Vec<String>,
    /// `(package-relative path, source text)` per `.can` file.
    files: Vec<(String, String)>,
}

/// Decode `bytes` as a Canon source artifact. Returns `None` when the
/// bytes aren't one (not wasm, or wasm without a `canon:package`
/// section) — the caller falls through to the WIT bindgen path, whose
/// own errors are the right ones for genuinely malformed input.
fn parse_source_artifact(bytes: &[u8]) -> Option<SourceArtifact> {
    let mut coordinate = None;
    let mut deps = Vec::new();
    let mut files = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        let Ok(payload) = payload else { return None };
        let wasmparser::Payload::CustomSection(section) = payload else {
            continue;
        };
        let text = || String::from_utf8(section.data().to_vec()).ok();
        match section.name() {
            SECTION_PACKAGE => coordinate = text(),
            SECTION_DEPS => {
                deps = text()?
                    .lines()
                    .map(str::to_string)
                    .filter(|l| !l.is_empty())
                    .collect();
            }
            name => {
                if let Some(rel) = name.strip_prefix(SECTION_SRC_PREFIX) {
                    files.push((rel.to_string(), text()?));
                }
            }
        }
    }
    Some(SourceArtifact {
        coordinate: coordinate?,
        deps,
        files,
    })
}

/// Vendor a fetched Canon source package under `<root>/deps/<ns>/<name>/`:
/// every embedded file is stamped with the `package` directive and
/// canonically formatted. Dependencies the package was published
/// against are reported, not installed — transitive install is
/// PACKAGES.md slice 4.
fn vendor_source_package(
    spec: &RegistrySpec,
    version: &Version,
    artifact: &SourceArtifact,
    root: &Path,
) -> Result<InstallOutcome, InstallError> {
    let coordinate = format!("{}:{}@{version}", spec.namespace, spec.name);
    // The artifact self-describes; a name mismatch means the registry
    // served something other than what was asked for. (The *version* is
    // taken from the release we resolved, which the content digest
    // already ties to these bytes.)
    if !artifact
        .coordinate
        .starts_with(&format!("{}:{}@", spec.namespace, spec.name))
    {
        return Err(InstallError(format!(
            "artifact claims to be `{}` but was fetched as `{coordinate}`",
            artifact.coordinate,
        )));
    }
    if artifact.files.is_empty() {
        return Err(InstallError(format!(
            "`{coordinate}` is a Canon source package with no source files"
        )));
    }

    let pkg_root = root.join("deps").join(&spec.namespace).join(&spec.name);
    let mut written = Vec::new();
    for (rel, source) in &artifact.files {
        // Defensive: a hostile artifact must not write outside its own
        // `deps/<ns>/<name>/` directory.
        if rel.split('/').any(|seg| seg == ".." || seg.is_empty()) || rel.starts_with('/') {
            return Err(InstallError(format!(
                "`{coordinate}` contains an invalid source path `{rel}`"
            )));
        }
        let stamped = format!("package \"{coordinate}\"\n\n{source}");
        let content = crate::formatter::format(&stamped).map_err(|e| {
            InstallError(format!(
                "source file `{rel}` in `{coordinate}` does not parse: {e:?}"
            ))
        })?;
        let target = pkg_root.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                InstallError(format!("could not create `{}`: {e}", parent.display()))
            })?;
        }
        fs::write(&target, content.as_bytes())
            .map_err(|e| InstallError(format!("could not write `{}`: {e}", target.display())))?;
        written.push(target);
    }

    let skipped = artifact
        .deps
        .iter()
        .map(|dep| {
            format!(
                "`{coordinate}` depends on `{dep}`: install it with `canon install {}` (transitive install lands with PACKAGES.md slice 4)",
                dep.split('@').next().unwrap_or(dep),
            )
        })
        .collect();

    written.sort();
    Ok(InstallOutcome { written, skipped })
}

// ---------------------------------------------------------------------------
// canon publish
// ---------------------------------------------------------------------------

/// Outcome of a successful publish.
#[derive(Debug)]
pub struct PublishOutcome {
    /// The full coordinate that was published (`ns:name@version`).
    pub coordinate: String,
    /// Package-relative paths of the source files included.
    pub files: Vec<String>,
}

/// Publish the Canon package rooted at `root` as `spec`. The package is
/// every `.can` file under `root` except the vendored/derived trees;
/// its recorded dependency list is read off the `deps/` directives.
/// With no version in `spec`, the registry's newest release is
/// patch-bumped (first publish starts at `0.1.0`).
pub fn publish_to_registry(
    spec: &RegistrySpec,
    root: &Path,
) -> Result<PublishOutcome, InstallError> {
    let files = collect_publish_sources(root)?;
    preflight(&files, root)?;
    let deps = collect_dep_coordinates(root)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| InstallError(format!("could not start async runtime: {e}")))?;
    runtime.block_on(async {
        let client = Client::new(load_config().await?);
        let package: PackageRef = format!("{}:{}", spec.namespace, spec.name)
            .parse()
            .map_err(|e| InstallError(format!("invalid package name: {e}")))?;

        let version = resolve_publish_version(&client, &package, spec.version.as_deref()).await?;
        let coordinate = format!("{package}@{version}");
        let artifact = build_source_artifact(&coordinate, &deps, &files);

        client
            .publish_release_data(
                Box::pin(std::io::Cursor::new(artifact)),
                PublishOpts {
                    package: Some((package.clone(), version.clone())),
                    registry: None,
                },
            )
            .await
            .map_err(|e| InstallError(format!("could not publish `{coordinate}`: {e}")))?;

        Ok(PublishOutcome {
            coordinate,
            files: files.into_iter().map(|(rel, _)| rel).collect(),
        })
    })
}

/// Decide the version to publish. An explicit full semver is used
/// verbatim; no version means "patch-bump the newest release", or
/// `0.1.0` for a package with no releases yet. (Version *prefixes* are
/// an install-side constraint; publishing needs an exact point, so
/// they're rejected here.)
async fn resolve_publish_version(
    client: &Client,
    package: &PackageRef,
    want: Option<&str>,
) -> Result<Version, InstallError> {
    if let Some(want) = want {
        return Version::parse(want).map_err(|e| {
            InstallError(format!(
                "`@{want}` is not a full version (publish needs an exact `x.y.z`): {e}"
            ))
        });
    }
    // A package that has never been published has no versions to list;
    // backends surface that as an error (e.g. the local backend's
    // missing directory), which for publishing just means "start
    // fresh".
    let versions = client.list_all_versions(package).await.unwrap_or_default();
    Ok(match pick_version(&versions, None) {
        Some(latest) => Version::new(latest.major, latest.minor, latest.patch + 1),
        None => Version::new(0, 1, 0),
    })
}

/// Collect the package's source files: every `.can` under `root`,
/// excluding the vendored tree (`deps/`), derived trees (`bindgen/`,
/// `.canon/`, `target/`), and hidden directories. Paths are returned
/// package-relative with `/` separators, sorted.
fn collect_publish_sources(root: &Path) -> Result<Vec<(String, String)>, InstallError> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir)
            .map_err(|e| InstallError(format!("could not read `{}`: {e}", dir.display())))?;
        for entry in entries {
            let entry = entry
                .map_err(|e| InstallError(format!("could not read `{}`: {e}", dir.display())))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if name.starts_with('.') || matches!(name.as_str(), "deps" | "bindgen" | "target") {
                    continue;
                }
                stack.push(path);
            } else if name.ends_with(".can") {
                let rel = path
                    .strip_prefix(root)
                    .expect("walked path is under root")
                    .to_string_lossy()
                    .replace('\\', "/");
                let source = fs::read_to_string(&path).map_err(|e| {
                    InstallError(format!("could not read `{}`: {e}", path.display()))
                })?;
                files.push((rel, source));
            }
        }
    }
    if files.is_empty() {
        return Err(InstallError(format!(
            "no `.can` files under `{}`: nothing to publish",
            root.display()
        )));
    }
    files.sort();
    Ok(files)
}

/// Publish preflight. Every file must parse and already be in canonical
/// format (the vendored copy consumers get is stamped and re-formatted;
/// publishing unformatted source would make the two diverge). When the
/// package has a `main.can` entry, the full checker runs too — a
/// program that doesn't check doesn't publish. (Pure libraries have no
/// entry point to check from; their errors surface in consumers, per
/// DESIGN.md's dead-code stance.)
fn preflight(files: &[(String, String)], root: &Path) -> Result<(), InstallError> {
    for (rel, source) in files {
        let formatted = crate::formatter::format(source)
            .map_err(|e| InstallError(format!("`{rel}` does not parse: {e:?}")))?;
        if &formatted != source {
            return Err(InstallError(format!(
                "`{rel}` is not canonically formatted: run `canon fmt` before publishing"
            )));
        }
    }
    let entry = root.join("main.can");
    if entry.is_file() {
        let loaded = crate::loader::load_module(&entry)
            .map_err(|e| InstallError(format!("package does not check: {}", e.message())))?;
        let errors = crate::checker::check_with_entry(&loaded.module, loaded.entry_items_start);
        if !errors.is_empty() {
            return Err(InstallError(format!(
                "package does not check ({} error(s)): run `canon check main.can`",
                errors.len()
            )));
        }
    }
    Ok(())
}

/// Read the unique package coordinates off the `deps/` tree's `package`
/// directives — the machine-recorded dependency list a published
/// artifact carries for consumers. A project with no `deps/` records
/// none.
fn collect_dep_coordinates(root: &Path) -> Result<Vec<String>, InstallError> {
    let deps_root = root.join("deps");
    let mut coords = std::collections::BTreeSet::new();
    if !deps_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut stack = vec![deps_root];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir)
            .map_err(|e| InstallError(format!("could not read `{}`: {e}", dir.display())))?;
        for entry in entries {
            let entry = entry
                .map_err(|e| InstallError(format!("could not read `{}`: {e}", dir.display())))?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("can") {
                let source = fs::read_to_string(&path).map_err(|e| {
                    InstallError(format!("could not read `{}`: {e}", path.display()))
                })?;
                if let Some(coord) = source
                    .lines()
                    .next()
                    .and_then(|l| l.strip_prefix("package \""))
                    .and_then(|l| l.strip_suffix('"'))
                {
                    coords.insert(coord.to_string());
                }
            }
        }
    }
    Ok(coords.into_iter().collect())
}

/// Assemble the source artifact: a minimal wasm module whose custom
/// sections carry the coordinate, the dependency list, and every source
/// file. See the module docs for why this is the wire format.
fn build_source_artifact(coordinate: &str, deps: &[String], files: &[(String, String)]) -> Vec<u8> {
    let mut module = wasm_encoder::Module::new();
    module.section(&wasm_encoder::CustomSection {
        name: Cow::Borrowed(SECTION_PACKAGE),
        data: Cow::Borrowed(coordinate.as_bytes()),
    });
    if !deps.is_empty() {
        let joined = deps.join("\n");
        module.section(&wasm_encoder::CustomSection {
            name: Cow::Borrowed(SECTION_DEPS),
            data: Cow::Owned(joined.into_bytes()),
        });
    }
    for (rel, source) in files {
        module.section(&wasm_encoder::CustomSection {
            name: Cow::Owned(format!("{SECTION_SRC_PREFIX}{rel}")),
            data: Cow::Borrowed(source.as_bytes()),
        });
    }
    module.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> VersionInfo {
        VersionInfo {
            version: Version::parse(s).unwrap(),
            yanked: false,
        }
    }

    #[test]
    fn parses_full_spec() {
        let s = parse_spec("wasi:random@0.3.0").unwrap();
        assert_eq!(s.namespace, "wasi");
        assert_eq!(s.name, "random");
        assert_eq!(s.version.as_deref(), Some("0.3.0"));
    }

    #[test]
    fn parses_versionless_spec() {
        let s = parse_spec("acme:image-decoder").unwrap();
        assert_eq!(s.namespace, "acme");
        assert_eq!(s.name, "image-decoder");
        assert!(s.version.is_none());
    }

    #[test]
    fn rejects_bad_specs() {
        for bad in [
            "acme",
            "acme:",
            ":http",
            "Acme:http",
            "acme:http@",
            "acme/http",
        ] {
            assert!(parse_spec(bad).is_err(), "`{bad}` should be rejected");
        }
    }

    #[test]
    fn picks_newest_without_constraint() {
        let vs = [v("0.9.0"), v("1.2.0"), v("1.10.0")];
        assert_eq!(
            pick_version(&vs, None),
            Some(Version::parse("1.10.0").unwrap())
        );
    }

    #[test]
    fn picks_exact_full_version() {
        let vs = [v("1.0.0"), v("1.1.0")];
        assert_eq!(
            pick_version(&vs, Some("1.0.0")),
            Some(Version::parse("1.0.0").unwrap())
        );
        assert_eq!(pick_version(&vs, Some("2.0.0")), None);
    }

    #[test]
    fn picks_newest_matching_prefix() {
        let vs = [v("0.2.9"), v("0.3.0"), v("0.3.4"), v("1.0.0")];
        assert_eq!(
            pick_version(&vs, Some("0.3")),
            Some(Version::parse("0.3.4").unwrap())
        );
    }

    #[test]
    fn source_artifact_round_trips() {
        let files = vec![(
            "greet/shout.can".to_string(),
            "shout = (String) -> String {\n    String.concat(\"!\")\n}\n".to_string(),
        )];
        let deps = vec!["other:pkg@1.2.3".to_string()];
        let bytes = build_source_artifact("acme:greet@1.0.0", &deps, &files);
        let artifact = parse_source_artifact(&bytes).expect("round trip");
        assert_eq!(artifact.coordinate, "acme:greet@1.0.0");
        assert_eq!(artifact.deps, deps);
        assert_eq!(artifact.files, files);
    }

    #[test]
    fn non_source_bytes_are_not_a_source_artifact() {
        assert!(parse_source_artifact(b"not wasm at all").is_none());
        // A valid wasm module without the `canon:package` section (the
        // shape of every WIT-package artifact) must fall through to the
        // bindgen path.
        let empty = wasm_encoder::Module::new().finish();
        assert!(parse_source_artifact(&empty).is_none());
    }

    #[test]
    fn yanked_versions_never_match() {
        let yanked = VersionInfo {
            version: Version::parse("2.0.0").unwrap(),
            yanked: true,
        };
        let vs = [v("1.0.0"), yanked];
        assert_eq!(
            pick_version(&vs, None),
            Some(Version::parse("1.0.0").unwrap())
        );
    }
}
