//! Harness for the Canon-language test suite.
//!
//! Walks `tests/canon/*_test.can`, invokes `canon test <file>` on each
//! one, and asserts that no test in the file reports `[FAIL]`. Each
//! file's output is included verbatim in the failure report so the
//! cause of a regression is obvious from the panic message.
//!
//! This brings the Canon-language tests under the same `cargo test`
//! umbrella as the checker and runtime fixtures. `just test-ow`
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
use std::path::PathBuf;

#[test]
fn canon_test_files() {
    let dir = PathBuf::from("tests/canon");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("could not read `{}`: {}", dir.display(), e))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("can")
                && p.file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.ends_with("_test.can"))
                    .unwrap_or(false)
        })
        .collect();
    files.sort();

    assert!(
        !files.is_empty(),
        "no `*_test.can` files found under `{}` — add at least one Canon test",
        dir.display()
    );

    let mut failures: Vec<String> = Vec::new();
    for test_file in &files {
        let output = run_canon_subcommand("test", test_file, &[]);

        if output.exit_code != Some(0) {
            failures.push(format!(
                "{}: `canon test` exited with {:?}\nstdout:\n{}\nstderr:\n{}",
                test_file.display(),
                output.exit_code,
                indent(&output.stdout, "  | "),
                indent(&output.stderr, "  | "),
            ));
            continue;
        }

        // Currently the runtime always exits 0; `[FAIL]` lines in stdout
        // are the failure signal. Once exit-code threading lands this
        // check becomes redundant but harmless.
        let fail_lines: Vec<&str> = output
            .stdout
            .lines()
            .filter(|line| line.contains("[FAIL]"))
            .collect();
        if !fail_lines.is_empty() {
            failures.push(format!(
                "{}: {} test(s) reported [FAIL]:\n{}\nfull output:\n{}",
                test_file.display(),
                fail_lines.len(),
                indent(&fail_lines.join("\n"), "  | "),
                indent(&output.stdout, "  | "),
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "{}/{} Canon test file(s) failed:\n\n{}",
            failures.len(),
            files.len(),
            failures.join("\n\n"),
        );
    }
}

fn indent(s: &str, prefix: &str) -> String {
    s.lines()
        .map(|line| format!("{}{}", prefix, line))
        .collect::<Vec<_>>()
        .join("\n")
}
