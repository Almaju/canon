//! End-to-end pins for the scalar `option<T>` / `list<T>` extern-return
//! shapes (`IndirectReturnShape::OptionScalar` / `ListScalar`).
//!
//! The embedded runtime has no host implementing a freestanding
//! `option<scalar>` return, so these tests pin the *build* half: the
//! program compiles to a component that passes wasmparser validation
//! (codegen aborts the process on invalid wasm, so a zero-exit
//! `canon build` subprocess is the assertion — this is exactly where
//! the old "type-checks but fails at build" gap used to bite). The
//! runtime half of the scalar-list decode is covered by
//! `tests/canon/random_test.can`, which calls `wasi:random`'s
//! `get-random-bytes` (`list<u8>`) through the real wasmtime host and
//! asserts on the decoded elements.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

/// Build a unique tmpdir under `target/extern-shape-tmp/<name>` (same
/// pattern as `tests/install_test.rs` — `target/` is gitignored and a
/// per-test subdirectory avoids cross-process collisions).
fn tmpdir(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target");
    p.push("extern-shape-tmp");
    p.push(name);
    if p.exists() {
        fs::remove_dir_all(&p).expect("could not clean tmpdir");
    }
    fs::create_dir_all(&p).expect("could not create tmpdir");
    p
}

/// Stand up a minimal project: a manifest (which marks the project
/// root so the loader consults `bindgen/`), one hand-written binding
/// file in the versioned-package layout, and a `main.can`.
fn write_project(root: &Path, binding_rel: &str, binding_src: &str, main_src: &str) {
    fs::write(
        root.join("canon.toml"),
        "name = \"shape-test\"\nversion = \"0.1.0\"\n",
    )
    .expect("write manifest");
    let binding = root.join(binding_rel);
    fs::create_dir_all(binding.parent().unwrap()).expect("create binding dir");
    fs::write(binding, binding_src).expect("write binding");
    fs::write(root.join("main.can"), main_src).expect("write main");
}

fn assert_builds(root: &Path) {
    let out = common::run_canon_subcommand("build", &root.join("main.can"), &[]);
    assert_eq!(
        out.exit_code,
        Some(0),
        "`canon build` failed:\nstdout: {}\nstderr: {}",
        out.stdout,
        out.stderr
    );
    assert!(
        root.join("build").join("main").join("main.wasm").exists(),
        "build should produce main.wasm"
    );
}

/// A binding returning `option<s64>` compiles and the emitted component
/// validates: the OptionScalar indirect return gets a real
/// component-level `option<s64>` import type and a decode into a Canon
/// `Option` struct that union dispatch can consume.
#[test]
fn option_scalar_return_builds() {
    let root = tmpdir("option_scalar");
    write_project(
        &root,
        "bindgen/demo/opt@0.1.0/opt.can",
        "MaybeValue = Option<Int>\n\nUnit => MaybeValue {\n    \"maybe-value\"\n}\n",
        "Unit => Program {\n    MaybeValue() -> (\n        * None => Unit { \"none\" -> Print }\n        * Some<Int> => Unit { Int -> Print }\n    )\n}\n",
    );
    assert_builds(&root);
}

/// `?` on a scalar-option binding return compiles: the decoded value is
/// a plain Canon `Option` struct, so the payload extraction reads the
/// i64 slot at +4 like any user-constructed Option.
#[test]
fn option_scalar_try_extraction_builds() {
    let root = tmpdir("option_scalar_try");
    write_project(
        &root,
        "bindgen/demo/opt@0.1.0/opt.can",
        "MaybeValue = Option<Int>\n\nUnit => MaybeValue {\n    \"maybe-value\"\n}\n",
        "Unit => Program {\n    MaybeValue()? -> Print\n}\n",
    );
    assert_builds(&root);
}

/// A binding returning `list<s64>` (a 64-bit element, the stride that
/// matches Canon's own list layout) compiles and validates — the
/// ListScalar fast path.
#[test]
fn list_wide_scalar_return_builds() {
    let root = tmpdir("list_wide_scalar");
    write_project(
        &root,
        "bindgen/demo/vals@0.1.0/vals.can",
        // Not named `Values` — the stdlib map's `Values` would collide.
        "HostValues = List<Int>\n\nUnit => HostValues {\n    \"values\"\n}\n",
        "Unit => Program {\n    HostValues()\n        -> Length\n        -> Print\n}\n",
    );
    assert_builds(&root);
}

/// The narrow-element path (`list<u8>`, stride read back per-width from
/// the vendored WIT): `get-random-bytes` resolves from the bundled
/// stdlib bindings and the program builds. Runtime behaviour is pinned
/// by `tests/canon/random_test.can`.
#[test]
fn list_narrow_scalar_return_builds() {
    let root = tmpdir("list_narrow_scalar");
    fs::write(
        root.join("main.can"),
        "Unit => Program {\n    GetRandomBytes(4)\n        -> Length\n        -> Print\n}\n",
    )
    .expect("write main");
    assert_builds(&root);
}

/// A WIT bare `result;` return (no ok/err payloads) compiles: the
/// single-i32 discriminant is boxed into a Canon `Result` struct, the
/// component-level import type is a real `result<_, _>`, and Err/Ok
/// dispatch consumes it like any user-constructed Result. Validation
/// would fail if the lowered core signature disagreed with the
/// component type — exactly the old gap.
#[test]
fn bare_result_return_builds() {
    let root = tmpdir("bare_result");
    write_project(
        &root,
        "bindgen/demo/sync@0.1.0/sync.can",
        "Synced = Unit\n\nUnit => Result<Synced, Unit> {\n    \"sync-all\"\n}\n",
        "Unit => Program {\n    Synced() -> (\n        * Err => Unit { \"failed\" -> Print }\n        * Ok => Unit { \"synced\" -> Print }\n    )\n}\n",
    );
    assert_builds(&root);
}
