//! `Stdin()` is `wasi:cli/stdin`'s byte stream as a `Stream<String>`.
//!
//! The binding's WIT shape is `tuple<stream<u8>, future<result<_,
//! error-code>>>`; Canon spells it `Unit => Result<Stdin, IoError>` with
//! `Stdin = Stream<String>`, and codegen reads a chunk per pull
//! (`stream::Stage::Host`). These pin the stream against real pipes:
//! drained whole, pulled chunk by chunk, an input longer than one read,
//! nothing, and pumped straight to stdout through `Printed`.

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
    // Written from a thread: a program that pumps stdin to stdout
    // (`Printed`) fills the stdout pipe while the input is still
    // going in, and `wait_with_output` drains stdout only afterwards.
    let mut stdin = child.stdin.take().expect("piped stdin");
    let input = input.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let out = child.wait_with_output().expect("canon run");
    writer.join().expect("stdin writer").expect("write stdin");
    assert!(
        out.status.success(),
        "canon run failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 stdout")
}

const LINES: &str = "Unit => Result<Program, IoError> {
    Stdin()? -> String -> Lines -> Sorted -> First -> (
        * None => Unit { \"empty\" -> Print }
        * Some<String> => Unit { String -> Print }
    )
    Stdin()?
        -> String
        -> Lines
        -> Length
        -> Print
    Unit() -> Ok
}
";

const LENGTH: &str = "Unit => Result<Program, IoError> {
    Stdin()?
        -> String
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

const UPPER: &str = "Unit => Result<Program, IoError> {
    Stdin()?
        -> Mapped((String) => Uppercased { String -> Uppercased })
        -> String
        -> Print
    Unit() -> Ok
}
";

const FIRST_CHUNK: &str = "Unit => Result<Program, IoError> {
    Stdin()? -> Taken(1) -> First -> (
        * None => Unit { \"none\" -> Print }
        * Some<String> => Unit { String -> Print }
    )
    Unit() -> Ok
}
";

const TOTAL: &str = "Total = Int

Unit => Result<Program, IoError> {
    Stdin()?
        -> Folded(Total(0) * (String * Total) => Total { Total -> Sum(String -> Length) -> Total })
        -> Print
    Unit() -> Ok
}
";

#[test]
fn stdin_chunks_map_before_the_drain() {
    assert_eq!(
        run_with_stdin("upper", UPPER, b"hello\nworld"),
        "HELLO\nWORLD\n"
    );
}

#[test]
fn stdin_pulls_one_chunk_at_a_time() {
    assert_eq!(run_with_stdin("first", FIRST_CHUNK, b"hi"), "hi\n");
    assert_eq!(run_with_stdin("first-empty", FIRST_CHUNK, b""), "none\n");
}

#[test]
fn stdin_folds_over_every_chunk() {
    let input = vec![b'x'; 300_000];
    assert_eq!(run_with_stdin("total", TOTAL, &input), "300000\n");
}

const PIPE: &str = "Unit => Result<Program, IoError> {
    Stdin()? -> Printed?
    Unit() -> Ok
}
";

const LOUD_PIPE: &str = "Loud = String

Unit => Result<Program, IoError> {
    Stdin()?
        -> Mapped((String) => Loud { Loud(`{String}!`) })
        -> Printed?
    Unit() -> Ok
}
";

#[test]
fn stdin_pumps_to_stdout_a_chunk_at_a_time() {
    let input = vec![b'x'; 300_000];
    assert_eq!(
        run_with_stdin("pipe", PIPE, &input).len(),
        300_000,
        "every byte comes back, no newline appended"
    );
    assert_eq!(run_with_stdin("pipe-empty", PIPE, b""), "");
    // One small write is one chunk.
    assert_eq!(
        run_with_stdin("loud-pipe", LOUD_PIPE, b"hello\nworld"),
        "hello\nworld!"
    );
}
