//! End-to-end tests for `canon install`.
//!
//! Each test stands up a temporary project directory whose `wit/`
//! directory holds one or more WIT sources (imports are declared by
//! file structure — there is no manifest), runs
//! `canon::install::install` against the project root, and asserts on
//! what landed under `bindgen/`.

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

/// Copy a WIT fixture into the project under `wit/<dest>` — declaring
/// it as an import. `dest` may contain a subdirectory (a directory
/// source: `clocks/monotonic-clock.wit`).
fn vendor_wit(project_root: &Path, fixture_relative: &str, dest: &str) {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("wit")
        .join(fixture_relative);
    let dst = project_root.join("wit").join(dest);
    fs::create_dir_all(dst.parent().expect("dest has a parent")).expect("create wit dir");
    fs::copy(&src, &dst).expect("copy fixture");
}

#[test]
fn install_emits_one_file_per_interface() {
    let root = tmpdir("monotonic_clock");
    vendor_wit(&root, "monotonic-clock.wit", "monotonic-clock.wit");

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
    // Each function is a string-anchored anonymous constructor over a
    // minted result newtype, its body naming the WIT fragment verbatim.
    assert!(content.contains("Now = Instant"));
    assert!(content.contains("Unit => Now {\n    \"now\"\n}"));
    assert!(content.contains("GetResolution = Duration"));
    assert!(content.contains("Unit => GetResolution {\n    \"get-resolution\"\n}"));

    // The index sidecar should map this file to the correct URN.
    let index_content = fs::read_to_string(&expected_index).expect("read index");
    assert!(index_content.contains("\"wasi/clocks@0.3.0-rc-2026-03-15/monotonic_clock.can\""));
    assert!(index_content.contains("wasi:clocks/monotonic-clock@"));
}

#[test]
fn install_accepts_directory_source() {
    // A subdirectory of `wit/` is a directory-of-WITs source (the shape
    // the vendored WASI tree uses). Installing the fixture from inside
    // one should produce the same binding as the flat-file form.
    let root = tmpdir("directory_source");
    vendor_wit(&root, "monotonic-clock.wit", "clocks/monotonic-clock.wit");

    let outcome = install::install(&root).expect("directory source should install");
    let expected = root.join("bindgen/wasi/clocks@0.3.0-rc-2026-03-15/monotonic_clock.can");
    assert!(
        expected.exists(),
        "expected `{}` to be written; got {:?}",
        expected.display(),
        outcome.written,
    );
}

#[test]
fn install_rejects_stray_file_in_wit_dir() {
    // `wit/` is a declaration: anything in it that isn't a `.wit` file,
    // a directory, or a `.wasm` component is an error, not silently
    // ignored.
    let root = tmpdir("stray_wit_entry");
    fs::create_dir_all(root.join("wit")).unwrap();
    fs::write(root.join("wit/README.txt"), "not a wit file").unwrap();

    let err = install::install(&root).expect_err("expected stray-entry error");
    let msg = err.to_string();
    assert!(
        msg.contains("README.txt") && msg.contains(".wit"),
        "error should name the stray entry and the accepted shapes; got: {msg}",
    );
}

#[test]
fn install_reports_missing_wit_dir() {
    let root = tmpdir("missing_wit_dir");

    let err = install::install(&root).expect_err("expected missing-`wit/` error");
    let msg = err.to_string();
    assert!(
        msg.contains("wit/"),
        "error should point at the `wit/` convention; got: {msg}",
    );
}

#[test]
fn install_defers_wasm_component_entries() {
    let root = tmpdir("wasm_deferred");
    // The file's contents don't matter — `.wasm` entries are recorded
    // as skipped without being read in this slice.
    fs::create_dir_all(root.join("wit")).unwrap();
    fs::write(root.join("wit/some-lib.wasm"), b"\0asm").unwrap();

    let outcome =
        install::install(&root).expect("install should succeed even with a deferred wasm entry");
    assert!(outcome.written.is_empty());
    assert_eq!(outcome.skipped.len(), 1);
    assert!(
        outcome.skipped[0].contains("some-lib.wasm")
            && outcome.skipped[0].contains("not yet supported"),
        "skip message should name the import and explain why; got: {}",
        outcome.skipped[0],
    );
}

#[test]
fn install_on_empty_wit_dir_is_a_no_op() {
    let root = tmpdir("no_imports");
    fs::create_dir_all(root.join("wit")).unwrap();

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
    // The end-to-end story for slices 2a+2b: a user drops a WIT source
    // under `wit/`, runs `canon install` to materialize the bindings
    // under `bindgen/`, and then their program can `use` the bound
    // interface as if it were any other Canon module.
    //
    // We stand up that exact shape on disk and assert that
    // `loader::load_module` resolves the `use` line against the
    // installed binding file.
    // A uniquely-named WIT package (not one the bundled `canon/std`
    // already provides) so a bare reference resolves to the project's
    // `bindgen/` without colliding with the standard library.
    let root = tmpdir("loader_uses_bindgen");
    vendor_wit(&root, "widget.wit", "widget.wit");

    install::install(&root).expect("install should succeed");

    // Write a source file in src/main.can that references a name the
    // binding declares. Imports are automatic: the loader resolves
    // `spin` against the project's `bindgen/` tree by declared name,
    // pulling the whole binding file in.
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).expect("create src/");
    let entry = src_dir.join("main.can");
    fs::write(&entry, "main = () => Unit {\n    Spin() -> Print\n}\n").expect("write entry");

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

    install::install(&root).expect("install should succeed");

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).expect("create src/");
    let entry = src_dir.join("main.can");
    fs::write(&entry, "main = () => Unit {\n    Spin() -> Print\n}\n").expect("write entry");

    let result = loader::load_module(&entry).expect("load");

    // The binding file declares `spin` and `wobble`; both should land in
    // `module.items` with their `extern_wasm.path` derived from the
    // vendored path (`bindgen/example/widget@1.0.0/gadget.can`). Each is a
    // string-anchored constructor — the loader lifts it into an extern
    // whose `extern_wasm.path` names the WIT fragment from the string body
    // (`spin` constructs the minted `Spin` newtype, receiver-renamed to
    // `Self`), so we key on the derived URN rather than the function name.
    let extern_paths: Vec<String> = result
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            canon::ast::Item::Function(f) => f.extern_wasm.as_ref().map(|ew| ew.path.clone()),
            _ => None,
        })
        .collect();

    assert!(
        extern_paths
            .iter()
            .any(|p| p == "example:widget/gadget@1.0.0#spin"),
        "loader should derive the full URN from the vendored path; got {extern_paths:?}",
    );

    assert!(
        extern_paths
            .iter()
            .any(|p| p == "example:widget/gadget@1.0.0#wobble"),
        "each function's URN is derived from the same vendored path; got {extern_paths:?}",
    );
}

#[test]
fn loader_falls_back_to_local_when_bindgen_does_not_exist() {
    // Project with no `wit/` and no `bindgen/`. The loader must NOT
    // short-circuit on the bindgen lookup; it must continue to
    // local-relative resolution exactly as it did before slice 2b.
    let root = tmpdir("loader_falls_back");
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
fn ensure_installed_no_project_when_outside_any_project() {
    // A path with no project-root ancestor (no `src/main.can`, `wit/`,
    // `bindgen/`, or `deps/` on the walk up) produces `NoProject` and
    // does nothing. This is the case for loose `.can` files outside any
    // project (e.g. our own `tests/runtime/` fixtures).
    let root = tmpdir("ensure_no_project");
    let loose = root.join("loose.can");
    fs::write(&loose, "main = () => Unit { Unit() }\n").unwrap();

    let outcome = install::ensure_installed(&loose).expect("ensure_installed should not fail");
    assert!(
        matches!(outcome, EnsureOutcome::NoProject),
        "expected NoProject, got {outcome:?}",
    );
}

#[test]
fn ensure_installed_no_project_when_wit_dir_is_empty() {
    // `wit/` exists but holds no sources — nothing to install, and we
    // don't want a stray `bindgen/` directory created.
    let root = tmpdir("ensure_no_imports");
    fs::create_dir_all(root.join("wit")).unwrap();

    let outcome = install::ensure_installed(&root).expect("ensure_installed should not fail");
    assert!(
        matches!(outcome, EnsureOutcome::NoProject),
        "expected NoProject for an empty `wit/`, got {outcome:?}",
    );
    assert!(!root.join("bindgen").exists());
}

#[test]
fn ensure_installed_installs_when_bindgen_missing() {
    let root = tmpdir("ensure_first_install");
    vendor_wit(&root, "monotonic-clock.wit", "monotonic-clock.wit");

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
fn ensure_installed_reinstalls_when_wit_source_is_touched() {
    let root = tmpdir("ensure_wit_touched");
    vendor_wit(&root, "monotonic-clock.wit", "monotonic-clock.wit");

    install::ensure_installed(&root).unwrap();

    // Bump the WIT source's mtime past the index's by sleeping just over
    // a second (filesystem mtime resolution on macOS is 1s for some
    // volumes) and then rewriting the file. We avoid pulling in a
    // `filetime` dependency for this single test — a 1.1s sleep is
    // acceptable in the suite (this test is the only one paying it).
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let wit_path = root.join("wit/monotonic-clock.wit");
    let wit_src = fs::read_to_string(&wit_path).unwrap();
    fs::write(&wit_path, wit_src).unwrap();

    let outcome = install::ensure_installed(&root).unwrap();
    assert!(
        matches!(outcome, EnsureOutcome::Installed(_)),
        "touched WIT source should trigger reinstall, got {outcome:?}",
    );
}

#[test]
fn install_is_idempotent() {
    let root = tmpdir("idempotent");
    vendor_wit(&root, "monotonic-clock.wit", "monotonic-clock.wit");

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
