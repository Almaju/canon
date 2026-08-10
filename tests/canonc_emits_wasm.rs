//! The self-hosted compiler's output is real wasm, and it runs.
//!
//! `canonc` reads *Canon* source from a file named on the command line —
//! a single nullary constructor whose body is an integer literal — and
//! emits a WebAssembly core module whose exported `answer` returns that
//! literal. One declaration form of one
//! language, but the input is Canon and the output is wasm. The emission
//! is hex
//! rather than bytes because Canon cannot write binary yet — `write`
//! takes a UTF-8 `string` (see the codegen gaps) — and hex sidesteps
//! that without waiting on it.
//!
//! This is the end-to-end claim in one test: Canon-authored compiler
//! output, decoded, handed to wasmtime, executed, and the result checked
//! against the input it was derived from.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn canonc_output_is_wasm_that_runs() {
    // The source `canonc` compiles, written where the test can point at
    // it — `canonc` reads its input path from the program arguments, so
    // this exercises the real entry rather than a baked-in string.
    let mut source = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    source.push("target");
    source.push("canonc-input");
    fs::create_dir_all(&source).expect("create tmpdir");
    source.push("seven.can");
    fs::write(&source, "Unit => Answer { 7 }\n").expect("write source");

    let out = Command::new(env!("CARGO_BIN_EXE_canon"))
        .args(["run", "canonc", source.to_str().expect("utf-8 path")])
        .output()
        .expect("canon run canonc");
    assert!(
        out.status.success(),
        "canon run canonc failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    let hex = stdout.trim();

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

    // `canonc` read `Unit => Answer { 7 }` off disk, found the literal in
    // the declaration body, and emitted `i32.const 7`.
    assert_eq!(answer.call(&mut store, ()).expect("call answer"), 7);
}
