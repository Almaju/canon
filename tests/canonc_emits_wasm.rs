//! The self-hosted compiler's output is real wasm, and it runs.
//!
//! `canonc` parses an arithmetic expression and emits a WebAssembly core
//! module whose exported `answer` computes it — one `i32.const` per
//! operand and an `i32.add` between them, so the instruction sequence is
//! generated from the source, not templated. The emission is hex
//! rather than bytes because Canon cannot write binary yet — `write`
//! takes a UTF-8 `string` (see the codegen gaps) — and hex sidesteps
//! that without waiting on it.
//!
//! This is the end-to-end claim in one test: Canon-authored compiler
//! output, decoded, handed to wasmtime, executed, and the result checked
//! against the input it was derived from.

use std::process::Command;

#[test]
fn canonc_output_is_wasm_that_runs() {
    let out = Command::new(env!("CARGO_BIN_EXE_canon"))
        .args(["run", "canonc"])
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

    // `canonc` compiled the expression `1+2+3`: three `i32.const`s and
    // two `i32.add`s, generated from the operands it found in the source.
    // Running them gives 6.
    assert_eq!(answer.call(&mut store, ()).expect("call answer"), 6);
}
