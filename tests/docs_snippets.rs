//! Compile guard for every Canon code block in the documentation.
//!
//! The docs runner (`docs/runner/build.mjs`) compiles only the seven
//! ` ```canon,run=… ` snippets; everything else was unchecked prose, and
//! it drifted — six of the fifteen commits before this test were
//! "docs: fix non-compiling example". This test closes that class: every
//! ` ```canon ` block in `docs/src/**/*.md` must pass `canon check`.
//!
//! A block that is a fragment (declarations without an entry point) is
//! fine: the one error `canon check` is allowed to report is the missing
//! entry. Anything else — parse error, unknown type, non-canonical
//! format — fails the build with the page and block line number.
//!
//! A block quoted verbatim from a `.can` file under `examples/` or
//! `packages/` is exempt from the standalone check: it belongs to a
//! package that already compiles as a whole (`just examples`, the
//! bundled stdlib), and cross-file references make it unfair to check
//! alone. `tests/docs_examples_inline.rs` pins those quotes verbatim.
//!
//! Blocks that are deliberately not Canon (annotated schematics, retired
//! syntax shown for contrast) opt out by using a ` ```text ` fence, which
//! also stops the site from syntax-highlighting them as Canon.
//!
//! The scratch project the snippets check in has every shipped package
//! vendored (`canon add`, the same way a reader would), so a page may
//! use any of them — the docs describe the ecosystem, not the prelude
//! alone.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Snippet {
    page: String,
    line: usize,
    body: String,
}

fn collect_can_sources(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_can_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "can") {
            if let Ok(src) = fs::read_to_string(&path) {
                out.push(src);
            }
        }
    }
}

fn collect_md_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_md_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

fn extract_canon_blocks(page: &str, text: &str) -> Vec<Snippet> {
    let mut snippets = Vec::new();
    let mut in_block = false;
    let mut is_canon = false;
    let mut start = 0usize;
    let mut body = String::new();
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if !in_block {
            if let Some(info) = trimmed.strip_prefix("```") {
                in_block = true;
                let info = info.trim();
                is_canon = info == "canon" || info.starts_with("canon,");
                start = idx + 1;
                body.clear();
            }
        } else if trimmed.starts_with("```") {
            in_block = false;
            if is_canon {
                snippets.push(Snippet {
                    page: page.to_string(),
                    line: start,
                    body: body.clone(),
                });
            }
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    snippets
}

/// A snippet passes when `canon check` accepts it outright, or when the
/// only complaint is the missing entry point (the snippet is a fragment).
fn check_failure(canon: &Path, dir: &Path, snippet: &Snippet, n: usize) -> Option<String> {
    let file = dir.join(format!("snippet_{n}.can"));
    fs::write(&file, &snippet.body).expect("write snippet");
    let out = Command::new(canon)
        .arg("check")
        .arg(&file)
        .output()
        .expect("run canon check");
    if out.status.success() {
        return None;
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let real: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("error["))
        .filter(|l| !l.contains("no entry point defined"))
        .collect();
    if real.is_empty() {
        return None;
    }
    Some(format!(
        "{}:{} —\n    {}",
        snippet.page,
        snippet.line,
        real.join("\n    ")
    ))
}

#[test]
fn every_docs_canon_block_checks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let canon = PathBuf::from(env!("CARGO_BIN_EXE_canon"));
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("docs_snippets");
    fs::create_dir_all(&scratch).expect("create scratch dir");
    for pkg in canon::add::addable() {
        canon::add::add(&scratch, pkg.name).expect("vendor a shipped package");
    }

    let mut pages = Vec::new();
    collect_md_files(&root.join("docs/src"), &mut pages);
    pages.sort();
    assert!(
        pages.len() > 30,
        "docs walk looks broken: {} pages",
        pages.len()
    );

    let mut snippets = Vec::new();
    for page in &pages {
        let text = fs::read_to_string(page).expect("read page");
        let rel = page.strip_prefix(root).unwrap().display().to_string();
        snippets.extend(extract_canon_blocks(&rel, &text));
    }
    assert!(
        snippets.len() > 50,
        "snippet extraction looks broken: {} blocks",
        snippets.len()
    );

    let mut corpus = Vec::new();
    collect_can_sources(&root.join("examples"), &mut corpus);
    collect_can_sources(&root.join("packages"), &mut corpus);

    let failures: Vec<String> = snippets
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            let quoted = s.body.trim();
            !corpus.iter().any(|src| src.contains(quoted))
        })
        .filter_map(|(n, s)| check_failure(&canon, &scratch, s, n))
        .collect();
    assert!(
        failures.is_empty(),
        "{} documentation code block(s) do not check:\n\n{}\n\n\
         Fix the Canon, or retag a deliberate non-program as ```text.",
        failures.len(),
        failures.join("\n\n")
    );
}
