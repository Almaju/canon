//! Package manifest (`oneway.toml`) parser.
//!
//! A package's manifest is a tiny TOML file declaring its identity, version,
//! optional fetch information, and dependencies. The compiler parses only the
//! subset shown in DESIGN.md § Package Manifests — top-level string keys plus
//! at most one `[deps]` table with quoted string keys and string values. Full
//! TOML compatibility is a non-goal; we want editor support and human
//! familiarity, not expressiveness. Keeping the parser hand-written preserves
//! the compiler's zero-external-dependency property.
//!
//! Example accepted package input:
//!
//! ```toml
//! name    = "oneway/std"
//! version = "0.1.0"
//! from    = "https://example.com/component.wasm"
//! sha256  = "ab12cd..."
//!
//! [deps]
//! "oneway/wasi"        = "0.3.x"
//! "acme/image-decoder" = "1.0.x"
//! ```
//!
//! Example accepted workspace input (a workspace itself isn't a package —
//! it just aggregates member packages, Cargo-style):
//!
//! ```toml
//! [workspace]
//! members = ["*"]              # every subdir with an oneway.toml
//! # members = ["clock", "now"] # or an explicit list of subpaths
//! ```
//!
//! Anything outside these shapes — inline tables, multiline strings,
//! integers, booleans, dotted keys, arrays outside `[workspace] members`,
//! unknown tables — is a parse error. If the schema needs to grow, this
//! module grows with it; we don't reach for a TOML crate until the cost is
//! unambiguous.

use std::collections::BTreeMap;

/// The fully parsed contents of an `oneway.toml`.
///
/// `deps` uses `BTreeMap` so iteration is alphabetical, matching the
/// "alphabetical wherever ordering is discretionary" rule.
///
/// When `workspace` is `Some`, this manifest is a workspace root and
/// `name` / `version` may be empty ("virtual workspace", Cargo-style).
/// When `workspace` is `None`, the manifest describes a package and both
/// `name` and `version` are required.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub from: Option<String>,
    pub sha256: Option<String>,
    pub deps: BTreeMap<String, String>,
    pub workspace: Option<WorkspaceConfig>,
}

/// The `[workspace]` table of a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceConfig {
    /// Member subpaths relative to the workspace root. A single literal
    /// `"*"` means "every immediate subdirectory containing an
    /// `oneway.toml`" and is expanded by the loader, not the parser.
    pub members: Vec<String>,
}

/// A manifest parse error. Carries the 1-based line number where the error
/// was detected — the manifest is small enough that a line number is all the
/// localization a human reader needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "oneway.toml:{}: {}", self.line, self.message)
    }
}

impl std::error::Error for ManifestError {}

/// Which top-level table we are currently inside while scanning lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    /// Bare top-level fields (`name`, `version`, `from`, `sha256`).
    TopLevel,
    /// Inside `[deps]`.
    Deps,
    /// Inside `[workspace]`.
    Workspace,
}

/// Parse the contents of an `oneway.toml` into a `Manifest`.
pub fn parse(source: &str) -> Result<Manifest, ManifestError> {
    let mut manifest = Manifest::default();
    let mut section = Section::TopLevel;
    let mut saw_deps_header = false;
    let mut saw_workspace_header = false;

    let mut name_seen = false;
    let mut version_seen = false;
    let mut from_seen = false;
    let mut sha256_seen = false;
    let mut members_seen = false;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }

        // Table header: `[deps]` or `[workspace]`. Each may appear at most
        // once. Other table names are rejected.
        if let Some(header) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let header = header.trim();
            match header {
                "deps" => {
                    if saw_deps_header {
                        return Err(err(line_no, "duplicate `[deps]` table".to_string()));
                    }
                    saw_deps_header = true;
                    section = Section::Deps;
                }
                "workspace" => {
                    if saw_workspace_header {
                        return Err(err(line_no, "duplicate `[workspace]` table".to_string()));
                    }
                    saw_workspace_header = true;
                    manifest.workspace = Some(WorkspaceConfig::default());
                    section = Section::Workspace;
                }
                _ => {
                    return Err(err(
                        line_no,
                        format!(
                            "unknown table `[{header}]` (expected one of: `[deps]`, `[workspace]`)"
                        ),
                    ));
                }
            }
            continue;
        }

        let (key, value) = split_key_value(line, line_no)?;

        if section == Section::Workspace {
            match key.as_str() {
                "members" => {
                    if members_seen {
                        return Err(err(line_no, "duplicate `members` key".to_string()));
                    }
                    members_seen = true;
                    let arr = parse_string_array(&value, line_no)?;
                    manifest
                        .workspace
                        .as_mut()
                        .expect("workspace was set")
                        .members = arr;
                }
                other => {
                    return Err(err(
                        line_no,
                        format!("unknown key `{other}` in `[workspace]` (expected `members`)"),
                    ));
                }
            }
        } else if section == Section::Deps {
            // `[deps]` entries use quoted package paths as keys.
            let dep_name =
                unquote(&key, line_no).map_err(|m| err(line_no, format!("dependency key: {m}")))?;
            if dep_name.is_empty() {
                return Err(err(line_no, "dependency name cannot be empty".to_string()));
            }
            if manifest.deps.contains_key(&dep_name) {
                return Err(err(line_no, format!("duplicate dependency `{dep_name}`")));
            }
            let version = unquote(&value, line_no)
                .map_err(|m| err(line_no, format!("dependency version: {m}")))?;
            manifest.deps.insert(dep_name, version);
        } else {
            debug_assert_eq!(section, Section::TopLevel);
            // Top-level fields: each has a fixed name and string value.
            let value = unquote(&value, line_no)
                .map_err(|m| err(line_no, format!("value of `{key}`: {m}")))?;
            match key.as_str() {
                "name" => {
                    if name_seen {
                        return Err(err(line_no, "duplicate `name`".to_string()));
                    }
                    name_seen = true;
                    manifest.name = value;
                }
                "version" => {
                    if version_seen {
                        return Err(err(line_no, "duplicate `version`".to_string()));
                    }
                    version_seen = true;
                    manifest.version = value;
                }
                "from" => {
                    if from_seen {
                        return Err(err(line_no, "duplicate `from`".to_string()));
                    }
                    from_seen = true;
                    manifest.from = Some(value);
                }
                "sha256" => {
                    if sha256_seen {
                        return Err(err(line_no, "duplicate `sha256`".to_string()));
                    }
                    sha256_seen = true;
                    manifest.sha256 = Some(value);
                }
                other => {
                    return Err(err(
                        line_no,
                        format!(
                            "unknown key `{other}` (expected one of: name, version, from, sha256)"
                        ),
                    ));
                }
            }
        }
    }

    // Workspaces don't need `name` / `version`. Packages do.
    let is_workspace = manifest.workspace.is_some();
    if !is_workspace && !name_seen {
        return Err(err(0, "missing required field `name`".to_string()));
    }
    if !is_workspace && !version_seen {
        return Err(err(0, "missing required field `version`".to_string()));
    }
    if manifest.sha256.is_some() && manifest.from.is_none() {
        return Err(err(
            0,
            "`sha256` requires `from` (a sha256 with no source URL is meaningless)".to_string(),
        ));
    }
    if manifest.from.is_some() && manifest.sha256.is_none() {
        return Err(err(
            0,
            "`from` requires `sha256` (fetched components must be hash-verified)".to_string(),
        ));
    }

    Ok(manifest)
}

fn err(line: usize, message: String) -> ManifestError {
    ManifestError { line, message }
}

/// Strip a trailing `# …` comment from a line. We do this before quoting is
/// considered, so a `#` inside a quoted string would be misinterpreted as a
/// comment. The manifest schema accepts only string values that are package
/// names, versions, URLs, and hex digests — none of these legitimately
/// contain `#`, so the simple rule is safe.
fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(idx) => &line[..idx],
        None => line,
    }
}

/// Split `key = value` (with arbitrary surrounding whitespace) into its two
/// halves. The key may be quoted (for `[deps]` entries) or bare (for
/// top-level fields); we return both halves as-found and let the caller
/// decide whether to `unquote` them.
fn split_key_value(line: &str, line_no: usize) -> Result<(String, String), ManifestError> {
    let eq = line
        .find('=')
        .ok_or_else(|| err(line_no, format!("expected `key = value`, got `{line}`")))?;
    let key = line[..eq].trim().to_string();
    let value = line[eq + 1..].trim().to_string();
    if key.is_empty() {
        return Err(err(line_no, "empty key".to_string()));
    }
    if value.is_empty() {
        return Err(err(line_no, "empty value".to_string()));
    }
    Ok((key, value))
}

/// Parse a single-line array literal of quoted strings, e.g.
/// `["a", "b", "c"]`. Empty arrays (`[]`) are allowed. Whitespace inside
/// the brackets and around commas is tolerated. Trailing commas are not.
///
/// This is the only array we accept anywhere in the manifest grammar —
/// only `[workspace] members` uses it. If a future field needs arrays
/// too, route it through this same helper.
fn parse_string_array(raw: &str, line_no: usize) -> Result<Vec<String>, ManifestError> {
    let inner = raw
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| {
            err(
                line_no,
                format!("expected array literal `[\"...\", ...]`, got `{raw}`"),
            )
        })?
        .trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for piece in inner.split(',') {
        let p = piece.trim();
        if p.is_empty() {
            return Err(err(
                line_no,
                "empty array entry (no trailing or doubled commas)".to_string(),
            ));
        }
        let s = unquote(p, line_no).map_err(|m| err(line_no, format!("array entry: {m}")))?;
        out.push(s);
    }
    Ok(out)
}

/// Unquote a `"…"` string literal. We accept only basic strings — no
/// escapes (the values we care about don't need them), no multiline
/// strings, no literal strings (`'…'`). If we ever need escapes, we add
/// them here and not before.
fn unquote(raw: &str, _line_no: usize) -> Result<String, String> {
    if raw.len() < 2 || !raw.starts_with('"') || !raw.ends_with('"') {
        return Err(format!("expected quoted string, got `{raw}`"));
    }
    let inner = &raw[1..raw.len() - 1];
    if inner.contains('"') {
        return Err("embedded `\"` in string value not supported".to_string());
    }
    if inner.contains('\\') {
        return Err("escape sequences in string values not supported".to_string());
    }
    Ok(inner.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_manifest() {
        let src = r#"
            name    = "oneway/std"
            version = "0.1.0"
        "#;
        let m = parse(src).unwrap();
        assert_eq!(m.name, "oneway/std");
        assert_eq!(m.version, "0.1.0");
        assert!(m.from.is_none());
        assert!(m.sha256.is_none());
        assert!(m.deps.is_empty());
    }

    #[test]
    fn parses_manifest_with_deps() {
        let src = r#"
name = "my-app"
version = "0.1.0"

[deps]
"oneway/std"         = "0.1.x"
"acme/image-decoder" = "1.0.x"
"#;
        let m = parse(src).unwrap();
        assert_eq!(m.deps.len(), 2);
        assert_eq!(m.deps.get("oneway/std").map(String::as_str), Some("0.1.x"));
        assert_eq!(
            m.deps.get("acme/image-decoder").map(String::as_str),
            Some("1.0.x"),
        );
    }

    #[test]
    fn parses_manifest_with_from_and_sha256() {
        let src = r#"
name    = "acme/image-decoder"
version = "1.0.0"
from    = "https://example.com/decoder.wasm"
sha256  = "ab12cd34ef56"
"#;
        let m = parse(src).unwrap();
        assert_eq!(m.from.as_deref(), Some("https://example.com/decoder.wasm"));
        assert_eq!(m.sha256.as_deref(), Some("ab12cd34ef56"));
    }

    #[test]
    fn rejects_missing_name() {
        let src = r#"version = "0.1.0""#;
        let e = parse(src).unwrap_err();
        assert!(e.message.contains("name"));
    }

    #[test]
    fn rejects_missing_version() {
        let src = r#"name = "foo/bar""#;
        let e = parse(src).unwrap_err();
        assert!(e.message.contains("version"));
    }

    #[test]
    fn rejects_sha256_without_from() {
        let src = r#"
name = "foo/bar"
version = "0.1.0"
sha256 = "ab12"
"#;
        let e = parse(src).unwrap_err();
        assert!(e.message.contains("requires `from`"));
    }

    #[test]
    fn rejects_from_without_sha256() {
        let src = r#"
name = "foo/bar"
version = "0.1.0"
from = "https://example.com/x.wasm"
"#;
        let e = parse(src).unwrap_err();
        assert!(e.message.contains("requires `sha256`"));
    }

    #[test]
    fn rejects_unknown_top_level_key() {
        let src = r#"
name = "foo/bar"
version = "0.1.0"
license = "MIT"
"#;
        let e = parse(src).unwrap_err();
        assert!(e.message.contains("license"));
    }

    #[test]
    fn rejects_unknown_table() {
        let src = r#"
name = "foo/bar"
version = "0.1.0"

[features]
"x" = "y"
"#;
        let e = parse(src).unwrap_err();
        assert!(e.message.contains("features"));
    }

    #[test]
    fn rejects_duplicate_table() {
        let src = r#"
name = "foo/bar"
version = "0.1.0"

[deps]
"a/b" = "1.0"

[deps]
"c/d" = "1.0"
"#;
        let e = parse(src).unwrap_err();
        assert!(e.message.contains("duplicate"));
    }

    #[test]
    fn rejects_duplicate_dep() {
        let src = r#"
name = "foo/bar"
version = "0.1.0"

[deps]
"a/b" = "1.0"
"a/b" = "2.0"
"#;
        let e = parse(src).unwrap_err();
        assert!(e.message.contains("duplicate dependency"));
    }

    #[test]
    fn rejects_unquoted_value() {
        let src = r#"
name = foo/bar
version = "0.1.0"
"#;
        let e = parse(src).unwrap_err();
        assert!(e.message.contains("quoted"));
    }

    #[test]
    fn strips_comments() {
        let src = r#"
# package identity
name = "foo/bar"  # the canonical name
version = "0.1.0"
"#;
        let m = parse(src).unwrap();
        assert_eq!(m.name, "foo/bar");
        assert_eq!(m.version, "0.1.0");
    }

    #[test]
    fn parses_workspace_with_glob_members() {
        let src = r#"
[workspace]
members = ["*"]
"#;
        let m = parse(src).unwrap();
        assert!(m.name.is_empty());
        assert!(m.version.is_empty());
        let ws = m.workspace.unwrap();
        assert_eq!(ws.members, vec!["*".to_string()]);
    }

    #[test]
    fn parses_workspace_with_explicit_members() {
        let src = r#"
[workspace]
members = ["clock", "now", "http-server"]
"#;
        let m = parse(src).unwrap();
        let ws = m.workspace.unwrap();
        assert_eq!(ws.members, vec!["clock", "now", "http-server"]);
    }

    #[test]
    fn parses_empty_workspace_members() {
        let src = r#"
[workspace]
members = []
"#;
        let m = parse(src).unwrap();
        assert!(m.workspace.unwrap().members.is_empty());
    }

    #[test]
    fn rejects_workspace_with_unknown_key() {
        let src = r#"
[workspace]
resolver = "2"
"#;
        let e = parse(src).unwrap_err();
        assert!(e.message.contains("resolver"));
    }

    #[test]
    fn rejects_duplicate_workspace_table() {
        let src = r#"
[workspace]
members = ["a"]

[workspace]
members = ["b"]
"#;
        let e = parse(src).unwrap_err();
        assert!(e.message.contains("duplicate"));
    }

    #[test]
    fn rejects_trailing_comma_in_members() {
        let src = r#"
[workspace]
members = ["a", "b",]
"#;
        let e = parse(src).unwrap_err();
        assert!(e.message.contains("empty array entry"));
    }

    #[test]
    fn allows_blank_lines_and_indentation() {
        let src = "\n   name    = \"foo/bar\"\n\n   version = \"0.1.0\"\n";
        let m = parse(src).unwrap();
        assert_eq!(m.name, "foo/bar");
    }
}
