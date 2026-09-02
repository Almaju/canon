//! Exit-code threading: a Canon program calling
//! `canon/cli`'s `exit` terminates the `canon run` process with
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
        r#"BrokenMath = TestResult

Unit => BrokenMath {
    1 -> Sum(2) -> Eq(7) -> (
        * False { Fail("math is broken") }
        * True { Pass() }
    )
}

Unit => WorkingMath {
    1
        -> Sum(2)
        -> Eq(3)
        -> TestResult
}

WorkingMath = TestResult
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
        stdout.contains("[FAIL] BrokenMath: math is broken"),
        "single-line failure banner, got:\n{stdout}"
    );
    assert!(stdout.contains("[ ok ] WorkingMath"), "got:\n{stdout}");
    assert_eq!(out.status.code(), Some(1), "failing suite exits 1");

    let passing = workdir.join("passing_test.can");
    std::fs::write(
        &passing,
        r#"Unit => WorkingMath {
    1
        -> Sum(2)
        -> Eq(3)
        -> TestResult
}

WorkingMath = TestResult
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
    Exited(3)
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
fn args_reach_argv() {
    // The entry takes no arguments — `wasi:cli/run.run` passes none.
    // The argument vector is fetched instead: `Args()` reads argv via
    // `wasi:cli/environment#get-arguments`, and `canon run` forwards
    // everything after the target into it.
    let workdir = std::env::temp_dir().join(format!("canon_argc_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let src_path = workdir.join("argc.can");
    std::fs::write(
        &src_path,
        r#"Unit => Program {
    Args()
        -> Length
        -> Print
}
"#,
    )
    .unwrap();
    let canon_bin = PathBuf::from(env!("CARGO_BIN_EXE_canon"));

    // No args: `Args() -> Length` is 0, and reaching the end is exit 0.
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
    assert_eq!(out.status.code(), Some(0), "reaching the end → exit 0");

    // Two forwarded args: argv length 2.
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
        "forwarded args reach the program's argv; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(0), "success is implicit");

    let _ = std::fs::remove_dir_all(&workdir);
}

#[test]
fn a_result_entry_fails_on_err() {
    // A `Result` entry that hits `Err` — through `?` or as its final
    // value — prints the error's string payload and exits 1, instead of
    // discarding the result and exiting 0.
    let workdir = std::env::temp_dir().join(format!("canon_result_entry_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let canon_bin = PathBuf::from(env!("CARGO_BIN_EXE_canon"));

    let via_try = workdir.join("via_try.can");
    std::fs::write(
        &via_try,
        "Unit => Result<Program, MalformedInt> {\n    Int(\"nope\")? -> Print\n    Unit() -> Ok\n}\n",
    )
    .unwrap();
    let out = Command::new(&canon_bin)
        .arg("run")
        .arg(&via_try)
        .output()
        .expect("canon run spawns");
    assert_eq!(out.status.code(), Some(1), "`?` on Err exits 1");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("invalid integer"),
        "the error payload is printed, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let final_err = workdir.join("final_err.can");
    std::fs::write(
        &final_err,
        "Unit => Result<Program, MalformedInt> {\n    \"start\" -> Print\n    MalformedInt(\"ended badly\") -> Err\n}\n",
    )
    .unwrap();
    let out = Command::new(&canon_bin)
        .arg("run")
        .arg(&final_err)
        .output()
        .expect("canon run spawns");
    assert_eq!(out.status.code(), Some(1), "an Err final value exits 1");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "start\nended badly"
    );
}
