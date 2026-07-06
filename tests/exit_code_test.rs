//! Exit-code threading: a Canon program calling
//! `canon/std/cli`'s `exit` terminates the `canon run` process with
//! that code. The stdlib wrapper rides the *real*
//! `wasi:cli/exit@0.3.0-rc-2026-03-15#exit-with-code` import — the
//! first narrow-int (u8) WASI binding emitted by the WIT-informed
//! extern lowering — and the runtime maps the resulting `I32Exit`
//! trap onto the process exit status.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn canon_test_exit_codes() {
    // `canon test` exits 1 when any test fails and 0 when all pass —
    // the synthesised main counts failures and drives
    // `wasi:cli/exit#exit-with-code`.
    let workdir = std::env::temp_dir().join(format!("canon_test_exit_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let canon_bin = PathBuf::from(env!("CARGO_BIN_EXE_canon"));

    let failing = workdir.join("failing_test.can");
    std::fs::write(
        &failing,
        r#"testBroken = () => TestResult {
    1
        -> Sum(2)
        -> Eq(7)
        -> TestResult("math is broken")
}

testFine = () => TestResult {
    1
        -> Sum(2)
        -> Eq(3)
        -> TestResult("math works")
}
"#,
    )
    .unwrap();
    let out = Command::new(&canon_bin)
        .arg("test")
        .arg(&failing)
        .output()
        .expect("canon test spawns");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[FAIL] testBroken: math is broken"),
        "single-line failure banner, got:\n{stdout}"
    );
    assert!(stdout.contains("[ ok ] testFine"), "got:\n{stdout}");
    assert_eq!(out.status.code(), Some(1), "failing suite exits 1");

    let passing = workdir.join("passing_test.can");
    std::fs::write(
        &passing,
        r#"testFine = () => TestResult {
    1
        -> Sum(2)
        -> Eq(3)
        -> TestResult("math works")
}
"#,
    )
    .unwrap();
    let out = Command::new(&canon_bin)
        .arg("test")
        .arg(&passing)
        .output()
        .expect("canon test spawns");
    assert_eq!(out.status.code(), Some(0), "passing suite exits 0");

    let _ = std::fs::remove_dir_all(&workdir);
}

#[test]
fn exit_code_propagates() {
    let workdir = std::env::temp_dir().join(format!("canon_exit_test_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let src_path = workdir.join("exit3.can");
    std::fs::write(
        &src_path,
        r#"Unit => Program {
    "terminating with 3" -> Print
    3 -> Exited
}
"#,
    )
    .unwrap();

    let canon_bin = PathBuf::from(env!("CARGO_BIN_EXE_canon"));
    let out = Command::new(&canon_bin)
        .arg("run")
        .arg(&src_path)
        .output()
        .expect("canon run spawns");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("terminating with 3"),
        "print before exit reached stdout, got:\n{stdout}"
    );
    assert_eq!(
        out.status.code(),
        Some(3),
        "exit code propagates; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let _ = std::fs::remove_dir_all(&workdir);
}

#[test]
fn args_entry_exit_status() {
    // The canonical CLI entry `Args => Exit`: the argument vector flows
    // in (`Args`), and the returned `Exit` maps onto the wasi:cli/run
    // `result` — `Exit(0)` is ok (process exit 0), any nonzero code is
    // err (process exit 1). Here the exit status *is* the argument
    // count, so the same program exits 0 with no args and 1 with some.
    let workdir = std::env::temp_dir().join(format!("canon_argc_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let src_path = workdir.join("argc.can");
    std::fs::write(
        &src_path,
        r#"Args => Exit {
    Args
        -> Length
        -> Print
    Args
        -> Length
        -> Exit
}
"#,
    )
    .unwrap();
    let canon_bin = PathBuf::from(env!("CARGO_BIN_EXE_canon"));

    // No args: `Args -> Length` is 0, printed, and `Exit(0)` → exit 0.
    let out = Command::new(&canon_bin)
        .arg("run")
        .arg(&src_path)
        .output()
        .expect("canon run spawns");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "0",
        "argv is empty with no forwarded args"
    );
    assert_eq!(out.status.code(), Some(0), "Exit(0) → process exit 0");

    // Two forwarded args: argv length 2, printed, and `Exit(2)` → err
    // (a nonzero exit maps to the run result's err discriminant → 1).
    let out = Command::new(&canon_bin)
        .arg("run")
        .arg(&src_path)
        .arg("alpha")
        .arg("beta")
        .output()
        .expect("canon run spawns");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "2",
        "forwarded args reach the program's argv"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "a nonzero Exit maps to the run err discriminant; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let _ = std::fs::remove_dir_all(&workdir);
}
