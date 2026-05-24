//! Slice 1b.1 prototype: prove that `wit-component` is a viable path for
//! emitting `wasi:http/service`-shaped components without hand-rolling
//! the type-section bytes for a 60-case variant and friends.
//!
//! The test is intentionally minimal: a single in-line WIT package
//! defining a `greet: func(name: string) -> string` export. We hand-build
//! a core module that imports nothing and exports `greet` with the right
//! canonical-ABI core signature `(i32, i32, i32) -> ()` (string in plus
//! a callee-allocated ret-area pointer for the string return — this is
//! what `cargo-component`-style guests emit). Then we embed the WIT
//! metadata as a custom section, run the whole thing through
//! `wit_component::ComponentEncoder`, and assert the result parses
//! back as a valid component.
//!
//! If this passes, the architectural shortcut described in
//! `WASI-HTTP-HANDLER.md` is sound: the actual codegen work for
//! `wasi:http/service` becomes "produce the right core module shape +
//! embed WIT, let wit-component do the type/lift/lower bytes".

use wasm_encoder::{
    CodeSection, ConstExpr, DataSection, ExportKind, ExportSection, Function, FunctionSection,
    Instruction, MemArg, MemorySection, MemoryType, Module, ValType,
};
use wit_component::{embed_component_metadata, ComponentEncoder, StringEncoding};
use wit_parser::Resolve;

/// Inline WIT package. Single function with one string param and one
/// string return — enough to exercise the canonical-ABI string lift
/// and lower without needing resources, variants, or sub-u64 ints.
const HELLO_WIT: &str = r#"
package test:hello@0.1.0;

interface api {
    greet: func(name: string) -> string;
}

world hello {
    export api;
}
"#;

/// Shape-equivalent of `wasi:http/service`, with all the bits the real
/// thing has that we *don't* currently lower by hand:
///
///   - **resources** (`request`, `response`) passed as `own<T>` handles
///     across the function boundary
///   - **sub-u64 ints** (`status-code = u16`)
///   - **variants** with payloads (`error-code` mirrors a slimmed-down
///     `wasi:http/error-code` — same structural shape: many cases, some
///     with `option<u16>` / `record` payloads)
///   - **`result<own<T>, error-code>`** as the handler's return shape
///
/// If `wit-component` accepts our core module against THIS WIT, it'll
/// accept it against the real `wasi:http/service` too — the encoder
/// doesn't care about the specific package or function names, only the
/// structural shapes of the types it has to lift/lower.
const HTTP_LIKE_WIT: &str = r#"
package test:httplike@0.1.0;

interface types {
    resource request;
    resource response {
        constructor(status: status-code);
    }

    type status-code = u16;

    record dns-error-payload {
        rcode: option<string>,
        info-code: option<u16>,
    }

    variant error-code {
        dns-timeout,
        dns-error(dns-error-payload),
        destination-unavailable,
        connection-refused,
        connection-timeout,
        request-body-size(option<u64>),
        internal-error(option<string>),
    }
}

interface handler {
    use types.{request, response, error-code};
    handle: async func(request: request) -> result<response, error-code>;
}

world service {
    import types;
    export handler;
}
"#;

#[test]
fn wit_component_round_trip_minimal_world() {
    // 1. Parse the WIT package into a Resolve and pick out the world id.
    let mut resolve = Resolve::default();
    let pkg = resolve.push_source("hello.wit", HELLO_WIT).unwrap();
    let world = resolve
        .select_world(&[pkg], Some("hello"))
        .expect("world `hello` exists in package");

    // 2. Build the core module:
    //    - one memory (exported as "memory")
    //    - one realloc helper (exported as "cabi_realloc")
    //    - the `greet` export — see signature notes below
    //
    //    For the canonical-ABI lowering wit-component will emit, an
    //    exported `func(string) -> string` whose return is too wide for
    //    the flat-result limit takes 3 i32s and returns nothing:
    //      (param str_ptr i32) (param str_len i32) (param ret_area i32)
    //    The function writes the return string's (ptr, len) into the
    //    ret_area. We satisfy this contract by returning a fixed
    //    "hello!" stored in the data section.
    let core_bytes = build_core_module();

    // 3. Embed the WIT metadata as a `component-type` custom section in
    //    the core module. `ComponentEncoder::module(…)` reads this on
    //    `encode()` to know which world to lift the module into.
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
        .expect("module accepted")
        .encode()
        .expect("component encoded");

    // 5. Validate the produced component parses cleanly through
    //    wasmparser at the component-model level.
    let mut validator = wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all());
    for payload in wasmparser::Parser::new(0).parse_all(&component_bytes) {
        let p = payload.expect("component bytes parse");
        validator.payload(&p).expect("component bytes validate");
    }
}

#[test]
fn wit_component_round_trip_http_like_world() {
    // This is the load-bearing prototype: same flow as the hello test
    // above, but the WIT carries resources + a payload-bearing variant
    // + sub-u64 ints. The core module we hand-build only needs to
    // import the response constructor (one i32 in, one i32 out) and
    // export the handler with the right canon-ABI core signature. All
    // the canonical-ABI lifts/lowers for the resource handles and the
    // error-code variant are emitted by `wit-component` automatically.
    let mut resolve = Resolve::default();
    let pkg = resolve
        .push_source("httplike.wit", HTTP_LIKE_WIT)
        .expect("http-like WIT parses");
    let world = resolve
        .select_world(&[pkg], Some("service"))
        .expect("world `service` exists");

    let core_bytes = build_http_like_core_module();

    let mut core_with_metadata = core_bytes;
    embed_component_metadata(
        &mut core_with_metadata,
        &resolve,
        world,
        StringEncoding::UTF8,
    )
    .expect("embed component metadata");

    let component_bytes = ComponentEncoder::default()
        .validate(true)
        .module(&core_with_metadata)
        .expect("module accepted by encoder")
        .encode()
        .expect("component encoded");

    let mut validator = wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all());
    for payload in wasmparser::Parser::new(0).parse_all(&component_bytes) {
        let p = payload.expect("component bytes parse");
        validator.payload(&p).expect("component bytes validate");
    }
}

/// Construct a minimal core module for the http-like world above.
///
/// Imports:
///   - `test:httplike/types@0.1.0` instance with a `[constructor]response`
///     function taking `u16` and returning an i32 (the new own-handle).
///
/// Exports:
///   - `memory`, `cabi_realloc` (canonical-ABI plumbing).
///   - `test:httplike/handler@0.1.0#handle` — takes the request handle
///     (i32) and returns the response handle (i32). At the canonical
///     ABI level the result is `result<own<response>, error-code>`
///     which lowers to a callee-allocated indirect return: the core
///     function returns an i32 ret-area pointer; the host reads the
///     discriminant + payload out of the ret area.
fn build_http_like_core_module() -> Vec<u8> {
    use wasm_encoder::{EntityType, ImportSection, TypeSection};

    // ── Type section ─────────────────────────────────────────────────
    // type 0: response constructor import — (i32) -> i32
    // type 1: cabi_realloc — (i32, i32, i32, i32) -> i32
    // type 2: handle export — (i32) -> i32  (request handle in, ret-area
    //          pointer out; canon ABI lifts the ret area into
    //          `result<own<response>, error-code>`)
    let mut types = TypeSection::new();
    types.ty().function([ValType::I32], [ValType::I32]); // 0
    types.ty().function(
        [ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        [ValType::I32],
    ); // 1
    types.ty().function([ValType::I32], [ValType::I32]); // 2

    // ── Import section ───────────────────────────────────────────────
    // Bring the response constructor in from the types instance under
    // the same naming convention wit-component uses.
    let mut imports = ImportSection::new();
    imports.import(
        "test:httplike/types@0.1.0",
        "[constructor]response",
        EntityType::Function(0),
    );

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
    funcs.function(1); // cabi_realloc (defined func 0; import takes slot 0 so this is slot 1)
    funcs.function(2); // handle       (slot 2)

    // ── Export section ───────────────────────────────────────────────
    let mut exports = ExportSection::new();
    exports.export("memory", ExportKind::Memory, 0);
    // Function index layout:
    //   0: imported [constructor]response
    //   1: cabi_realloc (first defined)
    //   2: handle      (second defined)
    exports.export("cabi_realloc", ExportKind::Func, 1);
    exports.export("test:httplike/handler@0.1.0#handle", ExportKind::Func, 2);

    // ── Code section ─────────────────────────────────────────────────
    let mut codes = CodeSection::new();

    // cabi_realloc stub. Never exercised in this test (the handler
    // returns a fixed-offset ret-area).
    {
        let mut f = Function::new([]);
        f.instruction(&Instruction::I32Const(64));
        f.instruction(&Instruction::End);
        codes.function(&f);
    }

    // handle: ignore the incoming request, call the imported
    // `[constructor]response` with status 200, write
    // `Ok(<response_handle>)` into the ret-area, return the ret-area
    // pointer.
    //
    // The canon-ABI shape for `result<own<response>, error-code>` is:
    //   byte 0: discriminant (0 = ok, 1 = err)
    //   byte 4+: payload (response handle for ok, variant for err)
    // We pick ret-area at fixed offset 192 (well past the data section).
    {
        let mut f = Function::new([(1, ValType::I32)]); // local: response_handle
                                                        // response_handle = response_constructor(200)
        f.instruction(&Instruction::I32Const(200));
        f.instruction(&Instruction::Call(0)); // imported response constructor
        f.instruction(&Instruction::LocalSet(1));

        // ret_area[0] = 0 (Ok discriminant)
        f.instruction(&Instruction::I32Const(192));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Store(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        // ret_area[4] = response_handle
        f.instruction(&Instruction::I32Const(192));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Store(MemArg {
            offset: 4,
            align: 2,
            memory_index: 0,
        }));

        // Return ret-area pointer.
        f.instruction(&Instruction::I32Const(192));
        f.instruction(&Instruction::End);
        codes.function(&f);
    }

    let mut m = Module::new();
    m.section(&types);
    m.section(&imports);
    m.section(&funcs);
    m.section(&memories);
    m.section(&exports);
    m.section(&codes);
    m.finish()
}

/// Construct a minimal core module that exports `greet` matching the
/// callee-allocated indirect-return string ABI.
fn build_core_module() -> Vec<u8> {
    // ── Type section ─────────────────────────────────────────────────
    // type 0: cabi_realloc — (i32, i32, i32, i32) -> i32
    // type 1: greet — (i32, i32) -> i32  (callee-allocated indirect
    //          return: function returns the ret-area pointer; host
    //          reads `(out_ptr, out_len)` from offsets 0/4 of it)
    let mut types = wasm_encoder::TypeSection::new();
    types.ty().function(
        [ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        [ValType::I32],
    );
    types
        .ty()
        .function([ValType::I32, ValType::I32], [ValType::I32]);

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
    funcs.function(0); // cabi_realloc
    funcs.function(1); // greet

    // ── Export section ───────────────────────────────────────────────
    // Interface-bound exports use the `<iface>#<func>` name convention
    // that `wit-component` looks for. For freestanding (world-level)
    // exports the name is just `<func>`.
    let mut exports = ExportSection::new();
    exports.export("memory", ExportKind::Memory, 0);
    exports.export("cabi_realloc", ExportKind::Func, 0);
    exports.export("test:hello/api@0.1.0#greet", ExportKind::Func, 1);

    // ── Code section ─────────────────────────────────────────────────
    let mut codes = CodeSection::new();

    // cabi_realloc: trivial bump-style stub. wit-component requires the
    // export to exist; for this prototype we never actually exercise it
    // (the static `hello!` string lives at a fixed data-section offset).
    {
        let mut f = Function::new([]);
        // params: old_ptr (0), old_size (1), align (2), new_size (3)
        // return the new_size'd allocation: just return 0 + 64 (placeholder).
        f.instruction(&Instruction::I32Const(64));
        f.instruction(&Instruction::End);
        codes.function(&f);
    }

    // greet: ignore the input string, allocate an 8-byte ret-area at a
    // fixed offset (192), write the static "hello!" (ptr=128, len=6)
    // into the ret-area, return the ret-area pointer.
    {
        let mut f = Function::new([]);
        // params: str_ptr (0), str_len (1) — both ignored
        // ret_area[0] = 128 (string ptr)
        f.instruction(&Instruction::I32Const(192));
        f.instruction(&Instruction::I32Const(128));
        f.instruction(&Instruction::I32Store(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        // ret_area[4] = 6 (string len)
        f.instruction(&Instruction::I32Const(192));
        f.instruction(&Instruction::I32Const(6));
        f.instruction(&Instruction::I32Store(MemArg {
            offset: 4,
            align: 2,
            memory_index: 0,
        }));
        // Return the ret-area pointer.
        f.instruction(&Instruction::I32Const(192));
        f.instruction(&Instruction::End);
        codes.function(&f);
    }

    // ── Data section: "hello!" at offset 128 ─────────────────────────
    let mut data = DataSection::new();
    data.active(0, &ConstExpr::i32_const(128), b"hello!".iter().copied());

    let mut m = Module::new();
    m.section(&types);
    m.section(&funcs);
    m.section(&memories);
    m.section(&exports);
    m.section(&codes);
    m.section(&data);
    m.finish()
}
