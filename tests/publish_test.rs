//! `canon publish` is a thin shell over `wkg oci push`. These tests pin
//! the two edges the shell owns: a missing `wkg` must teach the install
//! (before any build work happens), and a present `wkg` must receive
//! exactly `oci push <ref> <artifact>` after a successful build.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn canon_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_canon"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

#[test]
fn missing_wkg_teaches_the_install() {
    let dir = scratch("publish_no_wkg");
    let out = Command::new(canon_bin())
        .args(["publish", "localhost:5000/demo:0.1.0", "."])
        .current_dir(&dir)
        .env("PATH", "")
        .output()
        .expect("run canon publish");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("wasm-pkg-tools"), "stderr: {stderr}");
    assert!(stderr.contains("cargo install wkg"), "stderr: {stderr}");
}

#[test]
fn missing_reference_prints_usage() {
    let out = Command::new(canon_bin())
        .arg("publish")
        .output()
        .expect("run canon publish");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Usage: canon publish <oci-ref>"),
        "stderr: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn publish_invokes_wkg_oci_push() {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch("publish_fake_wkg");
    let bin = dir.join("bin");
    let src = dir.join("demo/src");
    fs::create_dir_all(&bin).expect("create bin dir");
    fs::create_dir_all(&src).expect("create package src");
    fs::write(
        src.join("main.can"),
        "Unit => Program {\n    \"hi\" -> Print\n}\n",
    )
    .expect("write program");

    let log = dir.join("wkg-args.txt");
    let fake = bin.join("wkg");
    fs::write(
        &fake,
        format!("#!/bin/sh\necho \"$@\" > {}\n", log.display()),
    )
    .expect("write fake wkg");
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("chmod fake wkg");

    let out = Command::new(canon_bin())
        .args(["publish", "localhost:5000/demo:0.1.0"])
        .arg(dir.join("demo"))
        .env("PATH", &bin)
        .output()
        .expect("run canon publish");
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let args = fs::read_to_string(&log).expect("fake wkg was invoked");
    assert!(
        args.starts_with("oci push localhost:5000/demo:0.1.0"),
        "wkg args: {args}"
    );
    assert!(args.trim_end().ends_with("demo.wasm"), "wkg args: {args}");
}
