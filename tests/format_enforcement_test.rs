//! The check/fmt unification: canonical formatting is part of the
//! language, so a formatting divergence IS a compiler error.
//! `canon check` (and `build`/`run`/`test`, through the same gate)
//! reports an unformatted file as an ordinary `error[path:line:col]`
//! diagnostic pointing at the first divergence, and `canon fmt` is
//! purely the mechanical fixer — its old `--check` mode is gone,
//! because verifying is `canon check`'s job.

use canon::formatter::{format, format_error};
use std::path::PathBuf;
use std::process::Command;

/// `Print("hello")` is the prefix-call spelling; canonical form pipes
/// the first input (`"hello" -> Print`), so line 2 diverges at the
/// first non-indent character (column 5).
const UNFORMATTED: &str = "Unit => Program {\n    Print(\"hello\")\n}\n";

fn canon_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_canon"))
}

#[test]
fn canonical_source_has_no_format_error() {
    let canonical = format(UNFORMATTED).expect("fixture parses");
    assert!(
        format_error(&canonical).is_none(),
        "canonical form must be a fixpoint"
    );
}

#[test]
fn divergence_is_a_check_error_spanning_the_first_differing_line() {
    let err = format_error(UNFORMATTED).expect("unformatted source yields an error");
    assert!(
        err.message().contains("canon fmt"),
        "error names the fixer, got: {}",
        err.message()
    );
    let span = err.span();
    assert_eq!((span.line, span.column), (2, 5), "first divergence");
}

#[test]
fn unparseable_source_defers_to_the_checker() {
    // A parse error is the checker pipeline's diagnostic to report,
    // with its precise location — format_error stays silent.
    assert!(format_error("Unit => Program {").is_none());
}

#[test]
fn check_rejects_unformatted_source_and_fmt_fixes_it() {
    let dir = std::env::temp_dir().join(format!("canon_fmt_unify_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("main.can");
    std::fs::write(&file, UNFORMATTED).unwrap();

    let out = Command::new(canon_bin())
        .arg("check")
        .arg(&file)
        .output()
        .expect("canon check spawns");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "unformatted file fails check");
    assert!(
        stderr.contains("main.can:2:5]") && stderr.contains("not canonically formatted"),
        "format divergence reported as a located compiler error, got:\n{stderr}"
    );
    assert!(
        stderr.contains("error(s) found."),
        "counted like any other compile error, got:\n{stderr}"
    );

    let out = Command::new(canon_bin())
        .arg("fmt")
        .arg(&file)
        .output()
        .expect("canon fmt spawns");
    assert_eq!(out.status.code(), Some(0), "fmt fixes in place");

    let out = Command::new(canon_bin())
        .arg("check")
        .arg(&file)
        .output()
        .expect("canon check spawns");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "formatted file passes check, stderr:\n{stderr}"
    );
}

#[test]
fn fmt_has_no_verify_only_mode() {
    let out = Command::new(canon_bin())
        .args(["fmt", "--check"])
        .output()
        .expect("canon fmt spawns");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown fmt flag '--check'"),
        "got:\n{stderr}"
    );
}
