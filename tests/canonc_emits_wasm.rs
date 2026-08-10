//! The self-hosted compiler's output is real wasm, and it runs.
//!
//! `canonc` reads *Canon* source from a file named on the command line,
//! tokenizes it, parses the literal out of a declaration body, and emits
//! a WebAssembly core module whose exported `answer` returns it. One
//! declaration form of one language, but the input is Canon and the
//! output is wasm. The emission is hex rather than bytes because Canon
//! cannot write binary yet — `write` takes a UTF-8 `string` (see the
//! codegen gaps) — and hex sidesteps that without waiting on it.
//!
//! This is the end-to-end claim in one test: Canon-authored compiler
//! output, decoded, handed to wasmtime, executed, and the result checked
//! against the input it was derived from.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Run `canonc` over `source` and return what it wrote to stdout —
/// hex for a module it accepted, a diagnostic for one it rejected.
fn canonc_stdout(name: &str, source: &str) -> String {
    // Written where the test can point at it — `canonc` reads its input
    // path from the program arguments, so this exercises the real entry
    // rather than a baked-in string.
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target");
    path.push("canonc-input");
    fs::create_dir_all(&path).expect("create tmpdir");
    path.push(name);
    fs::write(&path, source).expect("write source");

    let out = Command::new(env!("CARGO_BIN_EXE_canon"))
        .args(["run", "canonc", path.to_str().expect("utf-8 path")])
        .output()
        .expect("canon run canonc");
    assert!(
        out.status.success(),
        "canon run canonc failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("utf-8 stdout")
        .trim()
        .to_string()
}

/// Compile `source` with `canonc`, run the wasm it emits, and return
/// what `answer` evaluates to.
fn canonc_answer(name: &str, source: &str) -> i32 {
    let hex = canonc_stdout(name, source);
    let hex = hex.as_str();

    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex pair"))
        .collect();
    assert_eq!(&bytes[..4], b"\0asm", "emitted bytes are a wasm module");

    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, &bytes).expect("emitted wasm should validate");
    let mut store = wasmtime::Store::new(&engine, ());
    let instance =
        wasmtime::Instance::new(&mut store, &module, &[]).expect("emitted wasm should instantiate");
    let answer = instance
        .get_typed_func::<(), i32>(&mut store, "answer")
        .expect("emitted module exports `answer`");
    answer.call(&mut store, ()).expect("call answer")
}

#[test]
fn canonc_output_is_wasm_that_runs() {
    // `canonc` read `Unit => Answer { 7 }` off disk, found the literal
    // in the declaration body, and emitted `i32.const 7`.
    assert_eq!(canonc_answer("seven.can", "Unit => Answer { 7 }\n"), 7);
}

#[test]
fn canonc_tokenizes_rather_than_scanning_for_digits() {
    // A digit inside an identifier is part of that word, not a literal.
    // Reading the source byte by byte would fold the `7` of `Answer7`
    // into the answer and return 73; the scanner runs each word to its
    // end and classifies it by its first byte, so only `3` is a number.
    assert_eq!(canonc_answer("word.can", "Unit => Answer7 { 3 }\n"), 3);
}

#[test]
fn canonc_takes_the_literal_from_the_declaration_body() {
    // The parser wants the literal that opens a brace body, so a
    // number loose in the declaration head is not the answer. The
    // tokenizer alone would have taken the first `Number` it saw and
    // returned 7.
    assert_eq!(canonc_answer("body.can", "Answer 7 { 3 }\n"), 3);
}

#[test]
fn canonc_reports_a_body_with_no_literal() {
    // No literal to emit, so `canonc` says so instead of emitting a
    // module for a number it never read. Before the parser this was a
    // silent `i32.const 0`.
    assert_eq!(
        canonc_stdout("empty.can", "Unit => Answer { }\n"),
        "declaration body is empty"
    );
}
