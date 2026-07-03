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
fn exit_code_propagates() {
    let workdir = std::env::temp_dir().join(format!("canon_exit_test_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let src_path = workdir.join("exit3.can");
    std::fs::write(
        &src_path,
        r#"use canon/std/cli/Exit

main = () -> Unit {
    "terminating with 3".print()
    Exit(3).exit()
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
