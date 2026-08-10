//! Resolution is name → file, so a file whose declared names no
//! reference mentions is never read and its constructors never exist.
//! The failure used to surface at the call sites as `no method X on
//! type Y` — a description of the call, pointing nowhere near the file
//! that was skipped, and identical to what a genuinely wrong call
//! produces.
//!
//! The loader now reports it instead, naming the file and the rename
//! that fixes it. Confirming against the *finished* module is what
//! keeps it precise: a file grouping several declarations resolves
//! fine through any one of them, and must not be reported for the rest.

use std::path::PathBuf;
use std::process::Command;

fn canon_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_canon"))
}

/// Writes `files` into a fresh package `src/` and runs `canon check` on
/// it, returning (exit code, stderr).
fn check_package(name: &str, files: &[(&str, &str)]) -> (Option<i32>, String) {
    let src = std::env::temp_dir()
        .join(format!("canon_unnamed_{}_{}", name, std::process::id()))
        .join("src");
    let _ = std::fs::remove_dir_all(src.parent().unwrap());
    std::fs::create_dir_all(&src).unwrap();
    for (file, body) in files {
        std::fs::write(src.join(file), body).unwrap();
    }
    let out = Command::new(canon_bin())
        .arg("check")
        .arg(src.parent().unwrap())
        .output()
        .expect("canon check spawns");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

const MAIN: &str =
    "Unit => Program {\n    Greeting(\"hi\")\n        -> Loud\n        -> Print\n}\n";
const GREETING: &str = "Greeting = String\n";
const LOUD: &str = "Loud = String\n\nGreeting => Loud {\n    Greeting -> Uppercased\n}\n";

#[test]
fn an_unreachable_file_is_named_along_with_the_rename_that_fixes_it() {
    let (code, stderr) = check_package(
        "hidden",
        &[
            ("main.can", MAIN),
            ("greeting.can", GREETING),
            ("helpers.can", LOUD),
        ],
    );
    assert_eq!(
        code,
        Some(1),
        "the program does not compile, got:\n{stderr}"
    );
    assert!(
        stderr.contains("`Loud` is declared in") && stderr.contains("helpers.can"),
        "names the file that was skipped, got:\n{stderr}"
    );
    assert!(
        stderr.contains("loud.can"),
        "teaches the rename, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("no method `Loud`"),
        "replaces the call-site diagnostic rather than adding to it, got:\n{stderr}"
    );
}

#[test]
fn the_same_file_under_its_declared_name_compiles() {
    // Byte-identical content — only the filename differs.
    let (code, stderr) = check_package(
        "named",
        &[
            ("main.can", MAIN),
            ("greeting.can", GREETING),
            ("loud.can", LOUD),
        ],
    );
    assert_eq!(code, Some(0), "stderr:\n{stderr}");
}

#[test]
fn a_file_reached_through_one_of_its_names_is_not_reported_for_the_others() {
    // `resume.can` declares `Resume`, `WantedPay` and `WantedRole` —
    // the shape that bit the issue reporter, a file grouping related
    // declarations rather than defining one primary type. The reference
    // to `Resume` loads it, so the other two resolve from the module
    // and no rename is owed for either.
    let (code, stderr) = check_package(
        "grouped",
        &[
            (
                "main.can",
                "Unit => Program {\n    Resume(\"cv\")\n        -> WantedRole\n        -> Print\n}\n",
            ),
            (
                "resume.can",
                "Resume = String\n\nWantedPay = Resume\n\nWantedRole = Resume\n",
            ),
        ],
    );
    assert_eq!(code, Some(0), "stderr:\n{stderr}");
}

#[test]
fn builtin_vocabulary_is_never_blamed_on_a_sibling() {
    // `First` is builtin list vocabulary: it resolves without a file and
    // never appears among the module's items, so a sibling declaring
    // that name proves nothing about it.
    let (code, stderr) = check_package(
        "builtin",
        &[
            (
                "main.can",
                "Unit => Program {\n    List(\"a\" * \"b\") -> First -> (\n        * None { \"none\" -> Print }\n        * Some { \"some\" -> Print }\n    )\n}\n",
            ),
            ("first_holder.can", "First = String\n"),
        ],
    );
    assert_eq!(code, Some(0), "stderr:\n{stderr}");
}
