//! `canon add <git-url>@<tag>` vendors a package's source under
//! `deps/<owner>/<name>@<version>/`. A local repository stands in for the
//! remote: git clones a path the same way it clones a URL.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn canon_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_canon"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn git(dir: &PathBuf, args: &[&str]) {
    let out = Command::new("git")
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

#[test]
fn add_vendors_the_tagged_source_and_the_program_resolves_it() {
    let dir = scratch("add_basic");
    let upstream = dir.join("acme").join("greet");
    fs::create_dir_all(upstream.join("src")).unwrap();
    fs::write(
        upstream.join("src").join("shouted.can"),
        "Shouted = Uppercased\n\nString => Shouted {\n    String -> Uppercased\n}\n",
    )
    .unwrap();
    git(&upstream, &["init", "--quiet", "--initial-branch=main"]);
    git(
        &upstream,
        &["-c", "user.name=t", "-c", "user.email=t@t", "add", "."],
    );
    git(
        &upstream,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "-m",
            "greet",
        ],
    );
    git(&upstream, &["tag", "v1.2.0"]);

    let project = dir.join("app");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("src").join("main.can"),
        "Unit => Program {\n    Shouted(\"hello\") -> Print\n}\n",
    )
    .unwrap();

    let source = format!("{}@v1.2.0", upstream.display());
    let out = Command::new(canon_bin())
        .args(["add", &source])
        .current_dir(&project)
        .output()
        .expect("run canon add");
    assert!(
        out.status.success(),
        "canon add failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let vendored = project
        .join("deps")
        .join("acme")
        .join("greet@1.2.0")
        .join("shouted.can");
    assert!(
        vendored.is_file(),
        "source vendored at {}",
        vendored.display()
    );

    let out = Command::new(canon_bin())
        .args(["run", "."])
        .current_dir(&project)
        .output()
        .expect("run canon run");
    assert!(
        out.status.success(),
        "the program should resolve the vendored package:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "HELLO");

    let out = Command::new(canon_bin())
        .args(["add", &source])
        .current_dir(&project)
        .output()
        .expect("run canon add again");
    assert!(!out.status.success(), "adding twice is refused");
}

#[test]
fn add_requires_a_tag() {
    let dir = scratch("add_no_tag");
    let out = Command::new(canon_bin())
        .args(["add", "https://example.com/acme/greet"])
        .current_dir(&dir)
        .output()
        .expect("run canon add");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("<git-url>@<tag>"));
}
