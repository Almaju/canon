//! Streaming slice 1 prototype: prove `wit-component` is a viable path
//! for emitting components that carry `stream<u8>` at the canonical-ABI
//! boundary.
//!
//! This is the streams analogue of `wit_component_prototype.rs`, which
//! validated the same shortcut for resources + variants + sub-u64 ints
//! (the WASI HTTP handler shape). If both prototypes pass, the
//! architectural claim in `WASI-HTTP-HANDLER.md` and `STREAMING.md`
//! holds: the codegen's remaining responsibility for these features
//! shrinks to "produce a core module with the right `(import …)` +
//! `(export …)` names and let `wit-component` emit the canonical-ABI
//! type-section bytes."
//!
//! Concretely this test proves:
//!
//!   - A WIT function returning `stream<u8>` lifts cleanly: the core
//!     export's `() -> i32` signature is what the encoder expects, and
//!     it produces a valid component.
//!   - A WIT function taking `stream<u8>` as a parameter lowers
//!     cleanly: the core export's `(i32) -> u32` signature is accepted,
//!     and the encoder wires the canonical-ABI handle plumbing for us.
//!   - The encoder validates the produced component against
//!     wasmparser's strict component validation — no hand-written type
//!     section, no per-stream lowering code in our codegen.
//!
//! Once `tests/wit_component_prototype.rs` and this test both pass, the
//! slice 1b (HTTP) and slice 1 (streams) integration work share the
//! same path: switch the main codegen from hand-rolled `wasm-encoder`
//! type/component sections to `wit-component::ComponentEncoder` for any
//! program whose surface mentions resources, streams, or anything else
//! the encoder handles for free.
//!
//! The test does **not** execute the component — there's no host yet
//! that provides a guest with a `stream<u8>` to read from in this
//! test rig. Execution comes in slice 2 when the main codegen path
//! actually emits these.

use wasm_encoder::{
    CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction, MemorySection,
    MemoryType, Module, TypeSection, ValType,
};
use wit_component::{embed_component_metadata, ComponentEncoder, StringEncoding};
use wit_parser::Resolve;

/// Inline WIT package with two stream-bearing functions. Both
/// signatures are the minimum-viable shape for slice 1: a producer
/// that returns `stream<u8>` and a consumer that takes one.
const STREAMS_WIT: &str = r#"
package test:streams@0.1.0;

interface api {
    /// Produce a stream of bytes. At the canonical ABI this lowers to
    /// a core `() -> i32` (the new stream handle).
    make: func() -> stream<u8>;

    /// Consume a stream of bytes, returning how many were read.
    /// At the canonical ABI this lowers to a core `(i32) -> u32`.
    read-count: func(s: stream<u8>) -> u32;
}

world streams {
    export api;
}
"#;

#[test]
fn wit_component_round_trip_stream_world() {
    // 1. Parse the WIT package into a Resolve and pick out the world.
    let mut resolve = Resolve::default();
    let pkg = resolve
        .push_source("streams.wit", STREAMS_WIT)
        .expect("streams WIT parses");
    let world = resolve
        .select_world(&[pkg], Some("streams"))
        .expect("world `streams` exists");

    // 2. Build a core module exporting `make` and `read-count` with
    //    the canonical-ABI signatures wit-component expects when these
    //    functions are lifted.
    let core_bytes = build_streams_core_module();

    // 3. Embed the WIT metadata.
    let mut core_with_metadata = core_bytes;
    embed_component_metadata(
        &mut core_with_metadata,
        &resolve,
        world,
        StringEncoding::UTF8,
    )
    .expect("embed component metadata");

    // 4. Run through the component encoder.
    let component_bytes = ComponentEncoder::default()
        .validate(true)
        .module(&core_with_metadata)
        .expect("module accepted by encoder")
        .encode()
        .expect("component encoded");

    // 5. Validate the produced component parses through wasmparser's
    //    full component-model validator.
    let mut validator = wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all());
    for payload in wasmparser::Parser::new(0).parse_all(&component_bytes) {
        let p = payload.expect("component bytes parse");
        validator.payload(&p).expect("component bytes validate");
    }
}

/// Hand-built core module exporting `make` and `read-count` against
/// the WIT above. Both stubs return constant values — the prototype
/// only validates the encoder accepts the shape, not that the
/// functions are useful.
///
/// Exports:
///   - `memory`                              (canonical-ABI required)
///   - `cabi_realloc`                        (canonical-ABI required)
///   - `test:streams/api@0.1.0#make`         core `() -> i32`
///   - `test:streams/api@0.1.0#read-count`   core `(i32) -> i32`
fn build_streams_core_module() -> Vec<u8> {
    // ── Type section ─────────────────────────────────────────────────
    // type 0: cabi_realloc — (i32, i32, i32, i32) -> i32
    // type 1: make         — () -> i32
    // type 2: read-count   — (i32) -> i32
    let mut types = TypeSection::new();
    types.ty().function(
        [ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        [ValType::I32],
    );
    types.ty().function([], [ValType::I32]);
    types.ty().function([ValType::I32], [ValType::I32]);

    // ── Memory section ───────────────────────────────────────────────
    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: 1,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });

    // ── Function section ─────────────────────────────────────────────
    let mut funcs = FunctionSection::new();
    funcs.function(0); // cabi_realloc (defined func 0)
    funcs.function(1); // make         (defined func 1)
    funcs.function(2); // read-count   (defined func 2)

    // ── Export section ───────────────────────────────────────────────
    let mut exports = ExportSection::new();
    exports.export("memory", ExportKind::Memory, 0);
    exports.export("cabi_realloc", ExportKind::Func, 0);
    exports.export("test:streams/api@0.1.0#make", ExportKind::Func, 1);
    exports.export("test:streams/api@0.1.0#read-count", ExportKind::Func, 2);

    // ── Code section ─────────────────────────────────────────────────
    let mut codes = CodeSection::new();

    // cabi_realloc stub. Never exercised here; required to exist.
    {
        let mut f = Function::new([]);
        f.instruction(&Instruction::I32Const(64));
        f.instruction(&Instruction::End);
        codes.function(&f);
    }

    // make: return a fake handle value (0). In a real component this
    // would be a freshly-allocated stream handle the host gives us.
    // For shape-validation purposes any i32 works.
    {
        let mut f = Function::new([]);
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::End);
        codes.function(&f);
    }

    // read-count: ignore the incoming handle, return 0. Same as above —
    // the encoder only inspects the core signature, not the behaviour.
    {
        let mut f = Function::new([]);
        // params: stream_handle (i32) — ignored
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::End);
        codes.function(&f);
    }

    let mut m = Module::new();
    m.section(&types);
    m.section(&funcs);
    m.section(&memories);
    m.section(&exports);
    m.section(&codes);
    m.finish()
}
