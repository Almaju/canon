//! `canon add` vendors a package into a project's `deps/` tree from one
//! of two sources the argument's shape selects: a package bundled with
//! the toolchain (`canon/ansi`), or a git repository at a tag
//! (`<git-url>@<tag>`, where a local repository stands in for the
//! remote — git clones a path the same way it clones a URL).
//!
//! The bundled packages are invisible to name resolution until vendored
//! — that split is the whole point — so both halves are pinned: a
//! program reaching a package name fails to check until `canon add`
//! runs, and passes after.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn canon_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_canon"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("add-test-tmp")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// A fresh project holding one CLI entry with the given body.
fn project(name: &str, body: &str) -> PathBuf {
    let dir = scratch(name);
    fs::create_dir_all(dir.join("src")).expect("create src");
    fs::write(
        dir.join("src/main.can"),
        format!("Unit => Program {{\n{body}}}\n"),
    )
    .expect("write entry");
    dir
}

fn canon(args: &[&str], cwd: &Path) -> Output {
    Command::new(canon_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run canon")
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A git repository holding the `acme/greet` package, tagged `v1.2.0`.
fn upstream(dir: &Path) -> PathBuf {
    let upstream = dir.join("acme").join("greet");
    fs::create_dir_all(upstream.join("src")).unwrap();
    fs::write(
        upstream.join("src").join("shouted.can"),
        "Shouted = Uppercased\n\nString => Shouted {\n    String -> Uppercased\n}\n",
    )
    .unwrap();
    git(&upstream, &["init", "--quiet", "--initial-branch=main"]);
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "--quiet", "-m", "greet"]);
    git(&upstream, &["tag", "v1.2.0"]);
    upstream
}

#[test]
fn a_bundled_package_is_invisible_until_added_and_runs_after() {
    let dir = project(
        "bundled",
        "    \"ok\"\n        -> Styled(Green())\n        -> Print\n",
    );

    let before = canon(&["check", "."], &dir);
    assert!(!before.status.success(), "checked without the package");
    let stderr = String::from_utf8_lossy(&before.stderr);
    assert!(stderr.contains("Styled"), "stderr: {stderr}");

    let add = canon(&["add", "canon/ansi"], &dir);
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let vendored = dir
        .join("deps/canon")
        .join(format!("ansi@{}", canon::add::VERSION));
    assert!(vendored.join("styled.can").is_file(), "no vendored sources");
    assert_eq!(
        String::from_utf8_lossy(&add.stdout).trim(),
        format!(
            "added canon/ansi@{v} -> deps/canon/ansi@{v} (2 files)",
            v = canon::add::VERSION
        )
    );

    let run = canon(&["run", "."], &dir);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "\x1b[32mok\x1b[0m\n");
}

#[test]
fn a_git_tag_is_vendored_under_the_urls_owner_and_name() {
    let dir = scratch("git");
    let source = format!("{}@v1.2.0", upstream(&dir).display());
    let project = dir.join("app");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("src/main.can"),
        "Unit => Program {\n    Shouted(\"hello\") -> Print\n}\n",
    )
    .unwrap();

    let add = canon(&["add", &source], &project);
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    assert!(
        project.join("deps/acme/greet@1.2.0/shouted.can").is_file(),
        "the tag's `src/` is vendored under the URL's last two segments"
    );
    assert!(
        String::from_utf8_lossy(&add.stdout).starts_with("added acme/greet@1.2.0 -> "),
        "stdout: {}",
        String::from_utf8_lossy(&add.stdout)
    );

    let run = canon(&["run", "."], &project);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "HELLO");
}

#[test]
fn adding_replaces_another_vendored_version() {
    let dir = project("replace", "    Unit()\n");
    let stale = dir.join("deps/canon/ansi@0.0.1");
    fs::create_dir_all(&stale).expect("create stale copy");
    fs::write(stale.join("styled.can"), "Styled = String\n").expect("write stale copy");

    let add = canon(&["add", "canon/ansi", "src"], &dir);
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    assert!(!stale.exists(), "the stale version was left in place");
    assert!(
        String::from_utf8_lossy(&add.stdout).contains("replaced canon/ansi@0.0.1"),
        "stdout: {}",
        String::from_utf8_lossy(&add.stdout)
    );
    let entries = fs::read_dir(dir.join("deps/canon"))
        .expect("read deps")
        .count();
    assert_eq!(entries, 1, "one version of a package at a time");
}

#[test]
fn unknown_names_missing_tags_and_the_prelude_are_refused() {
    let dir = project("refused", "    Unit()\n");
    let unknown = canon(&["add", "canon/nope"], &dir);
    assert!(!unknown.status.success());
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(
        stderr.contains("no bundled package named `canon/nope`")
            && stderr.contains("canon/ansi")
            && stderr.contains("<git-url>@<tag>"),
        "stderr: {stderr}"
    );

    let untagged = canon(&["add", "https://example.com/acme/greet"], &dir);
    assert!(
        !untagged.status.success(),
        "a URL without a tag has no version"
    );

    let prelude = canon(&["add", "canon"], &dir);
    assert!(!prelude.status.success());
    assert!(
        String::from_utf8_lossy(&prelude.stderr).contains("prelude"),
        "the prelude cannot be added"
    );
    assert!(!dir.join("deps").exists(), "a refused add wrote something");
}

#[test]
fn no_package_lists_what_ships() {
    let out = canon(&["add"], Path::new("."));
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Usage: canon add <package>"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("  canon/ansi"), "stderr: {stderr}");
}
