//! Harness for the Canon-language test suite.
//!
//! Invokes `canon test <dir>` once per test directory — the directory
//! batch mode compiles every `*_test.can` file and runs them all in a
//! single process, sharing the stdlib parse and the wasmtime engine
//! across files (see `cmd_test_dir` in `src/main.rs`). The directories
//! are `tests/canon` (the language) and every shipped package's `src/`
//! (its own tests, which `canon doc` shows as its examples). The
//! subprocess exits non-zero if any file fails to compile or reports a
//! failing test, and its combined output is included in the panic
//! message so the cause of a regression is obvious. Per-file iteration
//! is still available for local debugging via `canon test <file>`.
//!
//! This brings the Canon-language tests under the same `cargo test`
//! umbrella as the checker and runtime fixtures. `just test-can`
//! remains as a convenience wrapper for running them in isolation
//! with prettier output, but the canonical entrypoint is now
//! `cargo test`.
//!
//! Why shell out instead of calling the library? The Canon test
//! framework lives behind the `canon test` subcommand which
//! synthesises a `main`, runs codegen, and hands the resulting
//! component to the WASI runtime. Replicating that machinery
//! in-process would be substantially more code; spawning the
//! binary captures stdout the same way `tests/runtime_fixtures.rs`
//! already does, with the same CARGO_BIN_EXE_canon path resolution.

mod common;

use common::*;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn canon_test_files() {
    run_dir(Path::new("tests/canon"));
}

/// Every package under `packages/` holding a `*_test.can` in its
/// `src/`, found the way `build.rs` finds the packages: a directory
/// with `src/`, walked past everything but a package's own trees.
#[test]
fn package_test_files() {
    let mut dirs = Vec::new();
    collect_test_dirs(Path::new("packages"), &mut dirs);
    dirs.sort();
    assert!(dirs.len() > 3, "package walk looks broken: {:?}", dirs);
    for dir in dirs {
        run_dir(&dir);
    }
}

fn collect_test_dirs(dir: &Path, out: &mut Vec<PathBuf>) {
    let src = dir.join("src");
    if fs::read_dir(&src).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.ends_with("_test.can"))
        })
    }) {
        out.push(src);
    }
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir()
            && !name.starts_with('.')
            && !["src", "bindgen", "wit", "build", "deps"].contains(&name.as_ref())
        {
            collect_test_dirs(&path, out);
        }
    }
}

fn run_dir(dir: &Path) {
    let output = run_canon_subcommand("test", dir, &[]);

    // `[FAIL]` lines in stdout are the per-test failure signal; a
    // non-zero exit covers those plus compile failures (which print to
    // stderr). Either is a suite failure.
    let fail_lines: Vec<&str> = output
        .stdout
        .lines()
        .filter(|line| line.contains("[FAIL]"))
        .collect();

    if output.exit_code != Some(0) || !fail_lines.is_empty() {
        panic!(
            "`canon test {}` failed: exited with {:?}, {} test(s) reported [FAIL]\nstdout:\n{}\nstderr:\n{}",
            dir.display(),
            output.exit_code,
            fail_lines.len(),
            indent(&output.stdout, "  | "),
            indent(&output.stderr, "  | "),
        );
    }
}
