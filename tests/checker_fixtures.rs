//! Fixture-based checker tests.
//!
//! Two test functions walk `tests/checker/{ok,fail}/` and enforce one
//! expectation per fixture. The `ok` set must compile cleanly; the
//! `fail` set must produce stderr that matches a sibling `.stderr`
//! golden file exactly.
//!
//! Adding a new positive test: drop an `.can` file into `ok/`.
//! Adding a new negative test:
//!   1. Drop an `.can` file into `fail/`.
//!   2. Run `CANON_UPDATE_FIXTURES=1 cargo test --test checker_fixtures`
//!      (or `just update-fixtures`) to generate the golden `.stderr`.
//!   3. Review the generated `.stderr` and commit both files.
//!
//! Updating error wording: same flow — re-run with the env var set
//! and review the diff to all affected `.stderr` files.

mod common;

use common::*;
use std::path::PathBuf;

#[test]
fn checker_ok_fixtures() {
    let dir = PathBuf::from("tests/checker/ok");
    let fixtures = collect_fixtures(&dir);
    assert!(
        !fixtures.is_empty(),
        "no fixtures found in `{}` — the harness should always have something to run",
        dir.display()
    );

    let mut failures: Vec<String> = Vec::new();
    for fixture in &fixtures {
        let actual = run_check_fixture(fixture);
        if !actual.is_empty() {
            failures.push(format!(
                "{} unexpectedly produced errors:\n{}",
                fixture.display(),
                indent(&actual, "  | "),
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "{}/{} ok-fixture(s) failed:\n\n{}",
            failures.len(),
            fixtures.len(),
            failures.join("\n\n"),
        );
    }
}

#[test]
fn checker_fail_fixtures() {
    let dir = PathBuf::from("tests/checker/fail");
    let fixtures = collect_fixtures(&dir);
    assert!(
        !fixtures.is_empty(),
        "no fixtures found in `{}` — the harness should always have something to run",
        dir.display()
    );

    let mut failures: Vec<String> = Vec::new();
    for fixture in &fixtures {
        let actual = run_check_fixture(fixture);
        if actual.is_empty() {
            failures.push(format!(
                "{}: expected the checker to report errors, but it accepted the program",
                fixture.display(),
            ));
            continue;
        }
        let golden = fixture.with_extension("stderr");
        if let Err(msg) = compare_or_update_golden(&golden, &actual) {
            failures.push(msg);
        }
    }

    if !failures.is_empty() {
        panic!(
            "{}/{} fail-fixture(s) failed:\n\n{}",
            failures.len(),
            fixtures.len(),
            failures.join("\n\n"),
        );
    }
}

/// Local indent helper — duplicates the one in `common::mod` to keep
/// each integration-test binary self-contained (cargo treats every
/// `tests/*.rs` as a separate crate, and `common::indent` is private
/// to that module).
fn indent(s: &str, prefix: &str) -> String {
    s.lines()
        .map(|line| format!("{}{}", prefix, line))
        .collect::<Vec<_>>()
        .join("\n")
}
