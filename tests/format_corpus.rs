//! Every checked-in `.can` file must be canonically formatted.
//!
//! `canon check/build/run` enforce formatting for user sources, but the
//! bundled stdlib is exempt at runtime (it ships inside the binary), and
//! examples are not part of the fixture harnesses. This test closes the
//! gap: `format(src) == src` for the whole corpus, so a drifted file
//! fails `cargo test` (and CI) instead of shipping.
//!
//! `tests/checker/fail/` is deliberately excluded — those fixtures stay
//! non-canonical on purpose (unsorted arms, ordering violations) so the
//! checker errors they pin keep firing.

use std::path::{Path, PathBuf};

const CANONICAL_TREES: &[&str] = &[
    "packages",
    "examples",
    "tests/canon",
    "tests/runtime",
    "tests/checker/ok",
];

fn collect_can_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
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

#[test]
fn corpus_is_canonically_formatted() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for tree in CANONICAL_TREES {
        collect_can_files(&root.join(tree), &mut files);
    }
    assert!(
        files.len() > 50,
        "corpus walk looks broken: only {} .can files found",
        files.len()
    );
    files.sort();

    let mut drifted = Vec::new();
    for path in &files {
        let src = std::fs::read_to_string(path).expect("read corpus file");
        match canon::formatter::format(&src) {
            Ok(formatted) if formatted == src => {}
            Ok(_) => drifted.push(format!("{}: not canonically formatted", path.display())),
            Err(e) => drifted.push(format!("{}: does not parse: {}", path.display(), e)),
        }
    }
    assert!(
        drifted.is_empty(),
        "{} corpus file(s) drifted from canonical format (run `just fmt-can`):\n  {}",
        drifted.len(),
        drifted.join("\n  ")
    );
}
