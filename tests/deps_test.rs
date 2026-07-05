//! Vendored-dependency (`deps/`) resolution tests.
//!
//! Each subdirectory of `tests/deps/` is a committed miniature project:
//! a `main.can` entry plus a hand-vendored `deps/` tree. The `ok_*`
//! projects must load, check, and run; the `fail_*` projects must be
//! rejected by the loader with a message naming the specific rule they
//! break. Messages are asserted by substring (not goldens) because the
//! full text contains machine-specific absolute paths for some errors.
//!
//! The vendored layout is the path-carried one: a package occupies
//! `deps/<ns>/<name>@<version>/`, the directory name is the pin, and
//! the files are pure source — no `package` directive exists (the
//! keyword left the language with slice 7). Binding files are
//! recognized by shape: body-less camelCase declarations in a file
//! directly under the package directory bind to the WIT interface the
//! path spells (`ok_bindings` pins that end-to-end).
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
fn path_derived_bindings_load_and_run() {
    // `deps/canon/builtins@0.1.0/math.can` holds a body-less camelCase
    // declaration and nothing else — no `bindings` directive. The
    // loader derives `canon:builtins/math@0.1.0#min` from the path
    // alone and the host builtin satisfies it at run time.
    let loaded = loader::load_module(&entry("ok_bindings")).expect("ok_bindings should load");
    let min = loaded
        .module
        .items
        .iter()
        .find_map(|item| match item {
            canon::ast::Item::Function(f) if f.name.name == "min" => f.extern_wasm.as_ref(),
            _ => None,
        })
        .expect("`min` should load as an extern binding");
    assert_eq!(
        min.path, "canon:builtins/math@0.1.0#min",
        "the binding URN must be derived from the vendored path"
    );

    let out = run_canon_subcommand("run", &entry("ok_bindings"), &[]);
    assert_eq!(
        out.exit_code,
        Some(0),
        "canon run failed.\nstdout:\n{}\nstderr:\n{}",
        out.stdout,
        out.stderr
    );
    assert_eq!(out.stdout, "3\n");
}

#[test]
fn unversioned_package_dir_is_rejected() {
    let msg = load_error("fail_unversioned");
    assert!(
        msg.contains("missing its version"),
        "unexpected message: {msg}"
    );
    assert!(
        msg.contains("deps/acme/greet@<version>/"),
        "message should show the expected versioned shape: {msg}"
    );
}

#[test]
fn malformed_version_is_rejected() {
    let msg = load_error("fail_malformed");
    assert!(
        msg.contains("malformed vendored package directory"),
        "unexpected message: {msg}"
    );
    assert!(
        msg.contains("deps/acme/greet@1.0_beta/"),
        "message should name the offending directory: {msg}"
    );
}

/// Load a fail-project whose function-only name collision passes the
/// loader (constructor/shape families may co-declare a function name —
/// DESIGN.md § Types-Only Canon, resolution rule 4) and return the
/// checker's error messages. The conflict is caught by the checker's
/// duplicate-definition guard because the colliding implementations
/// share a receiver and first input, which no family may.
fn check_errors(project: &str) -> Vec<String> {
    let loaded = loader::load_module(&entry(project))
        .unwrap_or_else(|err| panic!("`tests/deps/{}` should load: {}", project, err.message()));
    checker::check_with_entry(&loaded.module, loaded.entry_items_start)
        .iter()
        .map(|e| e.message().to_string())
        .collect()
}

#[test]
fn two_vendored_versions_are_rejected() {
    // Two versioned siblings both declare `Shouted` with the same
    // signature. Co-declaring a function name is legal (families), but
    // two implementations on the same receiver with the same first
    // input are a duplicate — the checker names the collision (install
    // removes old versions, so this only arises from manual tampering).
    let msgs = check_errors("fail_two_versions");
    assert!(
        msgs.iter()
            .any(|m| m.contains("duplicate function `Shouted` on `String`")),
        "expected a duplicate-function error, got: {msgs:?}"
    );
}

#[test]
fn deps_and_local_resolution_is_ambiguous() {
    // A local file and a vendored dep both implement `Shouted` on
    // `String`. Both load (function names may co-resolve); the
    // duplicate-definition guard reports the actual conflict.
    let msgs = check_errors("fail_ambiguous");
    assert!(
        msgs.iter()
            .any(|m| m.contains("duplicate function `Shouted` on `String`")),
        "expected a duplicate-function error, got: {msgs:?}"
    );
}
