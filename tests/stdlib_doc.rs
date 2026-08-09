//! Single-source-of-truth pin for the stdlib reference's Reserved Names
//! appendix: every type name `canon` declares in
//! `packages/canon/src/` must appear in
//! `docs/src/reference/stdlib.md` — a stdlib name colliding with a user
//! type is a hard error, so the appendix is the list a writer checks
//! before naming a type. If the two drift, this fails and forces them
//! back into lockstep (the same shape as `tests/codegen_gaps.rs` pins
//! the gaps page).
//!
//! Extraction is line-based, mirroring canonical format: bodies are
//! indented, so a column-0 line is exactly a declaration — an alias
//! `Name = …` or a constructor `Recv => Name {` (return type with
//! `Result`/`Option`/`Future` peeled). camelCase binding aliases (the
//! FFI boundary) declare no type name and are skipped. `bindgen/` is
//! derived and shadowed by `src/`, so only `src/` is walked.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn collect_can_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_can_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "can") {
            out.push(path);
        }
    }
}

/// The leading `[A-Z][A-Za-z0-9]*` identifier of `s`, if `s` starts
/// with one.
fn leading_type_name(s: &str) -> Option<String> {
    let mut chars = s.chars();
    if !chars.next()?.is_ascii_uppercase() {
        return None;
    }
    Some(
        s.chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect(),
    )
}

/// The type a top-level declaration line mints, or `None` for
/// non-declaration lines (indented bodies, blanks, camelCase bindings).
fn declared_name(line: &str) -> Option<String> {
    let first = line.chars().next()?;
    if first.is_whitespace() {
        return None;
    }
    if let Some((_, ret)) = line.trim_end().strip_suffix('{')?.split_once("=>") {
        // Constructor: `Recv => Name {` — peel a fallible/async wrapper
        // down to the constructed type.
        let ret = ret.trim();
        let inner = ["Result<", "Option<", "Future<"]
            .iter()
            .find_map(|w| ret.strip_prefix(w))
            .unwrap_or(ret);
        return leading_type_name(inner);
    }
    None
}

/// The type an alias line declares: `Name = …` (but not `Name => …`).
fn aliased_name(line: &str) -> Option<String> {
    let name = leading_type_name(line)?;
    let rest = line[name.len()..].trim_start();
    let body = rest.strip_prefix('=')?;
    (!body.starts_with('>')).then_some(name)
}

#[test]
fn every_stdlib_type_is_in_the_reserved_names_appendix() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc = fs::read_to_string(root.join("docs/src/reference/stdlib.md"))
        .expect("stdlib.md should exist");
    let appendix = doc
        .split("## Reserved Names")
        .nth(1)
        .expect("stdlib.md should end with a `## Reserved Names` appendix");

    let mut files = Vec::new();
    collect_can_files(&root.join("packages/canon/src"), &mut files);
    assert!(
        files.len() > 30,
        "stdlib walk looks broken: {} files",
        files.len()
    );

    let mut declared: BTreeMap<String, PathBuf> = BTreeMap::new();
    for file in &files {
        let src = fs::read_to_string(file).expect("read stdlib source");
        for line in src.lines() {
            if let Some(name) = aliased_name(line).or_else(|| declared_name(line)) {
                declared.entry(name).or_insert_with(|| file.clone());
            }
        }
    }
    assert!(
        declared.len() > 200,
        "declaration extraction looks broken: {} names",
        declared.len()
    );

    for (name, file) in &declared {
        let listed = appendix
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|token| token == name);
        assert!(
            listed,
            "stdlib type `{}` (declared in {}) is missing from the Reserved \
             Names appendix — add it to the alphabetical list in \
             docs/src/reference/stdlib.md",
            name,
            file.strip_prefix(root).unwrap_or(file).display()
        );
    }
}
