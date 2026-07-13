//! Formatting is a compiler phase. A divergence from canonical form is
//! a `FormatError` — its own error kind alongside lex/parse/check —
//! produced inside `checker::check_loaded`, so `canon check` (and
//! `build`/`run`/`test`) reports it as an ordinary
//! `error[path:line:col]` diagnostic, fused into the same run as
//! semantic errors rather than gating in front of them. There is no
//! separate formatter command: `canon check --fix` rewrites what is
//! mechanically fixable (formatting, including sort-order violations —
//! the formatter sorts) in place, re-loads, and checks the result.

use canon::formatter::{format, format_error};
use std::path::PathBuf;
use std::process::Command;

/// `Print("hello")` is the prefix-call spelling; canonical form pipes
/// the first input (`"hello" -> Print`), so line 2 diverges at the
/// first non-indent character (column 5).
const UNFORMATTED: &str = "Unit => Program {\n    Print(\"hello\")\n}\n";

/// Dispatch arms out of canonical order (`True` before `False`) — a
/// checker error on its own, and mechanically fixable: the formatter
/// sorts arms, so `--fix` repairs it before the checker runs.
const UNSORTED_ARMS: &str = "Unit => Program {\n    True() -> (\n        * True => Unit {\n            \"yes\" -> Print\n        }\n        * False => Unit {\n            \"no\" -> Print\n        }\n    )\n}\n";

fn canon_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_canon"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("canon_fmt_unify_{}_{}", name, std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn canonical_source_has_no_format_error() {
    let canonical = format(UNFORMATTED).expect("fixture parses");
    assert!(
        format_error(&canonical, "main.can").is_none(),
        "canonical form must be a fixpoint"
    );
}

#[test]
fn divergence_is_a_format_error_spanning_the_first_differing_line() {
    let err = format_error(UNFORMATTED, "main.can").expect("unformatted source yields an error");
    assert!(
        err.to_string().starts_with("format error"),
        "formatting is its own compiler phase, got: {}",
        err
    );
    assert!(
        err.message().contains("canon check --fix"),
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
    assert!(format_error("Unit => Program {", "main.can").is_none());
}

#[test]
fn check_rejects_unformatted_source_and_fix_repairs_it() {
    let file = scratch("fixes").join("main.can");
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

    // One invocation fixes and checks: `--fix` writes the canonical
    // form, re-loads, and the same run passes.
    let out = Command::new(canon_bin())
        .args(["check", "--fix"])
        .arg(&file)
        .output()
        .expect("canon check --fix spawns");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "--fix repairs and passes in one run, stderr:\n{stderr}"
    );
    assert!(
        stdout.contains("fixed:") && stdout.contains("All checks passed."),
        "reports the fix and the clean check, got:\n{stdout}"
    );
    let fixed = std::fs::read_to_string(&file).unwrap();
    assert_eq!(fixed, format(UNFORMATTED).unwrap(), "canonical on disk");
}

#[test]
fn fix_repairs_sort_order_violations_too() {
    // Unsorted dispatch arms are a checker error, not just cosmetics —
    // and still mechanically fixable. `--fix` must sort them and then
    // check the *re-loaded* source, not the stale unsorted parse.
    let file = scratch("sorts").join("main.can");
    std::fs::write(&file, UNSORTED_ARMS).unwrap();

    let out = Command::new(canon_bin())
        .args(["check", "--fix"])
        .arg(&file)
        .output()
        .expect("canon check --fix spawns");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "sort-order violation is auto-fixed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    let fixed = std::fs::read_to_string(&file).unwrap();
    let false_at = fixed.find("* False").expect("False arm present");
    let true_at = fixed.find("* True").expect("True arm present");
    assert!(false_at < true_at, "arms sorted on disk:\n{fixed}");
}

#[test]
fn format_and_semantic_errors_report_in_one_run() {
    // Fused, not gated: the format phase lives inside
    // `checker::check_loaded`, so it doesn't short-circuit the
    // semantic checker. Unsorted arms yield both the format error and
    // the arm-order check error from a single `canon check`.
    let file = scratch("fused").join("main.can");
    std::fs::write(&file, UNSORTED_ARMS).unwrap();

    let out = Command::new(canon_bin())
        .arg("check")
        .arg(&file)
        .output()
        .expect("canon check spawns");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr.contains("not canonically formatted")
            && stderr.contains("dispatch arms must follow the union's variant order"),
        "one run reports both phases, got:\n{stderr}"
    );
}

#[test]
fn fmt_is_not_a_command() {
    let out = Command::new(canon_bin())
        .args(["fmt", "main.can"])
        .output()
        .expect("canon spawns");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown command 'fmt'"), "got:\n{stderr}");
}
