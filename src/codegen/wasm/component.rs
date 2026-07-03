//! Component Model wrapper.
//!
//! Takes the core WASM module produced by `mod::generate_core_module` and
//! wraps it into a WebAssembly **Component Model** component targeting
//! **WASI Preview 3**. The resulting component:
//!
//!   - Imports `wasi:cli/stdout@0.3.0-rc-2026-03-15.write-via-stream` —
//!     the native WASI Preview 3 stdout interface. The compiler emits the
//!     full canonical-ABI stream sequence (`stream.new<u8>`,
//!     `stream.write<u8>`, `stream.drop-writable<u8>`,
//!     `future.drop-readable`) so the resulting `.wasm` is portable to any
//!     compliant WASI P3 runtime — it does **not** import any
//!     `canon:*` interface for output.
//!   - Exports `wasi:cli/run.run` — wasmtime's command entry point.
//!
//! ## Architecture
//!
//! Two core modules are linked together inside the component:
//!
//! 1. A trivial **memory provider** that exports a fresh linear memory.
//! 2. The **user core module** (output of `generate_core_module`) which
//!    imports that memory as `env.memory`, imports the five WASI P3
//!    stdout canonical builtins from a synthetic `wasi:cli/stdout`
//!    instance, and exports `run() -> i32` whose result is the
//!    canonical-ABI flattening of `result<_, _>` (0 = ok, 1 = err).
//!
//! Canonical-ABI `lower`s for string-passing imports need a `Memory` option
//! that points at an *already-instantiated* core memory. Instantiating the
//! memory provider first, then aliasing its `memory` export before lowering
//! `write-via-stream`, breaks the would-be cycle (the user core module
//! would otherwise need its own memory to exist before its imports could
//! be supplied).
//!
//! All instantiations and aliases happen at the component level and use the
//! standard Component Model section ordering.
//!
//! ## Canonical stdout stream sequence
//!
//! WASI P3 replaces P2's `output-stream` resource with a Component-Model
//! `stream<u8>`. The guest writes a byte buffer to stdout by emitting:
//!
//! ```text
//! (handles : i64) = canon stream.new<u8>()
//!   reader = low32(handles)
//!   writer = high32(handles)
//! (future : i32) = canon lower wasi:cli/stdout.write-via-stream (reader)
//! _ = canon stream.write<u8> [memory 0] (writer, ptr, len)
//! _ = canon stream.drop-writable<u8> (writer)
//! _ = canon future.drop-readable<future<…>> (future)
//! ```
//!
//! `stream.drop-writable` signals end-of-stream to wasmtime-wasi's host
//! pump, which flushes the bytes to the OS stdout file descriptor.
//! `future.drop-readable` discards the unused completion handle (we don't
//! need to know whether the write succeeded; the host has already accepted
//! ownership of the data and will retry/log as appropriate).

use wasm_encoder::{
    Alias, BlockType, CanonicalFunctionSection, CanonicalOption, CodeSection, Component,
    ComponentAliasSection, ComponentExportKind, ComponentExportSection, ComponentImportSection,
    ComponentInstanceSection, ComponentSection, ComponentSectionId, ComponentTypeRef,
    ComponentTypeSection, ComponentValType, ConstExpr, Encode, ExportKind, ExportSection, Function,
    FunctionSection, GlobalSection, GlobalType, InstanceSection, InstanceType, Instruction, MemArg,
    MemorySection, MemoryType, Module, ModuleArg, ModuleSection, PrimitiveValType, TypeBounds,
    TypeSection, ValType,
};

use super::MEM_HEAP_START;

/// Embeds an already-encoded core WASM module (as raw bytes) into a component
/// as a `CoreModule` section. `wasm-encoder` only exposes `ModuleSection`
/// taking `&Module`, but we need to wrap pre-encoded bytes coming out of the
/// codegen, so we implement the section trait ourselves.
struct RawModuleSection<'a>(&'a [u8]);

impl Encode for RawModuleSection<'_> {
    fn encode(&self, sink: &mut Vec<u8>) {
        // The Component Model encodes a module section as the module's bytes
        // prefixed by its length (LEB128). `Vec<u8>::encode` does exactly this.
        self.0.encode(sink);
    }
}

impl ComponentSection for RawModuleSection<'_> {
    fn id(&self) -> u8 {
        ComponentSectionId::CoreModule.into()
    }
}

use super::{ExternImport, IndirectReturnShape, ParamKind};
use crate::ast::{Item, Module as OModule};
use crate::codegen::async_analysis::AsyncSet;
use std::collections::BTreeMap;

/// The component-level import name for WASI Preview 3 stdout. This is the
/// instance name wasmtime (or any WASI P3 host) matches against the linker.
pub(super) const WASI_CLI_STDOUT_COMPONENT_IMPORT: &str = "wasi:cli/stdout@0.3.0-rc-2026-03-15";

/// The *core-module* import name. This is a private contract between the
/// emitted core module and the component wrapper; it just needs to match
/// the `(import "wasi:cli/stdout" "…" …)` declarations the core module
/// emits for the five canonical-ABI builtins it relies on for `.print`.
pub(super) const WASI_CLI_STDOUT_CORE_IMPORT: &str = "wasi:cli/stdout";

/// WASI Preview 3 cli/run interface name.
pub(super) const WASI_CLI_RUN: &str = "wasi:cli/run@0.3.0-rc-2026-03-15";

/// WASI Preview 3 http/handler interface name. Emitted as the
/// component-level export for programs whose entry has a
/// `(Request) -> Response` signature. See `WASI-HTTP-HANDLER.md`.
pub(super) const WASI_HTTP_HANDLER: &str = "wasi:http/handler@0.3.0-rc-2026-03-15";

/// The vendored WASI Preview 3 WIT sources needed to resolve the
/// `wasi:http/service` world. `http.wit` pulls in `wasi:clocks` via a
/// `use`; the world imports interfaces from `wasi:cli` and
/// `wasi:random`; and `wasi:cli`'s own worlds reference
/// `wasi:filesystem` and `wasi:sockets` — so the whole vendored set
/// must be in the `Resolve`. Order matters: dependencies before
/// dependents.
const WIT_WASI_CLOCKS: &str = include_str!("../../../wit-vendor/wasi/clocks.wit");
const WIT_WASI_FILESYSTEM: &str = include_str!("../../../wit-vendor/wasi/filesystem.wit");
const WIT_WASI_SOCKETS: &str = include_str!("../../../wit-vendor/wasi/sockets.wit");
const WIT_WASI_CLI: &str = include_str!("../../../wit-vendor/wasi/cli.wit");
const WIT_WASI_RANDOM: &str = include_str!("../../../wit-vendor/wasi/random.wit");
const WIT_WASI_HTTP: &str = include_str!("../../../wit-vendor/wasi/http.wit");

/// The core-module import namespace for `wasi:http/types` functions and
/// intrinsics. This is the `<iface>@<ver>` name `wit-component` matches
/// import clauses against when componentising.
pub(super) const WASI_HTTP_TYPES_MODULE: &str = "wasi:http/types@0.3.0-rc-2026-03-15";

/// The vendored WASI WIT packages, parsed once. Shared between the
/// HTTP world emission and the WIT-informed extern lowering (which
/// consults the true WIT signature of every `wasi:*` extern import to
/// honour narrow integer widths).
pub(super) fn vendored_resolve() -> &'static wit_parser::Resolve {
    static RESOLVE: std::sync::OnceLock<wit_parser::Resolve> = std::sync::OnceLock::new();
    RESOLVE.get_or_init(|| {
        let mut resolve = wit_parser::Resolve::default();
        // `exit-with-code` is `@unstable(feature = cli-exit-with-code)`
        // in the vendored WIT; the runtime opts into it too (see
        // `LinkOptions::cli_exit_with_code` in `src/runtime.rs`). Keep
        // this a targeted opt-in — `all_features` would also pull the
        // unstable `wasi:clocks/timezone` import into the embedded
        // `wasi:http/service` world, which hosts don't provide.
        resolve.features.insert("cli-exit-with-code".to_string());
        for (name, source) in [
            ("clocks.wit", WIT_WASI_CLOCKS),
            ("filesystem.wit", WIT_WASI_FILESYSTEM),
            ("sockets.wit", WIT_WASI_SOCKETS),
            ("random.wit", WIT_WASI_RANDOM),
            ("cli.wit", WIT_WASI_CLI),
            ("http.wit", WIT_WASI_HTTP),
        ] {
            resolve
                .push_source(name, source)
                .unwrap_or_else(|e| panic!("vendored {name} does not parse: {e:?}"));
        }
        resolve
    })
}

/// Resolves a WIT type to its primitive value type, following `type
/// x = y` alias chains. `None` for strings and every compound shape.
fn wit_prim(resolve: &wit_parser::Resolve, t: &wit_parser::Type) -> Option<PrimitiveValType> {
    use wit_parser::Type as T;
    match t {
        T::Bool => Some(PrimitiveValType::Bool),
        T::U8 => Some(PrimitiveValType::U8),
        T::U16 => Some(PrimitiveValType::U16),
        T::U32 => Some(PrimitiveValType::U32),
        T::U64 => Some(PrimitiveValType::U64),
        T::S8 => Some(PrimitiveValType::S8),
        T::S16 => Some(PrimitiveValType::S16),
        T::S32 => Some(PrimitiveValType::S32),
        T::S64 => Some(PrimitiveValType::S64),
        T::F32 => Some(PrimitiveValType::F32),
        T::F64 => Some(PrimitiveValType::F64),
        T::Id(id) => match &resolve.types[*id].kind {
            wit_parser::TypeDefKind::Type(inner) => wit_prim(resolve, inner),
            _ => None,
        },
        _ => None,
    }
}

/// Looks up a `wasi:*` extern URN
/// (`"wasi:cli/exit@<ver>#exit-with-code"`) in the vendored WIT and
/// returns the primitive value types of its parameters and result.
/// Inner `None` entries are non-primitive shapes (strings, options,
/// …) the caller should leave to the existing lowering. Outer `None`
/// when the URN doesn't resolve (unknown interface or function).
pub(super) type ExternPrimSig = (
    Vec<Option<PrimitiveValType>>,
    Option<Option<PrimitiveValType>>,
);

pub(super) fn vendored_extern_prim_sig(urn: &str) -> Option<ExternPrimSig> {
    let resolve = vendored_resolve();
    let (iface_ver, fn_name) = urn.split_once('#')?;
    let iface_full = iface_ver.split_once('@').map_or(iface_ver, |(i, _)| i);
    let (ns_pkg, iface_name) = iface_full.split_once('/')?;
    let (ns, pkg) = ns_pkg.split_once(':')?;
    let pkg_id = resolve
        .package_names
        .iter()
        .find_map(|(name, id)| (name.namespace == ns && name.name == pkg).then_some(*id))?;
    let iface_id = *resolve.packages[pkg_id].interfaces.get(iface_name)?;
    let func = resolve.interfaces[iface_id].functions.get(fn_name)?;
    let params = func
        .params
        .iter()
        .map(|p| wit_prim(resolve, &p.ty))
        .collect();
    let result = func.result.as_ref().map(|t| wit_prim(resolve, t));
    Some((params, result))
}

/// HTTP-entry programs route here. Unlike the CLI path (which
/// hand-rolls every component section via `wasm-encoder`), the HTTP
/// path delegates all canonical-ABI type emission to `wit-component`:
///
///   1. `super::generate_http_core_module` compiles the user program
///      into a self-contained core module (own memory, own
///      `cabi_realloc`, `wit-component` import naming) whose
///      `wasi:http/handler@…#handle` export calls the user's compiled
///      `(Request) -> Response` function — see `WasmGen::compile_http`.
///   2. Embed the parsed `wasi:http/service` world as component-type
///      metadata (`wit_component::embed_component_metadata`).
///   3. Run the result through `wit_component::ComponentEncoder`, which
///      emits every resource/variant/option lift & lower for us.
pub(super) fn wrap_http_service(module: &OModule) -> Vec<u8> {
    let resolve = vendored_resolve();
    let http_pkg = resolve
        .package_names
        .iter()
        .find_map(|(name, id)| (name.namespace == "wasi" && name.name == "http").then_some(*id))
        .expect("wasi:http package present in resolve");
    let world = resolve
        .select_world(&[http_pkg], Some("service"))
        .expect("wasi:http declares a `service` world");

    let mut core = super::generate_http_core_module(module);
    wit_component::embed_component_metadata(
        &mut core,
        resolve,
        world,
        wit_component::StringEncoding::UTF8,
    )
    .expect("embed wasi:http/service metadata");

    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(&core)
        .expect("core module matches the wasi:http/service world")
        .encode()
        .expect("component encoding succeeds")
}

/// Builds the Component Model component. Returns the binary `.wasm`.
///
/// `externs` lists every `extern Wasm` function the user program declared,
/// already deduplicated and assigned core function indices by the codegen.
/// Each one becomes a component-level instance import and a lowered core
/// function passed into the user core module's instantiation.
///
/// `_async_set` is the suspending-function set from
/// `async_analysis::analyse`. It is plumbed through for the upcoming
/// `CanonicalOption::Async` integration; the current wrapper still emits
/// sync lifts/lowers across the board, so the value is read but unused at
/// the binary level. Once async lowering is implemented, this is where
/// per-import and per-export decisions will be made.
pub(super) fn wrap(
    core_module: &[u8],
    externs: &[ExternImport],
    _async_set: &AsyncSet,
    has_handler: bool,
) -> Vec<u8> {
    // Group externs by core namespace so we build one instance per interface.
    // BTreeMap keeps the iteration order deterministic (alphabetical).
    let mut by_iface: BTreeMap<&str, Vec<&ExternImport>> = BTreeMap::new();
    for ext in externs {
        by_iface
            .entry(ext.core_namespace.as_str())
            .or_default()
            .push(ext);
    }
    // Sort each group's functions alphabetically too.
    for fns in by_iface.values_mut() {
        fns.sort_by(|a, b| a.fn_name.cmp(&b.fn_name));
    }

    let mut c = Component::new();

    // ── 1. Component type section ───────────────────────────────────────────
    //   0: instance type for `wasi:cli/stdout`
    //   1: defined type `result<_, _>` (return shape of `run`)
    //   2: func type `() -> result<_, _>` (signature of `run`)
    //   3: defined type `error-code` enum (top-level — referenced by
    //      the canon-builtin type indices below)
    //   4: defined type `result<_, error-code>`
    //   5: defined type `stream<u8>`
    //   6: defined type `future<result<_, error-code>>`
    //   7..: one instance type per extern Wasm interface
    //
    // The extern instance types are appended in iface-alphabetical order so
    // their indices line up with the imports section below.
    let stdout_instance_type_idx: u32 = 0;
    let run_func_type_idx: u32 = 2;
    let stream_u8_type_idx: u32 = 5;
    let future_result_type_idx: u32 = 6;
    let mut extern_iface_type_idx: BTreeMap<&str, u32> = BTreeMap::new();
    // Captured after the types section closes — used by the optional
    // handler-export code at the bottom of `wrap` to know what the next
    // free component-type index is when declaring the handler's
    // top-level function type.
    let next_top_level_type_idx: u32;
    {
        let mut types = ComponentTypeSection::new();

        // Type 0 — wasi:cli/stdout interface.
        //
        // The interface declares its own copies of `error-code`,
        // `result<_, error-code>`, `stream<u8>`, and
        // `future<result<_, error-code>>` so the embedded function type
        // is self-contained (no instance-type aliases needed). The case
        // order of `error-code` matches the WIT source order
        // (`io`, `illegal-byte-sequence`, `pipe`) — canonical-ABI enum
        // discriminants are assigned in declaration order, *not*
        // alphabetical order; preserving the source order is required
        // for wire compatibility.
        // The validator requires that every defined type referenced by an
        // imported function be "named" — i.e. exported from the
        // surrounding instance under some name. `error-code` (an enum)
        // and `result<_, error-code>` must therefore be exported.
        // `stream<u8>` only references the primitive `u8` so it doesn't
        // need a name, but we export it as well to keep the WIT shape
        // self-documenting. `future<…>` references `result<…>` which is
        // already named.
        // Build-up sequence (each line consumes one local type slot in
        // the instance type's index space). Crucially, downstream
        // references *must* go through the exported aliases — only the
        // alias's type identity is inserted into the validator's
        // "imported named types" set, so referencing the original
        // anonymous definitions would make the instance fail the
        // `all_valtypes_named` check.
        //
        //   slot 0 — defined enum (anonymous)
        //   slot 1 — exported `error-code = type 0`        (named alias)
        //   slot 2 — defined `result<_, type 1>`           (uses the alias!)
        //   slot 3 — exported `write-result = type 2`      (named alias)
        //   slot 4 — defined `stream<u8>`
        //   slot 5 — defined `future<type 3>`              (uses the result alias!)
        //   slot 6 — function (data: type 4) -> type 5
        //   exported `write-via-stream = func 6`
        let mut stdout_iface = InstanceType::new();
        stdout_iface
            .ty()
            .defined_type()
            .enum_type(["io", "illegal-byte-sequence", "pipe"]);
        stdout_iface.export("error-code", ComponentTypeRef::Type(TypeBounds::Eq(0)));
        stdout_iface
            .ty()
            .defined_type()
            .result(None, Some(ComponentValType::Type(1)));
        stdout_iface.export("write-result", ComponentTypeRef::Type(TypeBounds::Eq(2)));
        stdout_iface
            .ty()
            .defined_type()
            .stream(Some(ComponentValType::Primitive(PrimitiveValType::U8)));
        stdout_iface
            .ty()
            .defined_type()
            .future(Some(ComponentValType::Type(3)));
        {
            let mut fn_enc = stdout_iface.ty().function();
            fn_enc.params([("data", ComponentValType::Type(4))]);
            fn_enc.result(Some(ComponentValType::Type(5)));
        }
        stdout_iface.export("write-via-stream", ComponentTypeRef::Func(6));
        types.instance(&stdout_iface);

        // Type 1 — result<_, _>
        types.defined_type().result(None, None);

        // Type 2 — async func() -> result<_,_>  (signature of `run`)
        //
        // Declared async to match the `CanonicalOption::Async` we attach
        // to the lift below. Async-stackful semantics let `run` (and
        // anything transitively called from it) suspend on
        // `waitable-set.wait` when an `extern Wasm.async` import returns
        // a non-Returned status — see `WasmGen::emit_async_call`.
        {
            let mut fn_enc = types.function();
            fn_enc.async_(true);
            fn_enc.params(Vec::<(&str, PrimitiveValType)>::new());
            fn_enc.result(Some(ComponentValType::Type(1)));
        }

        // Type 3 — top-level `error-code` enum. Distinct from the copy
        // inside the stdout instance type — the canonical-ABI builtins
        // (`stream.new`, `stream.write`, `stream.drop-writable`,
        // `future.drop-readable`) reference these *top-level* type
        // indices, while the lowered `write-via-stream` reads its
        // signature from the imported instance type. wit-component / the
        // wasm validator check the two are structurally compatible.
        types
            .defined_type()
            .enum_type(["io", "illegal-byte-sequence", "pipe"]);

        // Type 4 — top-level result<_, error-code>
        types
            .defined_type()
            .result(None, Some(ComponentValType::Type(3)));

        // Type 5 — top-level stream<u8>
        types
            .defined_type()
            .stream(Some(ComponentValType::Primitive(PrimitiveValType::U8)));

        // Type 6 — top-level future<result<_, error-code>>
        types.defined_type().future(Some(ComponentValType::Type(4)));

        // Type 7+ — one instance type per extern Wasm interface.
        let mut next_type_idx: u32 = 7;
        for (iface_name, fns) in &by_iface {
            let mut iface_ty = InstanceType::new();
            // Inside the instance, define each func type then export it.
            // Each `extern Wasm` function gets two indices inside the
            // instance type:
            //   - a `defined_type` entry for `string` (only when the function
            //     returns one), followed by
            //   - a `function` type entry that uses it.
            // We track the running local type counter manually so the func
            // exports can reference whichever type they need.
            let mut next_local_ty: u32 = 0;
            for ext in fns.iter() {
                // Indirect-return shapes need a defined-type entry first so
                // the function type can reference it.
                let return_ty_idx = match &ext.indirect_return {
                    Some(IndirectReturnShape::String) => {
                        iface_ty
                            .ty()
                            .defined_type()
                            .primitive(PrimitiveValType::String);
                        let idx = next_local_ty;
                        next_local_ty += 1;
                        Some(idx)
                    }
                    Some(IndirectReturnShape::ResultStringString { .. }) => {
                        iface_ty.ty().defined_type().result(
                            Some(ComponentValType::Primitive(PrimitiveValType::String)),
                            Some(ComponentValType::Primitive(PrimitiveValType::String)),
                        );
                        let idx = next_local_ty;
                        next_local_ty += 1;
                        Some(idx)
                    }
                    None => None,
                };
                let mut fn_enc = iface_ty.ty().function();
                // `extern Wasm.async` functions must declare an async
                // function type at the WIT level so the validator pairs
                // them with `CanonicalOption::Async` on the canon.lower.
                // wasm-encoder 0.250 exposes this via `async_(true)`,
                // which must be called *before* `params()` / `result()`.
                fn_enc.async_(ext.is_async);
                let params = extern_params_to_component(ext);
                let param_iter: Vec<(&str, ComponentValType)> =
                    params.iter().map(|(n, t)| (n.as_str(), *t)).collect();
                fn_enc.params(param_iter);
                let result = if let Some(idx) = return_ty_idx {
                    Some(ComponentValType::Type(idx))
                } else {
                    extern_result_to_component(ext)
                };
                fn_enc.result(result);
                let fn_ty_idx = next_local_ty;
                next_local_ty += 1;
                iface_ty.export(&ext.fn_name, ComponentTypeRef::Func(fn_ty_idx));
            }
            types.instance(&iface_ty);
            extern_iface_type_idx.insert(*iface_name, next_type_idx);
            next_type_idx += 1;
        }

        // Optional handler-request instance type. When the user defined
        // `handleRequest = (String) -> String`, the wrapper exports an
        // `canon:http-handler/handler@0.1.0` instance carrying
        // `handle-request: func(body: string) -> string`. The host's
        // HTTP server runtime looks up this instance after
        // instantiation and invokes it per request.
        if has_handler {
            let mut handler_iface_ty = InstanceType::new();
            // The function type is the only thing in this instance; it
            // takes one string param `body` and returns a string.
            let mut fn_enc = handler_iface_ty.ty().function();
            fn_enc.params([(
                "body",
                ComponentValType::Primitive(PrimitiveValType::String),
            )]);
            fn_enc.result(Some(ComponentValType::Primitive(PrimitiveValType::String)));
            // Function-type index inside this instance type is 0 (it's
            // the first definition we add).
            handler_iface_ty.export("handle-request", ComponentTypeRef::Func(0));
            types.instance(&handler_iface_ty);
            // Track the resulting top-level type index for the export
            // step below by reusing `next_type_idx`.
            extern_iface_type_idx.insert("__canon_http_handler__", next_type_idx);
            next_type_idx += 1;
        }

        next_top_level_type_idx = next_type_idx;
        c.section(&types);
    }
    let _ = stdout_instance_type_idx; // documented for readability

    // ── 2. Component imports ─────────────────────────────────────────────────
    // Component instance index 0 = imported wasi:cli/stdout.
    // Component instances 1.. = imported extern Wasm interfaces, in the same
    // iface-alphabetical order as the types above.
    let mut extern_iface_instance_idx: BTreeMap<&str, u32> = BTreeMap::new();
    {
        let mut imports = ComponentImportSection::new();
        imports.import(
            WASI_CLI_STDOUT_COMPONENT_IMPORT,
            ComponentTypeRef::Instance(0),
        );
        for (next_instance, iface_name) in (1u32..).zip(by_iface.keys()) {
            let ty_idx = extern_iface_type_idx[iface_name];
            // The component-level import name should include the version. We
            // derive it from the first function's `component_namespace`,
            // which already carries it.
            let component_name = by_iface[iface_name][0].component_namespace.as_str();
            imports.import(component_name, ComponentTypeRef::Instance(ty_idx));
            extern_iface_instance_idx.insert(*iface_name, next_instance);
        }
        c.section(&imports);
    }

    // ── 3. Alias every imported func into the component-level func space ──
    // → component func 0 = wasi:cli/stdout.write-via-stream
    // → component funcs 1.. = extern Wasm funcs, in (iface, fn-name) order.
    let mut extern_component_fn_idx: BTreeMap<(&str, &str), u32> = BTreeMap::new();
    {
        let mut aliases = ComponentAliasSection::new();
        aliases.alias(Alias::InstanceExport {
            instance: 0,
            kind: ComponentExportKind::Func,
            name: "write-via-stream",
        });
        let mut next_func: u32 = 1;
        for (iface_name, fns) in &by_iface {
            let inst = extern_iface_instance_idx[iface_name];
            for ext in fns {
                aliases.alias(Alias::InstanceExport {
                    instance: inst,
                    kind: ComponentExportKind::Func,
                    name: &ext.fn_name,
                });
                extern_component_fn_idx.insert((*iface_name, ext.fn_name.as_str()), next_func);
                next_func += 1;
            }
        }
        c.section(&aliases);
    }

    // ── 4. Inline memory-provider core module ────────────────────────────────
    let memory_module = build_memory_module();
    c.section(&ModuleSection(&memory_module));
    // → core module 0
    c.section(&RawModuleSection(core_module));
    // → core module 1

    // ── 5. Instantiate the memory-provider core module ───────────────────────
    {
        let mut insts = InstanceSection::new();
        insts.instantiate::<[(String, ModuleArg); 0], String>(0, []);
        c.section(&insts);
    }
    // → core instance 0 = memory provider

    // ── 6. Alias memory, bump_ptr, and cabi_realloc from the helper module ──
    // These three are exported by `build_memory_module` and are needed by the
    // canonical-ABI lowers below (Memory for string params, Realloc for
    // indirect string returns).
    {
        let mut aliases = ComponentAliasSection::new();
        aliases.alias(Alias::CoreInstanceExport {
            instance: 0,
            kind: ExportKind::Memory,
            name: "memory",
        });
        aliases.alias(Alias::CoreInstanceExport {
            instance: 0,
            kind: ExportKind::Func,
            name: "cabi_realloc",
        });
        c.section(&aliases);
    }
    // → core memory 0, core func 0 = cabi_realloc
    let cabi_realloc_core_fn: u32 = 0;

    // ── 7. Lower `write-via-stream` and every extern func against memory ───
    // After the cabi_realloc alias above, the next core function index is 1,
    // so lowered `write-via-stream` = core func 1 and lowered externs start
    // at 2. write-via-stream is a *sync* function in the WIT (it returns a
    // future synchronously), so it lowers without options — the flat ABI
    // collapses `(data: stream<u8>) -> future<…>` to `(i32) -> i32` (one
    // stream-readable handle in, one future handle out).
    {
        let mut canon = CanonicalFunctionSection::new();
        canon.lower(
            0, // component func 0 = wasi:cli/stdout.write-via-stream
            std::iter::empty::<CanonicalOption>(),
        );
        for (iface_name, fns) in &by_iface {
            for ext in fns {
                let comp_fn = extern_component_fn_idx[&(*iface_name, ext.fn_name.as_str())];
                if ext.is_async {
                    // Async lower (`CanonicalOption::Async`) collapses the
                    // core signature to `(i32, i32) -> i32`: an arg-area
                    // pointer + a ret-area pointer go in, a subtask
                    // status code comes out. The validator always
                    // requires `Memory`, and `Realloc` when the result
                    // (or any param) holds a pointer — we include both
                    // unconditionally since extern Wasm.async functions
                    // typically traffic in strings / result<…, string>.
                    canon.lower(
                        comp_fn,
                        [
                            CanonicalOption::UTF8,
                            CanonicalOption::Memory(0),
                            CanonicalOption::Realloc(cabi_realloc_core_fn),
                            CanonicalOption::Async,
                        ],
                    );
                } else if ext.indirect_return.is_some() {
                    // Any indirect-return shape (string, result<string,string>)
                    // needs memory + realloc so the host can allocate the
                    // result buffer(s) in guest memory.
                    canon.lower(
                        comp_fn,
                        [
                            CanonicalOption::UTF8,
                            CanonicalOption::Memory(0),
                            CanonicalOption::Realloc(cabi_realloc_core_fn),
                        ],
                    );
                } else if extern_uses_strings(ext) {
                    canon.lower(comp_fn, [CanonicalOption::UTF8, CanonicalOption::Memory(0)]);
                } else {
                    canon.lower(comp_fn, std::iter::empty::<CanonicalOption>());
                }
            }
        }
        // ── 7b. Canon intrinsics for guest-side async wait ─────────────
        //
        // Emit the canonical-ABI async-wait intrinsics the guest calls
        // from `emit_async_call` when the imported async function
        // returns a non-Returned status, plus `task.return` which `run`'s
        // async-stackful lift uses to deliver its `result<_, _>` value.
        // They take no component-level function inputs — they just
        // declare core functions implementing the canon operators. The
        // order here must match the order in which the user core module
        // imports them (see `mod.rs::compile`, `canon:async/waitable.*`):
        //   set-new        → ()         -> i32
        //   join           → (i32, i32) -> ()
        //   set-wait       → (i32, i32) -> i32   (memory = core memory 0)
        //   set-drop       → (i32)      -> ()
        //   subtask-drop   → (i32)      -> ()
        //   task-return    → (i32)      -> ()   (discriminant of result<_,_>)
        //   subtask-cancel → (i32)      -> i32  (used by `race` to abandon the loser)
        canon.waitable_set_new();
        canon.waitable_join();
        canon.waitable_set_wait(false, 0);
        canon.waitable_set_drop();
        canon.subtask_drop();
        // `task.return` is parameterised by the result type. For `run` it
        // returns the `result<_, _>` (component type index 1) declared
        // above; canon.task_return lowers it to a core function
        // `(i32) -> ()` because `result<_, _>` flattens to one i32 tag.
        canon.task_return(
            Some(ComponentValType::Type(1)),
            std::iter::empty::<CanonicalOption>(),
        );
        // `subtask.cancel` with `async_ = false` blocks the calling task
        // (allowed because our `run` is lifted async-stackful) until the
        // cancellation is observed, then returns the new state code.
        // `compile_race` drops the state code after the call.
        canon.subtask_cancel(false);

        // ── 7c. Canon stream/future builtins for native stdout output ───
        //
        // These four define the canonical-ABI helpers `print_str` calls
        // around `write-via-stream`. They each emit one new core
        // function; their indices are computed below as
        // `8+N .. 11+N` (after cabi_realloc + write-via-stream + N
        // externs + 6 waitable defs). The type indices passed in are
        // the *top-level* `stream<u8>` (5) and
        // `future<result<_, error-code>>` (6) declared in section §1.
        canon.stream_new(stream_u8_type_idx);
        canon.stream_write(stream_u8_type_idx, [CanonicalOption::Memory(0)]);
        canon.stream_drop_writable(stream_u8_type_idx);
        canon.future_drop_readable(future_result_type_idx);
        c.section(&canon);
    }

    // Core func indices in the post-lower order:
    //   0           = cabi_realloc (aliased above)
    //   1           = lowered write-via-stream
    //   2..1+N      = lowered externs in (iface, fn) order
    //   2+N..8+N    = 7 waitable / task / cancel intrinsics
    //   9+N..12+N   = 4 stream/future canon builtins for stdout
    let write_via_stream_core_fn: u32 = 1;
    let mut extern_core_fn_idx: BTreeMap<(&str, &str), u32> = BTreeMap::new();
    {
        let mut next: u32 = 2;
        for (iface_name, fns) in &by_iface {
            for ext in fns {
                extern_core_fn_idx.insert((*iface_name, ext.fn_name.as_str()), next);
                next += 1;
            }
        }
    }
    let waitable_set_new_core_fn: u32 = 2 + externs.len() as u32;
    let waitable_join_core_fn: u32 = waitable_set_new_core_fn + 1;
    let waitable_set_wait_core_fn: u32 = waitable_set_new_core_fn + 2;
    let waitable_set_drop_core_fn: u32 = waitable_set_new_core_fn + 3;
    let subtask_drop_core_fn: u32 = waitable_set_new_core_fn + 4;
    let task_return_core_fn: u32 = waitable_set_new_core_fn + 5;
    let subtask_cancel_core_fn: u32 = waitable_set_new_core_fn + 6;
    // Stream/future stdout builtins live immediately after the
    // waitable group; their indices are 9+N .. 12+N.
    let stream_new_core_fn: u32 = waitable_set_new_core_fn + 7;
    let stream_write_core_fn: u32 = waitable_set_new_core_fn + 8;
    let stream_drop_writable_core_fn: u32 = waitable_set_new_core_fn + 9;
    let future_drop_readable_core_fn: u32 = waitable_set_new_core_fn + 10;

    // ── 8. Synthetic core instances, one per import-module ──────────
    // The user core module's `(import "<core-namespace>" "<fn>" ...)` clauses
    // require one synthetic instance per `<core-namespace>` whose exports
    // contain the right functions.
    //   - wasi:cli/stdout:               core instance 1
    //   - extern iface k:                core instance 2+k (in BTreeMap order)
    //   - canon:async/waitable:         core instance 2 + by_iface.len()
    {
        let mut insts = InstanceSection::new();
        // wasi:cli/stdout synthetic instance — bundles the lowered
        // `write-via-stream` together with the four canonical-ABI
        // builtins the user core module's `print_str` calls around it.
        // All five live under the `wasi:cli/stdout` module-import name
        // (purely a private contract; nothing outside this component
        // observes these synthetic exports).
        insts.export_items([
            (
                "write-via-stream",
                ExportKind::Func,
                write_via_stream_core_fn,
            ),
            ("stream-new", ExportKind::Func, stream_new_core_fn),
            ("stream-write", ExportKind::Func, stream_write_core_fn),
            (
                "stream-drop-writable",
                ExportKind::Func,
                stream_drop_writable_core_fn,
            ),
            (
                "future-drop-readable",
                ExportKind::Func,
                future_drop_readable_core_fn,
            ),
        ]);
        // one synthetic instance per extern interface
        for (iface_name, fns) in &by_iface {
            let exports: Vec<(&str, ExportKind, u32)> = fns
                .iter()
                .map(|ext| {
                    let core_fn = extern_core_fn_idx[&(*iface_name, ext.fn_name.as_str())];
                    (ext.fn_name.as_str(), ExportKind::Func, core_fn)
                })
                .collect();
            insts.export_items(exports);
        }
        // `canon:async/waitable` synthetic instance — always present so the
        // user core module's imports section is shape-stable.
        insts.export_items([
            ("set-new", ExportKind::Func, waitable_set_new_core_fn),
            ("join", ExportKind::Func, waitable_join_core_fn),
            ("set-wait", ExportKind::Func, waitable_set_wait_core_fn),
            ("set-drop", ExportKind::Func, waitable_set_drop_core_fn),
            ("subtask-drop", ExportKind::Func, subtask_drop_core_fn),
            ("task-return", ExportKind::Func, task_return_core_fn),
            ("subtask-cancel", ExportKind::Func, subtask_cancel_core_fn),
        ]);
        c.section(&insts);
    }

    // Compute the core-instance indices of the synthetic instances.
    let stdout_synth_inst: u32 = 1;
    let mut extern_synth_inst: BTreeMap<&str, u32> = BTreeMap::new();
    {
        for (next, iface_name) in (2u32..).zip(by_iface.keys()) {
            extern_synth_inst.insert(*iface_name, next);
        }
    }
    let waitable_synth_inst: u32 = 2 + by_iface.len() as u32;

    // ── 9. Instantiate the user core module ──────────────────────
    {
        let mut insts = InstanceSection::new();
        // `env` provides both memory and bump_ptr to the user module — they
        // come from the same helper module that owns `cabi_realloc`.
        let mut args: Vec<(String, ModuleArg)> = vec![
            ("env".to_string(), ModuleArg::Instance(0)),
            (
                WASI_CLI_STDOUT_CORE_IMPORT.to_string(),
                ModuleArg::Instance(stdout_synth_inst),
            ),
        ];
        for iface_name in by_iface.keys() {
            args.push((
                iface_name.to_string(),
                ModuleArg::Instance(extern_synth_inst[iface_name]),
            ));
        }
        // Waitable intrinsics module-import — matches the import names
        // declared by `mod.rs::compile` for the `canon:async/waitable`
        // group.
        args.push((
            "canon:async/waitable".to_string(),
            ModuleArg::Instance(waitable_synth_inst),
        ));
        insts.instantiate(1, args);
        c.section(&insts);
    }
    // → core instance N = user core module, where N = 3 + by_iface.len()

    let user_core_instance: u32 = 3 + by_iface.len() as u32;

    // ── 10. Alias the `run` export of the user core module ──────────────
    {
        let mut aliases = ComponentAliasSection::new();
        aliases.alias(Alias::CoreInstanceExport {
            instance: user_core_instance,
            kind: ExportKind::Func,
            name: "run",
        });
        c.section(&aliases);
    }
    // Core func index for `run`: after
    //   cabi_realloc(0),
    //   lowered write-via-stream(1),
    //   N lowered externs (2..2+N),
    //   7 waitable+task canon intrinsics (2+N..9+N),
    //   4 stream/future canon builtins (9+N..13+N),
    // it's `13 + N`.
    let run_core_fn: u32 = 13 + externs.len() as u32;

    // ── 11. Lift it as the typed `wasi:cli/run.run` ─────────────────────
    // Async-stackful lift — the core function's wasm signature is
    // `() -> ()` and the result is delivered via `task.return`. This is
    // what lets nested `extern Wasm.async` calls suspend on
    // `waitable-set.wait` without tripping wasmtime's "cannot block a
    // synchronous task" check.
    {
        let mut canon = CanonicalFunctionSection::new();
        canon.lift(run_core_fn, run_func_type_idx, [CanonicalOption::Async]);
        c.section(&canon);
    }
    // → component func (1 + num_extern_funcs) = lifted run
    let run_component_fn: u32 = 1 + externs.len() as u32;
    let _ = cabi_realloc_core_fn;

    // ── 12. Build a component instance carrying { run } ───────────────────
    let wasi_run_instance: u32 = 1 + by_iface.len() as u32;
    {
        let mut comp_insts = ComponentInstanceSection::new();
        comp_insts.export_items([("run", ComponentExportKind::Func, run_component_fn)]);
        c.section(&comp_insts);
    }

    // ── 13. Export it as `wasi:cli/run@0.3.0-rc-…` ──────────────────
    {
        let mut exports = ComponentExportSection::new();
        exports.export(
            WASI_CLI_RUN,
            ComponentExportKind::Instance,
            wasi_run_instance,
            None,
        );
        c.section(&exports);
    }

    // ── 14–17. Optional dynamic handler export. Only emitted when the
    // program defines `handleRequest = (String) -> String`. The
    // architecture matches `wasi:cli/run`: alias the wrapper's core
    // function out of the user core instance, lift it with the
    // canonical-ABI string-indirect-return convention, wrap in a
    // component instance, and export under a stable interface name.
    if has_handler {
        // 14. Alias `__handle_request` from the user core instance.
        //     This is the core function index of the synthesised
        //     wrapper emitted by `WasmGen::build_handler_wrapper`.
        {
            let mut aliases = ComponentAliasSection::new();
            aliases.alias(Alias::CoreInstanceExport {
                instance: user_core_instance,
                kind: ExportKind::Func,
                name: "__handle_request",
            });
            c.section(&aliases);
        }
        // Core func index of the aliased user function. It sits one
        // slot after `run` in the aliased-core-functions sequence.
        // `run` was the previous alias and lives at `13 + N`, so the
        // handler is at `14 + N`.
        let handler_core_fn: u32 = 14 + externs.len() as u32;

        // 15. Lift the user function directly as
        //     `func(body: string) -> string`. The user function's core
        //     signature `(i32, i32) -> (i32, i32)` already matches the
        //     canonical-ABI direct multi-value return shape under the
        //     default `MAX_FLAT_RESULTS=16`, so no wrapper is needed.
        //     The lift options give the canonical-ABI machinery access
        //     to guest memory and the realloc helper for marshalling
        //     the string params/results across the boundary.
        //
        //     The function type is declared in a fresh type section
        //     below; `next_top_level_type_idx` was captured right after
        //     the original types section closed, so it names the index
        //     this new function type will receive.
        let handler_fn_type_idx = next_top_level_type_idx;
        {
            let mut more_types = ComponentTypeSection::new();
            let mut fn_enc = more_types.function();
            fn_enc.params([(
                "body",
                ComponentValType::Primitive(PrimitiveValType::String),
            )]);
            fn_enc.result(Some(ComponentValType::Primitive(PrimitiveValType::String)));
            c.section(&more_types);
        }

        {
            let mut canon = CanonicalFunctionSection::new();
            canon.lift(
                handler_core_fn,
                handler_fn_type_idx,
                [
                    CanonicalOption::UTF8,
                    CanonicalOption::Memory(0),
                    CanonicalOption::Realloc(cabi_realloc_core_fn),
                ],
            );
            c.section(&canon);
        }
        // The lifted handler is the next component-level function after
        // the existing `run` lift, so its index is `run_component_fn + 1`.
        let handler_component_fn: u32 = run_component_fn + 1;

        // 16. Wrap in a component instance carrying `handle-request`.
        //
        // Instance-index counting: the existing `wasi:cli/run` export
        // bumps the instance index by *two* — once for the wrapper
        // instance created in section 12, once again for the
        // exported-instance entry in section 13. So our handler
        // wrapper instance sits at `wasi_run_instance + 2`.
        let handler_instance_idx: u32 = wasi_run_instance + 2;
        {
            let mut comp_insts = ComponentInstanceSection::new();
            comp_insts.export_items([(
                "handle-request",
                ComponentExportKind::Func,
                handler_component_fn,
            )]);
            c.section(&comp_insts);
        }

        // 17. Export the instance as `canon:http-handler/handler@0.1.0`.
        {
            let mut exports = ComponentExportSection::new();
            exports.export(
                "canon:http-handler/handler@0.1.0",
                ComponentExportKind::Instance,
                handler_instance_idx,
                None,
            );
            c.section(&exports);
        }
    }

    c.finish()
}

/// Converts a single `ExternImport`'s logical parameters into
/// `(name, ComponentValType)` pairs. `component_params` is already structured
/// so each entry corresponds to one Canon argument — strings show up as a
/// single `ParamKind::String` rather than two `i32` slots — so we just map
/// directly into the component-level type space.
fn extern_params_to_component(ext: &ExternImport) -> Vec<(String, ComponentValType)> {
    ext.component_params
        .iter()
        .enumerate()
        .map(|(i, kind)| {
            let ty = match kind {
                ParamKind::Scalar(prim) => ComponentValType::Primitive(*prim),
                ParamKind::String => ComponentValType::Primitive(PrimitiveValType::String),
            };
            (format!("arg{i}"), ty)
        })
        .collect()
}

fn extern_result_to_component(ext: &ExternImport) -> Option<ComponentValType> {
    let vt = ext.results.first()?;
    // WIT-informed result type wins when present (exact width and
    // signedness from the vendored WIT).
    if let Some(prim) = ext.component_result {
        return Some(ComponentValType::Primitive(prim));
    }
    let signed = !ext.component_namespace.starts_with("wasi:");
    Some(ComponentValType::Primitive(match vt {
        wasm_encoder::ValType::I32 if signed => PrimitiveValType::S32,
        wasm_encoder::ValType::I32 => PrimitiveValType::U32,
        wasm_encoder::ValType::I64 if signed => PrimitiveValType::S64,
        wasm_encoder::ValType::I64 => PrimitiveValType::U64,
        wasm_encoder::ValType::F32 => PrimitiveValType::F32,
        wasm_encoder::ValType::F64 => PrimitiveValType::F64,
        _ => PrimitiveValType::U32,
    }))
}

/// Does this extern's signature touch guest memory? True if any string param
/// is present — the lower needs `Memory(0)` so the host can read the bytes.
/// Indirect returns are detected separately via `ExternImport::indirect_return`.
fn extern_uses_strings(ext: &ExternImport) -> bool {
    ext.component_params
        .iter()
        .any(|p| matches!(p, ParamKind::String))
}

/// Builds the helper core module that the component wrapper instantiates
/// first. It owns the linear memory, the shared bump pointer, and a tiny
/// `cabi_realloc` implementation:
///
/// ```wat
/// (module
///   (memory (export "memory") 2)
///   (global (export "bump_ptr") (mut i32) (i32.const 65536))
///   (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
///     global.get $bump_ptr
///     global.get $bump_ptr
///     local.get 3        ;; new_size
///     i32.add
///     global.set $bump_ptr))
/// ```
///
/// The host uses `cabi_realloc` to allocate buffers in guest memory when
/// lowering an indirect-returning function (a `string` or `record` return).
/// `old_ptr`, `old_size`, and `align` are ignored — the bump allocator never
/// frees and 4-byte alignment is sufficient for everything we currently lower.
///
/// The user core module imports `env.memory` and `env.bump_ptr` from this
/// module, so its `$alloc` and the host's `cabi_realloc` share one heap.
fn build_memory_module() -> Module {
    let mut m = Module::new();

    // Type 0: cabi_realloc signature.
    let mut types = TypeSection::new();
    types.ty().function(
        [ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        [ValType::I32],
    );
    m.section(&types);

    // Single function declared (cabi_realloc).
    let mut funcs = FunctionSection::new();
    funcs.function(0);
    m.section(&funcs);

    // Memory.
    let mut mems = MemorySection::new();
    mems.memory(MemoryType {
        minimum: 2,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    m.section(&mems);

    // Shared bump pointer, initialised to the heap start.
    let mut globals = GlobalSection::new();
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(MEM_HEAP_START as i32),
    );
    m.section(&globals);

    // Exports.
    let mut exports = ExportSection::new();
    exports.export("memory", ExportKind::Memory, 0);
    exports.export("bump_ptr", ExportKind::Global, 0);
    exports.export("cabi_realloc", ExportKind::Func, 0);
    m.section(&exports);

    // cabi_realloc body: aligns `bump_ptr` up to the caller-requested
    // alignment (param 2 is `align` in bytes, always a power of two per the
    // canonical ABI), allocates `new_size` (param 3) bytes, advances
    // `bump_ptr`, and returns the aligned pointer.
    //
    //   aligned = (bump_ptr + (align - 1)) & ~(align - 1)
    //   bump_ptr = aligned + new_size
    //   return aligned
    //
    // `old_ptr` and `old_size` are ignored — this is a one-pass bump
    // allocator that never frees or shrinks. Sufficient for the canonical
    // ABI's use of `cabi_realloc(0, 0, align, new_size)` when allocating
    // host-side return buffers in guest memory.
    let mut code = CodeSection::new();
    let mut f = Function::new([(1, ValType::I32)]); // local 4: aligned
                                                    // bump_ptr + align - 1
    f.instruction(&Instruction::GlobalGet(0));
    f.instruction(&Instruction::LocalGet(2));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::I32Const(1));
    f.instruction(&Instruction::I32Sub);
    // & ~(align - 1)   — derived as `& (0 - align)` since align is a power of two.
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::LocalGet(2));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32And);
    // Stash aligned
    f.instruction(&Instruction::LocalTee(4));
    // bump_ptr = aligned + new_size
    f.instruction(&Instruction::LocalGet(3));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::GlobalSet(0));
    // return aligned
    f.instruction(&Instruction::LocalGet(4));
    f.instruction(&Instruction::End);
    code.function(&f);
    // Reference unused-but-imported helpers so the doc-friendly listing stays
    // accurate even when we widen this helper later.
    let _ = (
        BlockType::Empty,
        MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        },
    );
    m.section(&code);

    m
}

/// Generates the textual WIT world description that accompanies the `.wasm`.
///
/// The WIT file is written alongside each `canon build` output so users can
/// inspect the component contract and feed it to tools like `wasm-tools` or
/// `wit-bindgen`.
///
/// `async_set` is consulted to surface per-function async annotations as
/// comments in the emitted WIT. The actual binary component still uses
/// sync lifts/lowers — the comments preview what `wit-component` async
/// lowering will produce once that work lands.
pub(super) fn generate_wit(module: &OModule, async_set: &AsyncSet) -> String {
    let mut out = String::new();
    out.push_str("// Auto-generated by the Canon compiler.\n");
    out.push_str("// The compiled component implements this world.\n\n");
    out.push_str("package canon:app@0.1.0;\n\n");

    // Async inference summary — listed as a comment block so users can
    // verify the bottom-up fixpoint matches their expectations. Once the
    // codegen emits async lifts, these will become real `async func`
    // declarations in the world's exported interface.
    let mut suspending: Vec<String> = module
        .items
        .iter()
        .filter_map(|item| {
            if let Item::Function(func) = item {
                let key = (
                    func.receiver.as_ref().map(|r| r.name.clone()),
                    func.name.name.clone(),
                );
                if async_set.contains(&key) {
                    return Some(format_suspending(func.receiver.as_ref(), &func.name.name));
                }
            }
            None
        })
        .collect();
    suspending.sort();
    suspending.dedup();
    if suspending.is_empty() {
        out.push_str("/// Async inference — no suspending functions detected.\n\n");
    } else {
        out.push_str("/// Async inference — the following functions are suspending\n");
        out.push_str(
            "/// (lowered with `CanonicalOption::Async` and declared as\n\
             /// `async func(…)` in the imported interface type):\n",
        );
        for name in &suspending {
            out.push_str(&format!("///   - {}\n", name));
        }
        out.push('\n');
    }

    out.push_str(
        "/// The world the compiled component implements. It is a WASI Preview 3\n\
         /// command — stdout is reached natively through\n\
         /// `wasi:cli/stdout.write-via-stream`, no `canon:*` bridge is\n\
         /// imported.\n\
         world app {\n\
         \x20   include wasi:cli/command@0.3.0-rc-2026-03-15;\n\
         }\n",
    );
    out
}

/// Formats a `(receiver, name)` pair as a human-readable Canon-level
/// reference. Free functions are `name`; methods are `Receiver.name`.
fn format_suspending(receiver: Option<&crate::ast::Ident>, name: &str) -> String {
    match receiver {
        Some(r) => format!("{}.{}", r.name, name),
        None => name.to_string(),
    }
}
