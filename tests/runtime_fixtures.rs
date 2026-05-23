//! Fixture-based runtime tests.
//!
//! Walks `tests/runtime/*.ow` and runs each through the full pipeline
//! (lexer → parser → checker → codegen → runtime), comparing stdout
//! against a sibling golden `.stdout` file.
//!
//! This is the "does this program actually run and produce the right
//! output?" layer — complementary to `tests/checker_fixtures.rs`
//! (which only goes as far as the checker) and `tests/oneway/*` (which
//! uses the Oneway-language test framework with `TestResult`).
//!
//! Implementation note: we shell out to the freshly-built `oneway`
//! binary via `CARGO_BIN_EXE_oneway` rather than calling
//! `oneway::runtime::run_component` directly. The runtime inherits
//! stdio from its host process, so the only reliable way to capture
//! it is through a child process. Shelling out also matches how real
//! users invoke the compiler, which is a useful integration-test
//! property in its own right.
//!
//! Adding a new runtime fixture:
//!   1. Drop a `.ow` file into `tests/runtime/`.
//!   2. Run `just update-fixtures` (or set `ONEWAY_UPDATE_FIXTURES=1`)
//!      to generate the sibling `.stdout`.
//!   3. Review the generated file and commit both.

mod common;

use common::*;
use std::path::PathBuf;

#[test]
fn runtime_fixtures() {
    let dir = PathBuf::from("tests/runtime");
    let fixtures = collect_fixtures(&dir);
    assert!(
        !fixtures.is_empty(),
        "no fixtures found in `{}` — the harness should always have something to run",
        dir.display()
    );

    let mut failures: Vec<String> = Vec::new();
    for fixture in &fixtures {
        let output = run_oneway_subcommand("run", fixture, &[]);

        if output.exit_code != Some(0) {
            failures.push(format!(
                "{}: `oneway run` exited with {:?}\nstdout:\n{}\nstderr:\n{}",
                fixture.display(),
                output.exit_code,
                indent(&output.stdout, "  | "),
                indent(&output.stderr, "  | "),
            ));
            continue;
        }

        let golden = fixture.with_extension("stdout");
        if let Err(msg) = compare_or_update_golden(&golden, &output.stdout) {
            failures.push(msg);
        }
    }

    if !failures.is_empty() {
        panic!(
            "{}/{} runtime fixture(s) failed:\n\n{}",
            failures.len(),
            fixtures.len(),
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
