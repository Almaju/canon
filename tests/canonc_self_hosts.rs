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

/// Compile `source` with the compiler `canonc` emitted for its own
/// source, and return the hex that compiler prints. The second stage
/// runs on the host stack, so the whole thing goes on a thread deep
/// enough for a compiler with no loops: every walk over the input is a
/// recursion.
fn second_stage(source: &str) -> String {
    let source = source.to_string();
    std::thread::Builder::new()
        .stack_size(96 * 1024 * 1024)
        .spawn(move || {
            let mut config = wasmtime::Config::new();
            config.max_wasm_stack(64 * 1024 * 1024);
            config.async_stack_size(66 * 1024 * 1024);
            let engine = wasmtime::Engine::new(&config).expect("engine");
            let module =
                wasmtime::Module::new(&engine, decode(&canonc_on("canonc/src/compiled.can")))
                    .expect("the second-stage compiler should validate");
            let mut store = wasmtime::Store::new(&engine, ());
            let instance = wasmtime::Instance::new(&mut store, &module, &[])
                .expect("the self-compiled module should instantiate");
            let memory = instance
                .get_memory(&mut store, "memory")
                .expect("the self-compiled module should export its memory");
            // Room for the arena to bump into, with the source parked
            // above it — nothing is freed, so the whole compile has to
            // fit.
            let pages = 1024;
            let have = memory.size(&store);
            memory
                .grow(&mut store, pages - have)
                .expect("grow the arena");
            let at = ((pages - 64) * 65536) as usize;
            memory
                .write(&mut store, at, source.as_bytes())
                .expect("write the source into the arena");
            let (ptr, len) = instance
                .get_typed_func::<(i32, i32), (i32, i32)>(&mut store, "compiled")
                .expect("`compiled` takes a string and gives one back")
                .call(&mut store, (at as i32, source.len() as i32))
                .expect("run the second stage");
            let mut out = vec![0u8; len as usize];
            memory
                .read(&store, ptr as usize, &mut out)
                .expect("read the emitted hex");
            String::from_utf8(out).expect("utf-8 hex")
        })
        .expect("spawn the second stage")
        .join()
        .expect("join the second stage")
}

#[test]
fn the_second_stage_compiles_what_the_first_does() {
    // Written where both stages can read it: the first from the command
    // line, the second out of its own memory.
    let mut path = std::env::temp_dir();
    path.push("canonc-stage2.can");
    let source = "Answer = Int\n\nUnit => Answer { 2 -> Sum(3) -> Product(4) }\n";
    std::fs::write(&path, source).expect("write the input");

    let first = canonc_on(path.to_str().expect("utf-8 path"));
    assert_eq!(
        second_stage(source).trim(),
        first.trim(),
        "the compiler canonc emits should emit what canonc emits"
    );
}

#[test]
fn the_second_stage_compiles_the_compiler() {
    // The fixpoint. `canonc` compiled by the reference compiler reads
    // its own source and emits a compiler; that compiler, handed the
    // same source, emits the same bytes. From here the reference
    // compiler is a bootstrap host, not a dependency.
    let source = std::fs::read_to_string("canonc/src/compiled.can").expect("read the compiler");
    let first = canonc_on("canonc/src/compiled.can");
    let second = second_stage(&source);
    assert_eq!(
        second.len(),
        first.trim().len(),
        "the two stages should emit the same number of bytes"
    );
    assert_eq!(second, first.trim(), "the two stages should agree");
}
