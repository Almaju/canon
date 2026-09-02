//! `Stdin()` drains `wasi:cli/stdin`'s byte stream into one string.
//!
//! The binding's WIT shape is `tuple<stream<u8>, future<result<_,
//! error-code>>>`; Canon spells it `Unit => Result<Stdin, IoError>` and
//! codegen reads the stream to its end at the boundary
//! (`IndirectReturnShape::ByteStream`). These pin the drain against real
//! pipes: a few lines, an input longer than one read chunk, and nothing.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn run_with_stdin(name: &str, program: &str, input: &[u8]) -> String {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target");
    path.push("stdin-test");
    // One directory per program: a sibling `.can` in the same directory
    // would be discovered as part of the program.
    path.push(name);
    std::fs::create_dir_all(&path).expect("create tmpdir");
    path.push("main.can");
    std::fs::write(&path, program).expect("write program");
    let mut child = Command::new(env!("CARGO_BIN_EXE_canon"))
        .args(["run", path.to_str().expect("utf-8 path")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn canon run");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input)
        .expect("write stdin");
    let out = child.wait_with_output().expect("canon run");
    assert!(
        out.status.success(),
        "canon run failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 stdout")
}

const LINES: &str = "Unit => Result<Program, IoError> {
    Stdin()? -> Lines -> Sorted -> First -> (
        * None => Unit { \"empty\" -> Print }
        * Some<String> => Unit { String -> Print }
    )
    Stdin()?
        -> Lines
        -> Length
        -> Print
    Unit() -> Ok
}
";

const LENGTH: &str = "Unit => Result<Program, IoError> {
    Stdin()?
        -> Length
        -> Print
    Unit() -> Ok
}
";

#[test]
fn stdin_lines_are_a_list_of_strings() {
    // The second `Stdin()` finds the stream already drained: an empty
    // string, one (empty) line.
    assert_eq!(
        run_with_stdin("lines", LINES, b"pear\napple\nfig"),
        "apple\n1\n"
    );
}

#[test]
fn stdin_longer_than_one_chunk_is_read_whole() {
    let input = vec![b'x'; 300_000];
    assert_eq!(run_with_stdin("long", LENGTH, &input), "300000\n");
}

#[test]
fn empty_stdin_is_the_empty_string() {
    assert_eq!(run_with_stdin("empty", LENGTH, b""), "0\n");
}
