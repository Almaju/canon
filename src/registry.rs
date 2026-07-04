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

use std::fs;
use std::path::{Path, PathBuf};

use futures_util::TryStreamExt;
use wasm_pkg_client::{Client, Config, PackageRef, Version, VersionInfo};

use crate::bindgen;
use crate::install::{has_no_decls, strip_src_segment, InstallError, InstallOutcome};

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
    vendor_wit_package(spec, &version, &bytes, root)
}

/// Resolve the version and download the package content.
async fn fetch(spec: &RegistrySpec) -> Result<(Version, Vec<u8>), InstallError> {
    let config = match std::env::var_os(REGISTRY_CONFIG_ENV) {
        Some(path) => Config::from_file(&path).await.map_err(|e| {
            InstallError(format!(
                "could not read registry config `{}`: {e}",
                PathBuf::from(&path).display()
            ))
        })?,
        None => Config::global_defaults()
            .await
            .map_err(|e| InstallError(format!("could not load registry config: {e}")))?,
    };
    let client = Client::new(config);

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
