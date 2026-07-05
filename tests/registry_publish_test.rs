//! `canon publish`.
//!
//! Drives the real CLI against a `local`-type registry, offline. The
//! headline test is the full round trip the RFC promises: publish a
//! pure-Canon library from one project, install it into another, run a
//! program that calls it. Also pinned: bare-spec patch-bumping, the
//! machine-recorded dependency list (read off the publisher's
//! `deps/<ns>/<name>@<version>/` directory names, surfaced on
//! install), and the canonical-format preflight.

mod common;

use common::canon_binary;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("canon-publish-tests")
        .join(format!("{}-{}", test, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Write a registry config routing every namespace to a `local`-type
/// registry rooted under `dir`. Returns the config path; the registry
/// root starts empty (publish creates package directories on demand).
fn write_registry_config(dir: &Path) -> PathBuf {
    let root = dir.join("registry");
    fs::create_dir_all(&root).expect("create registry root");
    let config_path = dir.join("wasm-pkg-config.toml");
    let config = format!(
        "default_registry = \"local.test\"\n\n\
         [registry.\"local.test\"]\ntype = \"local\"\n\n\
         [registry.\"local.test\".local]\nroot = {:?}\n",
        root.display().to_string(),
    );
    fs::write(&config_path, config).expect("write registry config");
    config_path
}

fn run_canon(cwd: &Path, config: &Path, args: &[&str]) -> (String, String, Option<i32>) {
    let output = Command::new(canon_binary())
        .args(args)
        .current_dir(cwd)
        .env("CANON_REGISTRY_CONFIG", config)
        .output()
        .expect("spawn canon");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    )
}

const SHOUT_CAN: &str = "shout = (String) => String {\n    String.concat(\"!\")\n}\n";

const MAIN_CAN: &str =
    "main = () => Unit {\n    \"hello\"\n        .shout()\n        .print()\n}\n";

#[test]
fn publish_install_run_round_trip() {
    let dir = scratch("round-trip");
    let config = write_registry_config(&dir);

    // Publisher side: a one-file pure-Canon library.
    let lib = dir.join("lib");
    fs::create_dir_all(&lib).unwrap();
    fs::write(lib.join("shout.can"), SHOUT_CAN).unwrap();
    let (stdout, stderr, code) = run_canon(&lib, &config, &["publish", "acme:greet@1.0.0"]);
    assert_eq!(
        code,
        Some(0),
        "publish failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("published: acme:greet@1.0.0"),
        "unexpected stdout: {stdout}"
    );
    assert!(
        dir.join("registry/acme/greet/1.0.0.wasm").is_file(),
        "artifact should land in the local registry"
    );

    // Consumer side: install it and run a program through it.
    let app = dir.join("app");
    fs::create_dir_all(&app).unwrap();
    let (stdout, stderr, code) = run_canon(&app, &config, &["install", "acme:greet"]);
    assert_eq!(
        code,
        Some(0),
        "install failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // The pin is the directory name; the vendored file is byte-for-byte
    // the published source.
    let vendored = fs::read_to_string(app.join("deps/acme/greet@1.0.0/shout.can")).unwrap();
    assert_eq!(
        vendored, SHOUT_CAN,
        "vendored source must be the published source, unstamped"
    );

    fs::write(app.join("main.can"), MAIN_CAN).unwrap();
    let (stdout, stderr, code) = run_canon(&app, &config, &["run", "main.can"]);
    assert_eq!(
        code,
        Some(0),
        "run failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, "hello!\n");
}

#[test]
fn bare_publish_starts_at_0_1_0_and_patch_bumps() {
    let dir = scratch("patch-bump");
    let config = write_registry_config(&dir);
    let lib = dir.join("lib");
    fs::create_dir_all(&lib).unwrap();
    fs::write(lib.join("shout.can"), SHOUT_CAN).unwrap();

    let (stdout, _, code) = run_canon(&lib, &config, &["publish", "acme:greet"]);
    assert_eq!(code, Some(0));
    assert!(
        stdout.contains("published: acme:greet@0.1.0"),
        "first bare publish should be 0.1.0: {stdout}"
    );

    let (stdout, _, code) = run_canon(&lib, &config, &["publish", "acme:greet"]);
    assert_eq!(code, Some(0));
    assert!(
        stdout.contains("published: acme:greet@0.1.1"),
        "second bare publish should patch-bump: {stdout}"
    );
}

#[test]
fn published_dependency_list_surfaces_on_install() {
    let dir = scratch("dep-list");
    let config = write_registry_config(&dir);

    // The publisher vendors one dependency; its versioned directory
    // name is the machine-recorded dep list.
    let lib = dir.join("lib");
    fs::create_dir_all(lib.join("deps/other/pkg@2.0.0")).unwrap();
    fs::write(lib.join("shout.can"), SHOUT_CAN).unwrap();
    fs::write(
        lib.join("deps/other/pkg@2.0.0/thing.can"),
        "thing = (String) => String {\n    String.concat(\"?\")\n}\n",
    )
    .unwrap();
    let (_, stderr, code) = run_canon(&lib, &config, &["publish", "acme:combo@1.0.0"]);
    assert_eq!(code, Some(0), "publish failed: {stderr}");

    let app = dir.join("app");
    fs::create_dir_all(&app).unwrap();
    let (_, stderr, code) = run_canon(&app, &config, &["install", "acme:combo"]);
    assert_eq!(code, Some(0), "install failed: {stderr}");
    assert!(
        stderr.contains("depends on `other:pkg@2.0.0`")
            && stderr.contains("canon install other:pkg"),
        "install should surface the recorded dependency: {stderr}"
    );
    // Only the package's own files are vendored — the dependency is the
    // consumer's own install (until slice 4 automates it).
    assert!(app.join("deps/acme/combo@1.0.0/shout.can").is_file());
    assert!(!app.join("deps/other").exists());
}

#[test]
fn publish_refuses_unformatted_source() {
    let dir = scratch("unformatted");
    let config = write_registry_config(&dir);
    let lib = dir.join("lib");
    fs::create_dir_all(&lib).unwrap();
    // Same program, non-canonical whitespace.
    fs::write(
        lib.join("shout.can"),
        "shout = (String) => String { String.concat(\"!\") }\n",
    )
    .unwrap();

    let (_, stderr, code) = run_canon(&lib, &config, &["publish", "acme:greet@1.0.0"]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("not canonically formatted"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !dir.join("registry/acme/greet/1.0.0.wasm").exists(),
        "nothing may be published when preflight fails"
    );
}
