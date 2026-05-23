//! Shared helpers for fixture-based integration tests.
//!
//! Tests in `tests/checker_fixtures.rs` (and friends) walk a directory
//! of `.ow` source files and assert one of two outcomes per fixture:
//!
//!   * Files under `tests/fixtures/<phase>/ok/` must compile cleanly
//!     (the chosen pipeline phase produces no errors).
//!   * Files under `tests/fixtures/<phase>/fail/` must produce errors
//!     that match a sibling golden `.stderr` file exactly.
//!
//! The golden file approach is borrowed from Rust's `trybuild`: when
//! the error format changes intentionally, re-run the test suite with
//! `ONEWAY_UPDATE_FIXTURES=1` to rewrite every `.stderr` file from the
//! actual output. The diff in `git status` then becomes the review
//! surface for "did the error wording change in a sensible way?".
//!
//! Errors are formatted as a single line per error, in the same shape
//! `oneway check` emits to stderr:
//!
//! ```text
//! error[<fixture-path>:<line>:<column>]: <message>
//! ```
//!
//! The path component is the fixture path *as passed to the harness*
//! (a workspace-relative path), so output is portable across machines.
//!
//! NOTE: this file lives in `tests/common/mod.rs` so it can be imported
//! by multiple integration-test binaries via `mod common;`. The standard
//! `tests/foo.rs` layout treats each top-level file as a separate crate,
//! so a shared module needs the `common/` subdirectory shape to avoid
//! cargo creating a separate `common` test binary.
#![allow(dead_code)]

use oneway::checker;
use oneway::error::OnewayError;
use oneway::loader;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Path to the `oneway` binary built for this test run. Cargo populates
/// `CARGO_BIN_EXE_<name>` when compiling integration tests so the harness
/// always invokes the exact build artefact the test belongs to (no PATH
/// lookup, no need to `cargo install`).
pub fn oneway_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_oneway"))
}

/// Runs the lexer + parser + checker on a fixture file, returning the
/// formatted error output (one error per line) — or an empty string
/// when the fixture checks cleanly.
///
/// Uses the real `loader::load_module`, so fixtures may `use std/...`
/// and exercise the same import machinery `oneway check` does.
pub fn run_check_fixture(fixture_path: &Path) -> String {
    let display_path = fixture_display_path(fixture_path);
    match loader::load_module(fixture_path) {
        Ok(loaded) => {
            let errors = checker::check_with_entry(&loaded.module, loaded.entry_items_start);
            format_errors(&display_path, &errors)
        }
        Err(err) => format_single_error(&display_path, &err),
    }
}

/// Captured result of running an `oneway` subcommand on a fixture file.
pub struct RunOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// Invokes the `oneway` binary on a fixture with the given subcommand
/// (e.g. `"run"`, `"test"`, `"check"`) and returns its captured output.
///
/// The fixture path is passed as the first positional argument; any
/// additional arguments are appended in order.
pub fn run_oneway_subcommand(subcommand: &str, fixture: &Path, extra_args: &[&str]) -> RunOutput {
    let mut cmd = Command::new(oneway_binary());
    cmd.arg(subcommand).arg(fixture);
    for a in extra_args {
        cmd.arg(a);
    }
    // Don't propagate the update flag into the subprocess. The harness
    // *invoking* the subprocess interprets it (to write goldens); the
    // subprocess itself should run with clean defaults.
    cmd.env_remove("ONEWAY_UPDATE_FIXTURES");

    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn `oneway`: {}", e));
    RunOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code(),
    }
}

/// Walks a fixture directory, returning every `.ow` file found, sorted
/// alphabetically so failure reports are stable run-to-run.
pub fn collect_fixtures(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("could not read fixture dir `{}`: {}", dir.display(), e))
        .filter_map(|entry| entry.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("ow"))
        .collect();
    files.sort();
    files
}

/// Compares actual error output against a golden `.stderr` file.
///
/// Returns `Ok(())` on match. Returns an `Err(String)` describing the
/// mismatch otherwise — including the diff-style "expected vs actual"
/// block that the harness prints in its panic message.
///
/// When the env var `ONEWAY_UPDATE_FIXTURES` is set, this *writes*
/// `actual` to `golden_path` instead of comparing, and returns `Ok`.
/// The convention matches `trybuild`'s `TRYBUILD=overwrite`.
pub fn compare_or_update_golden(
    golden_path: &Path,
    actual: &str,
) -> std::result::Result<(), String> {
    if std::env::var_os("ONEWAY_UPDATE_FIXTURES").is_some() {
        fs::write(golden_path, actual).map_err(|e| {
            format!(
                "could not write golden file `{}`: {}",
                golden_path.display(),
                e
            )
        })?;
        return Ok(());
    }

    let expected = match fs::read_to_string(golden_path) {
        Ok(s) => s,
        Err(_) => {
            return Err(format!(
                "missing golden file `{}`\n\
                 run the test suite with ONEWAY_UPDATE_FIXTURES=1 to create it.\n\
                 actual output:\n{}",
                golden_path.display(),
                indent(actual, "  | "),
            ));
        }
    };

    if normalize(&expected) == normalize(actual) {
        return Ok(());
    }

    Err(format!(
        "golden mismatch for `{}`\n\
         expected:\n{}\n\
         actual:\n{}\n\
         to accept this change, re-run with ONEWAY_UPDATE_FIXTURES=1",
        golden_path.display(),
        indent(&expected, "  | "),
        indent(actual, "  | "),
    ))
}

/// Convert a filesystem path to the display form used inside error
/// messages and golden files. We strip the workspace prefix so the
/// `.stderr` is portable across checkouts.
fn fixture_display_path(p: &Path) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match p.strip_prefix(&cwd) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => p.display().to_string(),
    }
}

fn format_errors(display_path: &str, errors: &[OnewayError]) -> String {
    let mut out = String::new();
    for err in errors {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format_one(display_path, err));
    }
    out
}

fn format_single_error(display_path: &str, err: &OnewayError) -> String {
    format_one(display_path, err)
}

fn format_one(display_path: &str, err: &OnewayError) -> String {
    let span = err.span();
    format!(
        "error[{}:{}:{}]: {}",
        display_path,
        span.line,
        span.column,
        err.message()
    )
}

/// Trim trailing whitespace on every line + drop the final newline,
/// so cosmetic differences don't fail tests. A trailing newline in
/// the golden file is benign; we never produce one in actual output.
fn normalize(s: &str) -> String {
    s.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

fn indent(s: &str, prefix: &str) -> String {
    s.lines()
        .map(|line| format!("{}{}", prefix, line))
        .collect::<Vec<_>>()
        .join("\n")
}
