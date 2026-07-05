//! End-to-end tests for `canon install`.
//!
//! Each test stands up a temporary project directory containing an
//! `canon.toml` (with an `[imports]` table) and one or more vendored
//! `.wit` files, runs `canon::install::install` against the project
//! root, and asserts on what landed under `bindgen/`.
//!
//! These tests don't exercise the loader yet — that's slice 2b. Here we
//! just verify the materialization step in isolation.

use std::fs;
use std::path::{Path, PathBuf};

use canon::install;
use canon::install::EnsureOutcome;
use canon::loader;

/// Build a unique tmpdir under `target/install-test-tmp/<name>`. The
/// `target/` directory is already gitignored by Cargo, and using a
/// fresh subdirectory per test avoids the cross-process flakiness that
/// `std::env::temp_dir()` can produce when two test binaries collide.
fn tmpdir(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target");
    p.push("install-test-tmp");
    p.push(name);
    if p.exists() {
        fs::remove_dir_all(&p).expect("could not clean tmpdir");
    }
    fs::create_dir_all(&p).expect("could not create tmpdir");
    p
}

/// Copy a vendored WIT fixture into the project under `vendor/<name>.wit`.
fn vendor_wit(project_root: &Path, fixture_relative: &str, dest_name: &str) {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("wit")
        .join(fixture_relative);
    let dst_dir = project_root.join("vendor");
    fs::create_dir_all(&dst_dir).expect("create vendor dir");
    let dst = dst_dir.join(dest_name);
    fs::copy(&src, &dst).expect("copy fixture");
}

fn write_manifest(project_root: &Path, contents: &str) {
    fs::write(project_root.join("canon.toml"), contents).expect("write manifest");
}

#[test]
fn install_emits_one_file_per_interface() {
    let root = tmpdir("monotonic_clock");
    vendor_wit(&root, "monotonic-clock.wit", "monotonic-clock.wit");
    write_manifest(
        &root,
        r#"
name = "my-app"
version = "0.1.0"

[imports]
"wasi/clocks" = "vendor/monotonic-clock.wit"
"#,
    );

    let outcome = install::install(&root).expect("install should succeed");

    // The fixture defines a single `monotonic-clock` interface within
    // the `wasi:clocks` package. Expect one binding file at the
    // snake-cased path plus the install index sidecar.
    let expected_binding = root.join("bindgen/wasi/clocks@0.3.0-rc-2026-03-15/monotonic_clock.can");
    let expected_index = root.join("bindgen/_install.toml");
    assert!(
        expected_binding.exists(),
        "expected `{}` to be written; got {:?}",
        expected_binding.display(),
        outcome.written,
    );
    assert!(
        expected_index.exists(),
        "expected install index at `{}`",
        expected_index.display(),
    );
    assert_eq!(outcome.written.len(), 2);
    assert!(outcome.written.contains(&expected_binding));
    assert!(outcome.written.contains(&expected_index));

    let content = fs::read_to_string(&expected_binding).expect("read binding");
    // The binding file is pure source: bare function-type aliases, no
    // header. The versioned directory name carries the interface's
    // package and version, and the loader derives each declaration's
    // URN from that path (a binding file is recognized by shape).
    assert!(
        !content.contains("bindings \""),
        "no bindings header should be emitted, got:\n{content}",
    );
    assert!(
        !content.contains("extern Wasm"),
        "bindgen should not emit per-function `extern Wasm`, got:\n{content}",
    );
    assert!(content.contains("now = () -> Instant"));
    assert!(content.contains("getResolution = () -> Duration"));

    // The index sidecar should map this file to the correct URN.
    let index_content = fs::read_to_string(&expected_index).expect("read index");
    assert!(index_content.contains("\"wasi/clocks@0.3.0-rc-2026-03-15/monotonic_clock.can\""));
    assert!(index_content.contains("wasi:clocks/monotonic-clock@"));
}

#[test]
fn install_rejects_mismatched_manifest_key() {
    let root = tmpdir("mismatched_key");
    vendor_wit(&root, "monotonic-clock.wit", "monotonic-clock.wit");
    write_manifest(
        &root,
        r#"
name = "my-app"
version = "0.1.0"

[imports]
"wasi/random" = "vendor/monotonic-clock.wit"
"#,
    );

    let err = install::install(&root).expect_err("expected manifest-key mismatch");
    let msg = err.to_string();
    // Error should name both the key (`wasi/random`) and the actual
    // interface path the WIT produced (`wasi/clocks/monotonic_clock`).
    assert!(
        msg.contains("wasi/random") && msg.contains("monotonic_clock"),
        "error message should name both the manifest key and the offending interface path; got: {msg}",
    );
}

#[test]
fn install_accepts_broader_prefix_key() {
    // A key of `"wasi"` is a valid prefix for `wasi/clocks/...`, so
    // installing the monotonic-clock fixture under that broader key
    // should succeed and produce a file at `bindgen/wasi/clocks/...`.
    let root = tmpdir("broad_prefix_key");
    vendor_wit(&root, "monotonic-clock.wit", "monotonic-clock.wit");
    write_manifest(
        &root,
        r#"
name = "my-app"
version = "0.1.0"

[imports]
"wasi" = "vendor/monotonic-clock.wit"
"#,
    );

    let outcome = install::install(&root).expect("install with broad prefix should succeed");
    let expected = root.join("bindgen/wasi/clocks@0.3.0-rc-2026-03-15/monotonic_clock.can");
    assert!(
        expected.exists(),
        "expected `{}` to be written; got {:?}",
        expected.display(),
        outcome.written,
    );
}

#[test]
fn install_reports_missing_wit_source() {
    let root = tmpdir("missing_wit");
    write_manifest(
        &root,
        r#"
name = "my-app"
version = "0.1.0"

[imports]
"wasi/clocks" = "vendor/does-not-exist.wit"
"#,
    );

    let err = install::install(&root).expect_err("expected missing-source error");
    let msg = err.to_string();
    assert!(
        msg.contains("does-not-exist.wit"),
        "error should name the missing path; got: {msg}",
    );
}

#[test]
fn install_defers_wasm_component_entries() {
    let root = tmpdir("wasm_deferred");
    // The file doesn't need to exist — `.wasm` entries are recorded as
    // skipped without being read in this slice.
    write_manifest(
        &root,
        r#"
name = "my-app"
version = "0.1.0"

[imports]
"example/foo" = "vendor/some-lib.wasm"
"#,
    );

    let outcome =
        install::install(&root).expect("install should succeed even with a deferred wasm entry");
    assert!(outcome.written.is_empty());
    assert_eq!(outcome.skipped.len(), 1);
    assert!(
        outcome.skipped[0].contains("example/foo")
            && outcome.skipped[0].contains("not yet supported"),
        "skip message should name the import and explain why; got: {}",
        outcome.skipped[0],
    );
}

#[test]
fn install_on_manifest_without_imports_is_a_no_op() {
    let root = tmpdir("no_imports");
    write_manifest(
        &root,
        r#"
name = "my-app"
version = "0.1.0"
"#,
    );

    let outcome = install::install(&root).expect("install should succeed");
    assert!(outcome.written.is_empty());
    assert!(outcome.skipped.is_empty());

    // And no `bindgen/` directory should be created when there's nothing
    // to install — we don't want stray empty directories appearing in
    // user projects.
    assert!(!root.join("bindgen").exists());
}

#[test]
fn loader_resolves_use_against_installed_bindgen() {
    // The end-to-end story for slices 2a+2b: a user declares a WIT
    // import in their manifest, runs `canon install` to materialize
    // the bindings under `bindgen/`, and then their program can `use`
    // the bound interface as if it were any other Canon module.
    //
    // We stand up that exact shape on disk and assert that
    // `loader::load_module` resolves the `use` line against the
    // installed binding file.
    // A uniquely-named WIT package (not one the bundled `canon/std`
    // already provides) so a bare reference resolves to the project's
    // `bindgen/` without colliding with the standard library.
    let root = tmpdir("loader_uses_bindgen");
    vendor_wit(&root, "widget.wit", "widget.wit");
    write_manifest(
        &root,
        r#"
name = "my-app"
version = "0.1.0"

[imports]
"example/widget" = "vendor/widget.wit"
"#,
    );

    install::install(&root).expect("install should succeed");

    // Write a source file in src/main.can that references a name the
    // binding declares. Imports are automatic: the loader resolves
    // `spin` against the project's `bindgen/` tree by declared name,
    // pulling the whole binding file in.
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).expect("create src/");
    let entry = src_dir.join("main.can");
    fs::write(&entry, "main = () -> Unit {\n    spin().print()\n}\n").expect("write entry");

    let result = loader::load_module(&entry).expect("loader should resolve the bindgen import");

    // The binding file declares `spin` and `wobble`; both should land in
    // `module.items`. If resolution failed we'd never get here.
    let item_count = result.module.items.len();
    assert!(
        item_count >= 2,
        "expected the bindgen file's declarations to be loaded; got {item_count} items",
    );
}

#[test]
fn loader_derives_extern_urns_from_vendored_path() {
    // The slice-8 contract: an installed binding file carries no
    // directive; the loader derives each function's `extern_wasm.path`
    // from the vendored path (`bindgen/wasi/clocks@<ver>/…`), with the
    // function name camel-back-converted to kebab-case.
    let root = tmpdir("patch_bare_externs");
    vendor_wit(&root, "widget.wit", "widget.wit");
    write_manifest(
        &root,
        r#"
name = "my-app"
version = "0.1.0"

[imports]
"example/widget" = "vendor/widget.wit"
"#,
    );

    install::install(&root).expect("install should succeed");

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).expect("create src/");
    let entry = src_dir.join("main.can");
    fs::write(&entry, "main = () -> Unit {\n    spin().print()\n}\n").expect("write entry");

    let result = loader::load_module(&entry).expect("load");

    // The binding file declares `spin` and `wobble`; both should land in
    // `module.items` with their `extern_wasm.path` derived from the
    // vendored path (`bindgen/example/widget@1.0.0/gadget.can`).
    let externs: Vec<(String, String)> = result
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            canon::ast::Item::Function(f) => f
                .extern_wasm
                .as_ref()
                .map(|ew| (f.name.name.clone(), ew.path.clone())),
            _ => None,
        })
        .collect();

    let spin = externs
        .iter()
        .find(|(name, _)| name == "spin")
        .expect("`spin` should be loaded");
    assert_eq!(
        spin.1, "example:widget/gadget@1.0.0#spin",
        "loader should derive the full URN from the vendored path",
    );

    let wobble = externs
        .iter()
        .find(|(name, _)| name == "wobble")
        .expect("`wobble` should be loaded");
    assert_eq!(
        wobble.1, "example:widget/gadget@1.0.0#wobble",
        "each function's URN is derived from the same vendored path",
    );
}

#[test]
fn loader_falls_back_to_local_when_bindgen_does_not_exist() {
    // Project that has a manifest but no `[imports]` entry for the path
    // being imported. The loader must NOT short-circuit on the bindgen
    // lookup; it must continue to local-relative resolution exactly as
    // it did before slice 2b. This protects pre-slice-2b projects from
    // any regression.
    let root = tmpdir("loader_falls_back");
    write_manifest(
        &root,
        r#"
name = "my-app"
version = "0.1.0"
"#,
    );
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).expect("create src/");
    // A local sibling file, resolved by the name → file convention.
    fs::write(src_dir.join("marker.can"), "Marker = Int\n").expect("write sibling");
    let entry = src_dir.join("main.can");
    fs::write(&entry, "Wrapped = Marker\n").expect("write entry");

    let result =
        loader::load_module(&entry).expect("loader should fall through to local resolution");
    let names: Vec<String> = result
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            canon::ast::Item::TypeDef(t) => Some(t.name.name.clone()),
            _ => None,
        })
        .collect();
    assert!(
        names.iter().any(|n| n == "Marker"),
        "expected `Marker` from marker.can to be loaded; got names {names:?}",
    );
}

#[test]
fn ensure_installed_no_project_when_outside_any_canon_toml() {
    // A path with no `canon.toml` ancestor produces `NoProject` and
    // does nothing. This is the case for loose `.can` files outside any
    // project (e.g. our own `tests/runtime/` fixtures).
    let root = tmpdir("ensure_no_project");
    let loose = root.join("loose.can");
    fs::write(&loose, "main = () -> Unit { Unit() }\n").unwrap();

    let outcome = install::ensure_installed(&loose).expect("ensure_installed should not fail");
    assert!(
        matches!(outcome, EnsureOutcome::NoProject),
        "expected NoProject, got {outcome:?}",
    );
}

#[test]
fn ensure_installed_no_project_when_manifest_has_no_imports() {
    // Manifest exists but `[imports]` is empty — nothing to install,
    // and we don't want a stray `bindgen/` directory created.
    let root = tmpdir("ensure_no_imports");
    write_manifest(
        &root,
        r#"
name = "my-app"
version = "0.1.0"
"#,
    );

    let outcome = install::ensure_installed(&root).expect("ensure_installed should not fail");
    assert!(
        matches!(outcome, EnsureOutcome::NoProject),
        "expected NoProject for manifest without `[imports]`, got {outcome:?}",
    );
    assert!(!root.join("bindgen").exists());
}

#[test]
fn ensure_installed_installs_when_bindgen_missing() {
    let root = tmpdir("ensure_first_install");
    vendor_wit(&root, "monotonic-clock.wit", "monotonic-clock.wit");
    write_manifest(
        &root,
        r#"
name = "my-app"
version = "0.1.0"

[imports]
"wasi/clocks" = "vendor/monotonic-clock.wit"
"#,
    );

    let outcome = install::ensure_installed(&root).expect("ensure_installed should succeed");
    let installed = match outcome {
        EnsureOutcome::Installed(o) => o,
        other => panic!("expected Installed, got {other:?}"),
    };
    assert!(
        !installed.written.is_empty(),
        "should have written something"
    );
    assert!(root
        .join("bindgen/wasi/clocks@0.3.0-rc-2026-03-15/monotonic_clock.can")
        .is_file());
}

#[test]
fn ensure_installed_up_to_date_after_first_install() {
    let root = tmpdir("ensure_up_to_date");
    vendor_wit(&root, "monotonic-clock.wit", "monotonic-clock.wit");
    write_manifest(
        &root,
        r#"
name = "my-app"
version = "0.1.0"

[imports]
"wasi/clocks" = "vendor/monotonic-clock.wit"
"#,
    );

    // First call: installs.
    let first = install::ensure_installed(&root).unwrap();
    assert!(
        matches!(first, EnsureOutcome::Installed(_)),
        "first ensure should install, got {first:?}",
    );

    // Second call: nothing changed; should report UpToDate.
    let second = install::ensure_installed(&root).unwrap();
    assert!(
        matches!(second, EnsureOutcome::UpToDate),
        "second ensure should be UpToDate, got {second:?}",
    );
}

#[test]
fn ensure_installed_reinstalls_when_manifest_is_touched() {
    let root = tmpdir("ensure_manifest_touched");
    vendor_wit(&root, "monotonic-clock.wit", "monotonic-clock.wit");
    let manifest_src = r#"
name = "my-app"
version = "0.1.0"

[imports]
"wasi/clocks" = "vendor/monotonic-clock.wit"
"#;
    write_manifest(&root, manifest_src);

    install::ensure_installed(&root).unwrap();

    // Bump the manifest's mtime past the index's by sleeping just over
    // a second (filesystem mtime resolution on macOS is 1s for some
    // volumes) and then rewriting the file. We avoid pulling in a
    // `filetime` dependency for this single test — a 1.1s sleep is
    // acceptable in the suite (this test is the only one paying it).
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let manifest_path = root.join("canon.toml");
    fs::write(&manifest_path, manifest_src).unwrap();

    let outcome = install::ensure_installed(&root).unwrap();
    assert!(
        matches!(outcome, EnsureOutcome::Installed(_)),
        "touched manifest should trigger reinstall, got {outcome:?}",
    );
}

#[test]
fn install_is_idempotent() {
    let root = tmpdir("idempotent");
    vendor_wit(&root, "monotonic-clock.wit", "monotonic-clock.wit");
    write_manifest(
        &root,
        r#"
name = "my-app"
version = "0.1.0"

[imports]
"wasi/clocks" = "vendor/monotonic-clock.wit"
"#,
    );

    let first = install::install(&root).expect("first install");
    let first_content = fs::read_to_string(&first.written[0]).expect("read after first install");

    // Run install again; should rewrite the same files with the same
    // content. (Idempotence matters because `canon install` will
    // eventually be invoked implicitly from `canon build`.)
    let second = install::install(&root).expect("second install");
    assert_eq!(first.written, second.written);
    let second_content = fs::read_to_string(&second.written[0]).expect("read after second install");
    assert_eq!(first_content, second_content);
}
