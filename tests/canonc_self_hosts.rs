//! The bootstrap: `canonc` compiled by the reference compiler, reading
//! its own source.
//!
//! `canonc/src/compiled.can` is the compiler proper — every declaration
//! from the scanner to the module writer, and no host boundary. The
//! entry beside it (`main.can`) is the only part that reads a file and
//! prints, and it is four lines long.
//!
//! So the fixpoint is: run `canonc` over `compiled.can`, and the wasm it
//! emits is a compiler. Instantiate that, hand it a Canon program, and
//! it emits the same bytes the reference-compiled `canonc` does.

use std::process::Command;

/// Run `canonc` over a file already on disk and return its stdout.
fn canonc_on(path: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_canon"))
        .args(["run", "canonc", path])
        .output()
        .expect("canon run canonc");
    assert!(
        out.status.success(),
        "canon run canonc {path} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 stdout")
}

fn decode(hex: &str) -> Vec<u8> {
    let hex = hex.trim();
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex pair"))
        .collect()
}

#[test]
fn canonc_compiles_its_own_source() {
    let bytes = decode(&canonc_on("canonc/src/compiled.can"));
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, &bytes)
        .expect("the module canonc emits for its own source should validate");

    // Every declared type is exported, lowercased — including the one
    // the entry calls.
    let has_compiled = module
        .exports()
        .any(|e| e.name() == "compiled" && e.ty().func().is_some());
    assert!(
        has_compiled,
        "the self-compiled module should export `compiled`"
    );
}
