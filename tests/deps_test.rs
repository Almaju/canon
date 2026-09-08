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
//! recognized by shape: string-anchored constructors in a file
//! directly under the package directory bind to the WIT interface the
//! path spells (`ok_bindings` pins that end-to-end).
//!
//! Each project is nothing but files — a `deps/` tree next to the
//! entry is the whole declaration, which is exactly the layout modules
//! & packages (docs/src/spec/modules.md) specifies.

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
    // `deps/wasi/random@0.3.0-rc-2026-03-15/random.can` holds a
    // string-anchored constructor and nothing else — no `bindings`
    // directive. The loader derives the
    // `wasi:random/random@0.3.0-rc-2026-03-15` URN from the path alone
    // and reads the `get-random-u64` fragment from the constructor's
    // string body.
    //
    // The target is a real WASI interface rather than a bespoke host
    // shim, so this pins the whole path end to end: derived URN, lowered
    // import, and a live call through the embedded runtime. `main.can`
    // multiplies the draw by zero, which keeps stdout deterministic
    // while still requiring the import to resolve — a missing one is an
    // instantiation failure, not a wrong number.
    let loaded = loader::load_module(&entry("ok_bindings")).expect("ok_bindings should load");
    let seed = loaded
        .module
        .items
        .iter()
        .find_map(|item| match item {
            canon::ast::Item::Function(f)
                if f.receiver.as_ref().is_some_and(|r| r.name == "Seed") =>
            {
                f.extern_wasm.as_ref()
            }
            _ => None,
        })
        .expect("`Seed` should load as an extern binding");
    assert_eq!(
        seed.path, "wasi:random/random@0.3.0-rc-2026-03-15#get-random-u64",
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
    assert_eq!(out.stdout, "0\n");
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
            .any(|m| m.contains("duplicate constructor: `Shouted` already has a constructor whose first input is `String`")),
        "expected a duplicate-function error, got: {msgs:?}"
    );
}

#[test]
fn a_binding_hiding_a_wit_stream_is_rejected() {
    // Canon has no surface for `stream` or `future`, so a binding to
    // `wasi:cli/stdout`'s `func(data: stream<u8>) -> future<…>` is
    // spelled `String => Result<Piped, IoError>` and reads as an
    // ordinary string function. Codegen would type the import from that
    // signature and the component would fail to instantiate against a
    // host carrying the real shape — after passing both check and
    // build. The vendored WIT is the only place the shape is visible,
    // so the gap is caught there.
    let msgs = check_errors("fail_stream_binding");
    assert!(
        msgs.iter()
            .any(|m| m.contains("has a `stream` or `future` in its WIT signature")),
        "expected the stream-shape gap error, got: {msgs:?}"
    );
}

#[test]
fn a_local_type_shadows_a_vendored_one() {
    // A local file and a vendored dep both declare `Shouted`. The
    // project's declaration wins unqualified; the dep's is reached as
    // `greet.Shouted` — the package's name, a dot, the type.
    let out = run_canon_subcommand("run", &entry("ok_shadowed"), &[]);
    assert_eq!(
        out.exit_code,
        Some(0),
        "canon run failed.\nstdout:\n{}\nstderr:\n{}",
        out.stdout,
        out.stderr
    );
    assert_eq!(out.stdout, "hello?\nhello!\n");
}

#[test]
fn two_vendored_packages_declaring_a_type_need_qualification() {
    // Two dependencies declare `Shouted` and the project declares none:
    // there is no precedence between packages, so the plain name is
    // ambiguous and the error names both qualified spellings…
    let msg = load_error("fail_two_packages");
    assert!(
        msg.contains("`Shouted` is ambiguous")
            && msg.contains("`greet.Shouted`")
            && msg.contains("`loud.Shouted`"),
        "expected the two qualified spellings, got: {msg}"
    );
    // …and each qualified spelling reaches its own package.
    let out = run_canon_subcommand("run", &entry("ok_two_packages"), &[]);
    assert_eq!(
        out.exit_code,
        Some(0),
        "canon run failed.\nstdout:\n{}\nstderr:\n{}",
        out.stdout,
        out.stderr
    );
    assert_eq!(out.stdout, "hello!\nhello!!\n");
}
