//! Registry-backed `canon install` — PACKAGES.md slice 2.
//!
//! Drives the real CLI end-to-end against a `local`-type registry (the
//! filesystem backend `wasm-pkg-client` ships): the test encodes a
//! throwaway WIT package into the wasm form registries serve, lays it
//! out as `<root>/<ns>/<name>/<version>.wasm`, points the install at it
//! via a `CANON_REGISTRY_CONFIG` config file, and asserts the vendored
//! `deps/` tree — `package` + `bindings` directives included — checks
//! cleanly with the loader. No network involved.

mod common;

use common::canon_binary;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Fresh scratch directory for one test. Anything from a previous run
/// is removed first so assertions never see stale files.
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("canon-registry-tests")
        .join(format!("{}-{}", test, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Encode a WIT package (source text) into the wasm binary form a
/// registry serves for it.
fn encode_wit(wit: &str) -> Vec<u8> {
    let mut resolve = wit_parser::Resolve::default();
    let pkg = resolve
        .push_str("test.wit", wit)
        .expect("test WIT should parse");
    wit_component::encode(&resolve, pkg).expect("test WIT should encode")
}

/// Lay out a local registry containing the `test:adder` package at the
/// given versions, plus a config file routing everything to it.
/// Returns the config file path.
fn write_local_registry(dir: &Path, versions: &[&str]) -> PathBuf {
    let root = dir.join("registry");
    for version in versions {
        let wit = format!(
            "package test:adder@{version};\n\n\
             interface add {{\n    add: func(left: u64, right: u64) -> u64;\n}}\n"
        );
        let path = root
            .join("test")
            .join("adder")
            .join(format!("{version}.wasm"));
        fs::create_dir_all(path.parent().unwrap()).expect("create registry dirs");
        fs::write(&path, encode_wit(&wit)).expect("write package artifact");
    }
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

/// Run `canon <subcommand> <args…>` with the given working directory
/// and registry config, returning (stdout, stderr, exit code).
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

#[test]
fn install_vendors_latest_release_and_project_checks() {
    let dir = scratch("latest");
    let config = write_local_registry(&dir, &["1.0.0", "1.1.0"]);
    let project = dir.join("project");
    fs::create_dir_all(&project).unwrap();

    let (stdout, stderr, code) = run_canon(&project, &config, &["install", "test:adder"]);
    assert_eq!(
        code,
        Some(0),
        "install failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let vendored = project.join("deps/test/adder/add.can");
    let content = fs::read_to_string(&vendored)
        .unwrap_or_else(|e| panic!("expected `{}` to exist: {e}", vendored.display()));
    assert!(
        content.starts_with("package \"test:adder@1.1.0\"\n"),
        "vendored file must open with the provenance directive pinning the newest release:\n{content}"
    );
    assert!(
        content.contains("bindings \"test:adder/add@1.1.0\""),
        "vendored file must carry the interface URN:\n{content}"
    );

    // The vendored package is usable: a program `use`s it and the
    // loader accepts the whole tree (directives validated, extern
    // signatures resolved). Format first so the fixture doesn't encode
    // the formatter's chain-breaking rules.
    fs::write(
        project.join("main.can"),
        "use test/adder/add\n\nmain = () -> Unit {\n    1.add(2).print()\n}\n",
    )
    .unwrap();
    let (o, e, c) = run_canon(&project, &config, &["fmt", "main.can"]);
    assert_eq!(c, Some(0), "fmt failed.\nstdout:\n{o}\nstderr:\n{e}");
    let (o, e, c) = run_canon(&project, &config, &["check", "main.can"]);
    assert_eq!(c, Some(0), "check failed.\nstdout:\n{o}\nstderr:\n{e}");
}

#[test]
fn install_pins_exact_and_prefix_versions() {
    let dir = scratch("pinned");
    let config = write_local_registry(&dir, &["1.0.0", "1.1.0", "2.0.0"]);

    let exact = dir.join("exact");
    fs::create_dir_all(&exact).unwrap();
    let (stdout, stderr, code) = run_canon(&exact, &config, &["install", "test:adder@1.0.0"]);
    assert_eq!(
        code,
        Some(0),
        "install failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let content = fs::read_to_string(exact.join("deps/test/adder/add.can")).unwrap();
    assert!(
        content.starts_with("package \"test:adder@1.0.0\"\n"),
        "exact pin should install 1.0.0:\n{content}"
    );

    let prefix = dir.join("prefix");
    fs::create_dir_all(&prefix).unwrap();
    let (stdout, stderr, code) = run_canon(&prefix, &config, &["install", "test:adder@1"]);
    assert_eq!(
        code,
        Some(0),
        "install failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let content = fs::read_to_string(prefix.join("deps/test/adder/add.can")).unwrap();
    assert!(
        content.starts_with("package \"test:adder@1.1.0\"\n"),
        "prefix pin `@1` should install the newest 1.x:\n{content}"
    );
}

#[test]
fn install_reports_unknown_version() {
    let dir = scratch("unknown-version");
    let config = write_local_registry(&dir, &["1.0.0"]);
    let project = dir.join("project");
    fs::create_dir_all(&project).unwrap();

    let (_, stderr, code) = run_canon(&project, &config, &["install", "test:adder@9.9.9"]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("no release of `test:adder` matches `9.9.9`"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        stderr.contains("1.0.0"),
        "error should list available versions: {stderr}"
    );
}

#[test]
fn malformed_spec_is_rejected_before_any_fetch() {
    let dir = scratch("malformed");
    let config = dir.join("nonexistent-config.toml");
    let project = dir.join("project");
    fs::create_dir_all(&project).unwrap();

    let (_, stderr, code) = run_canon(&project, &config, &["install", "Acme:Http@"]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("malformed package spec"),
        "unexpected stderr: {stderr}"
    );
}
