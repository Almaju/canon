//! Vendored-dependency (`deps/`) resolution tests.
//!
//! Each subdirectory of `tests/deps/` is a committed miniature project:
//! a `main.can` entry plus a hand-vendored `deps/` tree. The `ok_*`
//! projects must load, check, and run; the `fail_*` projects must be
//! rejected by the loader with a message naming the specific rule they
//! break. Messages are asserted by substring (not goldens) because the
//! full text contains machine-specific absolute paths for some errors.
//!
//! None of these projects has an `canon.toml` — the `deps/` root falls
//! back to the entry file's directory, which is exactly the
//! manifest-free layout modules & packages (docs/src/spec/modules.md)
//! ends at.

mod common;

use canon::{checker, loader};
use common::*;
use std::path::PathBuf;

fn entry(project: &str) -> PathBuf {
    PathBuf::from("tests/deps").join(project).join("main.can")
}

/// Load a fail-project and return the loader's error message.
fn load_error(project: &str) -> String {
    match loader::load_module(&entry(project)) {
        Ok(_) => panic!(
            "`tests/deps/{}` loaded successfully, expected a loader error",
            project
        ),
        Err(err) => err.message().to_string(),
    }
}

#[test]
fn vendored_package_loads_checks_and_runs() {
    let loaded = loader::load_module(&entry("ok_basic")).expect("ok_basic should load");
    let errors = checker::check_with_entry(&loaded.module, loaded.entry_items_start);
    assert!(
        errors.is_empty(),
        "ok_basic should check cleanly, got: {:?}",
        errors
    );

    let out = run_canon_subcommand("run", &entry("ok_basic"), &[]);
    assert_eq!(
        out.exit_code,
        Some(0),
        "canon run failed.\nstdout:\n{}\nstderr:\n{}",
        out.stdout,
        out.stderr
    );
    assert_eq!(out.stdout, "hello!\n");
}

#[test]
fn missing_package_directive_is_rejected() {
    let msg = load_error("fail_missing_directive");
    assert!(
        msg.contains("is missing its `package` directive"),
        "unexpected message: {msg}"
    );
    assert!(
        msg.contains("deps/acme/greet/shout.can"),
        "message should name the vendored file: {msg}"
    );
}

#[test]
fn coordinate_must_match_deps_directory() {
    let msg = load_error("fail_wrong_dir");
    assert!(
        msg.contains("does not match its directory `deps/acme/greet/`"),
        "unexpected message: {msg}"
    );
}

#[test]
fn malformed_coordinate_is_rejected() {
    let msg = load_error("fail_malformed");
    assert!(
        msg.contains("malformed package coordinate `acme greet 1.0`"),
        "unexpected message: {msg}"
    );
}

#[test]
fn version_conflict_within_a_package_is_rejected() {
    let msg = load_error("fail_version_conflict");
    assert!(
        msg.contains("conflicting versions"),
        "unexpected message: {msg}"
    );
    assert!(
        msg.contains("`1.0.0`") && msg.contains("`1.1.0`"),
        "message should name both versions: {msg}"
    );
}

#[test]
fn deps_and_local_resolution_is_ambiguous() {
    let msg = load_error("fail_ambiguous");
    assert!(
        msg.contains("`shout` is ambiguous"),
        "unexpected message: {msg}"
    );
}

#[test]
fn formatter_round_trips_the_package_directive() {
    let src = "package \"acme:greet@1.0.0\"\n\nshout = (String) -> String {\n    String.concat(\"!\")\n}\n";
    let once = canon::formatter::format(src).expect("directive file should format");
    assert_eq!(once, src, "canonical source should be a fixpoint");
    assert!(
        once.starts_with("package \"acme:greet@1.0.0\"\n"),
        "directive must stay first: {once}"
    );
}
