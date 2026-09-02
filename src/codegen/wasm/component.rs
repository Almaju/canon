//! Component Model wrapping.
//!
//! Takes the self-contained core module codegen emits and makes it a
//! WebAssembly **Component Model** component targeting **WASI Preview
//! 3**, through `wit-component`: the world the program implements is
//! embedded as component-type metadata and `ComponentEncoder` emits
//! every lift and lower. A CLI program's world is synthesized from the
//! interfaces its extern imports name (`program_world_wit`) and exports
//! `wasi:cli/run`; an HTTP program implements the vendored
//! `wasi:http/service` world. The resulting `.wasm` imports only WASI
//! interfaces (and, until they retire, the `canon:builtins` bridges),
//! so it is portable to any compliant WASI P3 runtime.
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

use wasm_encoder::PrimitiveValType;

use super::{collect_extern_imports, ExternImport, IndirectReturnShape, ParamKind};
use crate::ast::Module as OModule;

/// WASI Preview 3 cli/run interface name.
pub(super) const WASI_CLI_RUN: &str = "wasi:cli/run@0.3.0-rc-2026-03-15";

/// WASI Preview 3 http/handler interface name. Emitted as the
/// component-level export for programs whose entry has a
/// `(Request) -> Response` signature. See the HTTP handler docs (docs/src/tour/http.md).
pub(super) const WASI_HTTP_HANDLER: &str = "wasi:http/handler@0.3.0-rc-2026-03-15";

/// The vendored WASI Preview 3 WIT sources needed to resolve the
/// `wasi:http/service` world. `http.wit` pulls in `wasi:clocks` via a
/// `use`; the world imports interfaces from `wasi:cli` and
/// `wasi:random`; and `wasi:cli`'s own worlds reference
/// `wasi:filesystem` and `wasi:sockets` — so the whole vendored set
/// must be in the `Resolve`. Order matters: dependencies before
/// dependents.
const WIT_WASI_CLOCKS: &str = include_str!("../../../packages/canon/wit/wasi/clocks.wit");
const WIT_WASI_FILESYSTEM: &str = include_str!("../../../packages/canon/wit/wasi/filesystem.wit");
const WIT_WASI_SOCKETS: &str = include_str!("../../../packages/canon/wit/wasi/sockets.wit");
const WIT_WASI_CLI: &str = include_str!("../../../packages/canon/wit/wasi/cli.wit");
const WIT_WASI_RANDOM: &str = include_str!("../../../packages/canon/wit/wasi/random.wit");
const WIT_WASI_HTTP: &str = include_str!("../../../packages/canon/wit/wasi/http.wit");

/// The `canon:builtins` host bridges the runtime still fulfils (see the
/// `host_builtin_*` modules in `src/runtime.rs`). Each retires as the
/// WASI interface behind it lowers; the last one takes this with it.
const WIT_CANON_BUILTINS: &str = "
package canon:builtins@0.1.0;

interface filesystem {
    open-file: func(path: string) -> result<string, string>;
    read: func(file: string) -> result<string, string>;
    write: func(contents: string, path: string) -> result<string, string>;
}

interface json {
    from-float: func(value: f64) -> string;
}
";

/// The `wasi:http/client` function codegen fuses into one round trip
/// (`IndirectReturnShape::HttpSend`).
pub(super) const WASI_HTTP_CLIENT_SEND: &str = "wasi:http/client@0.3.0-rc-2026-03-15#send";

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
            ("canon-builtins.wit", WIT_CANON_BUILTINS),
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

/// Does `urn` (`iface@ver#fn`) return `tuple<stream<u8>, future<result<_,
/// E>>>` — the byte-stream shape codegen drains at the boundary — with
/// no async value among its parameters? Chases aliases on the way.
pub fn vendored_extern_returns_byte_stream(urn: &str) -> bool {
    fn shape(urn: &str) -> Option<()> {
        use wit_parser::TypeDefKind as K;
        let (resolve, func) = vendored_func(urn)?;
        if func
            .params
            .iter()
            .any(|p| wit_mentions_async_value(resolve, &p.ty))
        {
            return None;
        }
        let chase = |mut t: wit_parser::Type| {
            for _ in 0..20 {
                let wit_parser::Type::Id(id) = t else {
                    return None;
                };
                match &resolve.types[id].kind {
                    K::Type(inner) => t = *inner,
                    _ => return Some(id),
                }
            }
            None
        };
        let K::Tuple(tuple) = &resolve.types[chase(*func.result.as_ref()?)?].kind else {
            return None;
        };
        let [stream, future] = tuple.types.as_slice() else {
            return None;
        };
        let K::Stream(Some(wit_parser::Type::U8)) = &resolve.types[chase(*stream)?].kind else {
            return None;
        };
        let K::Future(Some(result)) = &resolve.types[chase(*future)?].kind else {
            return None;
        };
        let K::Result(result) = &resolve.types[chase(*result)?].kind else {
            return None;
        };
        (result.ok.is_none() && result.err.is_some()).then_some(())
    }
    shape(urn).is_some()
}

pub(super) type ExternPrimSig = (
    Vec<Option<PrimitiveValType>>,
    Option<Option<PrimitiveValType>>,
);

/// Whether a `wasi:*` extern's vendored WIT signature mentions a
/// `stream` or `future` anywhere.
///
/// Canon's own signature cannot say so: a binding spells
/// `wasi:cli/stdin`'s `func() -> tuple<stream<u8>, future<result<_,
/// error-code>>>` as `Unit => Result<Stdin, IoError>`, because Canon has
/// no surface for either type. The vendored WIT is the only place the
/// real shape is visible, and without this the extern is typed from the
/// Canon signature — the component then imports `read-via-stream` with a
/// `result` return where the host has a `tuple`, and instantiation fails
/// on a program that passed `check` and `build`.
pub fn vendored_extern_uses_async_value(urn: &str) -> bool {
    let Some((resolve, func)) = vendored_func(urn) else {
        return false;
    };
    func.params
        .iter()
        .map(|p| &p.ty)
        .chain(func.result.as_ref())
        .any(|t| wit_mentions_async_value(resolve, t))
}

/// Recursive walk for `vendored_extern_uses_async_value`. Descends
/// through the shapes a binding can legitimately name (aliases, tuples,
/// options, results, lists) looking for a `stream` or `future` in any
/// position.
fn wit_mentions_async_value(resolve: &wit_parser::Resolve, t: &wit_parser::Type) -> bool {
    use wit_parser::TypeDefKind as K;
    let wit_parser::Type::Id(id) = t else {
        return false;
    };
    let nested = |ty: &Option<wit_parser::Type>| {
        ty.as_ref()
            .is_some_and(|inner| wit_mentions_async_value(resolve, inner))
    };
    match &resolve.types[*id].kind {
        K::Stream(_) | K::Future(_) => true,
        K::Type(inner) | K::List(inner) | K::Option(inner) => {
            wit_mentions_async_value(resolve, inner)
        }
        K::Tuple(t) => t.types.iter().any(|i| wit_mentions_async_value(resolve, i)),
        K::Result(r) => nested(&r.ok) || nested(&r.err),
        K::Record(r) => r
            .fields
            .iter()
            .any(|f| wit_mentions_async_value(resolve, &f.ty)),
        K::Variant(v) => v.cases.iter().any(|c| nested(&c.ty)),
        _ => false,
    }
}

pub(super) fn vendored_extern_prim_sig(urn: &str) -> Option<ExternPrimSig> {
    let (resolve, func) = vendored_func(urn)?;
    let params = func
        .params
        .iter()
        .map(|p| wit_prim(resolve, &p.ty))
        .collect();
    let result = func.result.as_ref().map(|t| wit_prim(resolve, t));
    Some((params, result))
}

/// Navigates a `wasi:*` extern URN to its function in the vendored WIT.
fn vendored_func(
    urn: &str,
) -> Option<(&'static wit_parser::Resolve, &'static wit_parser::Function)> {
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
    Some((resolve, func))
}

/// The scalar payload of a `wasi:*` extern's `option<T>` return, from
/// the vendored WIT. `None` when the URN doesn't resolve or the result
/// isn't an option of a scalar primitive.
pub(super) fn vendored_extern_option_payload(urn: &str) -> Option<PrimitiveValType> {
    vendored_extern_payload(urn, |kind| match kind {
        wit_parser::TypeDefKind::Option(t) => Some(*t),
        _ => None,
    })
}

/// The scalar element of a `wasi:*` extern's `list<T>` return, from the
/// vendored WIT. `None` when the URN doesn't resolve or the result
/// isn't a list of a scalar primitive.
pub(super) fn vendored_extern_list_elem(urn: &str) -> Option<PrimitiveValType> {
    vendored_extern_payload(urn, |kind| match kind {
        wit_parser::TypeDefKind::List(t) => Some(*t),
        _ => None,
    })
}

/// Shared walk for the two helpers above: chase `type x = y` aliases
/// from the extern's WIT result to its structural kind, pick the inner
/// type out of it, and resolve that to a primitive.
fn vendored_extern_payload(
    urn: &str,
    pick: impl Fn(&wit_parser::TypeDefKind) -> Option<wit_parser::Type>,
) -> Option<PrimitiveValType> {
    let (resolve, func) = vendored_func(urn)?;
    let mut t = *func.result.as_ref()?;
    for _ in 0..20 {
        let wit_parser::Type::Id(id) = t else {
            return None;
        };
        match &resolve.types[id].kind {
            wit_parser::TypeDefKind::Type(inner) => t = *inner,
            other => return pick(other).and_then(|inner| wit_prim(resolve, &inner)),
        }
    }
    None
}

/// WIT record-of-scalars return info for a `wasi:*` extern: the WIT
/// type name (kebab) plus each field's kebab name and primitive type
/// in declaration order. `None` unless the function returns a named
/// record whose fields are all primitives.
pub(super) fn vendored_extern_record_return(
    urn: &str,
) -> Option<(String, Vec<(String, PrimitiveValType)>)> {
    let (resolve, func) = vendored_func(urn)?;
    let wit_parser::Type::Id(id) = func.result.as_ref()? else {
        return None;
    };
    let td = &resolve.types[*id];
    let wit_parser::TypeDefKind::Record(rec) = &td.kind else {
        return None;
    };
    let name = td.name.clone()?;
    let mut fields = Vec::new();
    for field in &rec.fields {
        fields.push((field.name.clone(), wit_prim(resolve, &field.ty)?));
    }
    Some((name, fields))
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

/// The world a CLI program implements, as WIT text: every interface
/// its extern imports name (always `wasi:cli/stdout`, which `.print`
/// reaches natively), and the `wasi:cli/run` export.
fn program_world_wit(module: &OModule) -> String {
    let mut imports: Vec<String> = collect_extern_imports(module)
        .into_iter()
        .map(|ext| ext.component_namespace)
        .collect();
    imports.push(STDOUT_INTERFACE.to_string());
    // The fused `send` builds its request through `wasi:http/types`.
    if imports
        .iter()
        .any(|i| WASI_HTTP_CLIENT_SEND.starts_with(i.as_str()))
    {
        imports.push(WASI_HTTP_TYPES_MODULE.to_string());
    }
    imports.sort();
    imports.dedup();
    let mut out = String::from("package canon:program;\n\nworld program {\n");
    for iface in imports {
        out.push_str(&format!("    import {iface};\n"));
    }
    out.push_str(&format!("    export {WASI_CLI_RUN};\n}}\n"));
    out
}

/// The `wasi:cli/stdout` interface every CLI program imports.
const STDOUT_INTERFACE: &str = "wasi:cli/stdout@0.3.0-rc-2026-03-15";

/// The WIT keyword for a primitive.
fn prim_wit(prim: PrimitiveValType) -> &'static str {
    match prim {
        PrimitiveValType::Bool => "bool",
        PrimitiveValType::S8 => "s8",
        PrimitiveValType::U8 => "u8",
        PrimitiveValType::S16 => "s16",
        PrimitiveValType::U16 => "u16",
        PrimitiveValType::S32 => "s32",
        PrimitiveValType::U32 => "u32",
        PrimitiveValType::S64 => "s64",
        PrimitiveValType::U64 => "u64",
        PrimitiveValType::F32 => "f32",
        PrimitiveValType::F64 => "f64",
        PrimitiveValType::Char => "char",
        PrimitiveValType::String => "string",
        PrimitiveValType::ErrorContext => "error-context",
    }
}

/// The WIT declaration of one extern — a function, preceded by the
/// record type its result names, if any. Only externs outside the
/// vendored WIT come through here (a project's own bindings), so the
/// signature is what the Canon declaration says: named `arg0 …`, the
/// integer widths Canon's `Int` erases taken as the widest signed.
fn extern_wit(ext: &ExternImport) -> String {
    let params: Vec<String> = ext
        .component_params
        .iter()
        .enumerate()
        .map(|(i, kind)| {
            let ty = match kind {
                ParamKind::Scalar(prim) => prim_wit(*prim),
                ParamKind::String => "string",
            };
            format!("arg{i}: {ty}")
        })
        .collect();
    let mut out = String::new();
    let result = match &ext.indirect_return {
        Some(IndirectReturnShape::String) => Some("string".to_string()),
        Some(IndirectReturnShape::ResultStringString { .. }) => {
            Some("result<string, string>".to_string())
        }
        Some(IndirectReturnShape::OptionString) => Some("option<string>".to_string()),
        Some(IndirectReturnShape::OptionScalar { prim }) => {
            Some(format!("option<{}>", prim_wit(*prim)))
        }
        Some(IndirectReturnShape::ListString) => Some("list<string>".to_string()),
        Some(IndirectReturnShape::ListScalar { prim }) => {
            Some(format!("list<{}>", prim_wit(*prim)))
        }
        Some(IndirectReturnShape::ScalarRecord {
            wit_name, fields, ..
        }) => {
            out.push_str(&format!("    record {wit_name} {{\n"));
            for field in fields {
                out.push_str(&format!(
                    "        {}: {},\n",
                    field.wit_name,
                    prim_wit(field.prim)
                ));
            }
            out.push_str("    }\n");
            Some(wit_name.clone())
        }
        // Only the vendored WIT produces these shapes.
        Some(IndirectReturnShape::ByteStream { .. } | IndirectReturnShape::HttpSend { .. }) => None,
        None if ext.bare_result => Some("result".to_string()),
        None => ext
            .component_result
            .map(|prim| prim_wit(prim).to_string())
            .or_else(|| {
                ext.results.first().map(|vt| {
                    match vt {
                        wasm_encoder::ValType::I32 => "s32",
                        wasm_encoder::ValType::I64 => "s64",
                        wasm_encoder::ValType::F32 => "f32",
                        wasm_encoder::ValType::F64 => "f64",
                        _ => "s32",
                    }
                    .to_string()
                })
            }),
    };
    let keyword = if ext.is_async { "async func" } else { "func" };
    out.push_str(&format!(
        "    {}: {keyword}({})",
        ext.fn_name,
        params.join(", ")
    ));
    if let Some(result) = result {
        out.push_str(&format!(" -> {result}"));
    }
    out.push_str(";\n");
    out
}

/// Is `iface` (`ns:pkg/name@ver`) an interface the resolve already
/// holds?
fn resolve_has_interface(resolve: &wit_parser::Resolve, iface: &str) -> bool {
    let Some((pkg, rest)) = iface.split_once('/') else {
        return false;
    };
    let (name, version) = match rest.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => (rest, None),
    };
    resolve.package_names.iter().any(|(pkg_name, id)| {
        format!("{}:{}", pkg_name.namespace, pkg_name.name) == pkg
            && pkg_name.version.as_ref().map(|v| v.to_string()).as_deref() == version
            && resolve.packages[*id].interfaces.contains_key(name)
    })
}

/// WIT packages for the extern interfaces the vendored set doesn't
/// declare — a project's own bindings — synthesized from the Canon
/// signatures, one package per `ns:pkg@ver`.
fn synthesized_packages(resolve: &wit_parser::Resolve, externs: &[ExternImport]) -> Vec<String> {
    use std::collections::BTreeMap;
    let mut by_package: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for ext in externs {
        if resolve_has_interface(resolve, &ext.component_namespace) {
            continue;
        }
        let Some((pkg, rest)) = ext.component_namespace.split_once('/') else {
            continue;
        };
        let (iface, version) = match rest.split_once('@') {
            Some((iface, version)) => (iface, format!("@{version}")),
            None => (rest, String::new()),
        };
        by_package
            .entry(format!("{pkg}{version}"))
            .or_default()
            .entry(iface.to_string())
            .or_default()
            .push_str(&extern_wit(ext));
    }
    by_package
        .into_iter()
        .map(|(pkg, ifaces)| {
            let mut out = format!("package {pkg};\n");
            for (iface, body) in ifaces {
                out.push_str(&format!("\ninterface {iface} {{\n{body}}}\n"));
            }
            out
        })
        .collect()
}

/// The program's world resolved against the vendored WIT.
fn program_world(module: &OModule) -> (wit_parser::Resolve, wit_parser::WorldId) {
    let mut resolve = vendored_resolve().clone();
    for (i, source) in synthesized_packages(&resolve, &collect_extern_imports(module))
        .iter()
        .enumerate()
    {
        resolve
            .push_source(&format!("extern-{i}.wit"), source)
            .expect("a synthesized extern interface parses");
    }
    let pkg = resolve
        .push_source("program.wit", &program_world_wit(module))
        .expect("the program world names vendored interfaces");
    let world = resolve
        .select_world(&[pkg], Some("program"))
        .expect("the program package declares one world");
    (resolve, world)
}

/// CLI-entry programs route here: the self-contained core module
/// `WasmGen::compile` emits (own memory, own `cabi_realloc`,
/// `wit-component` import naming, the entry exported as
/// `[async-lift-stackful]wasi:cli/run@…#run`) gets the program's world
/// embedded as component-type metadata, and `wit-component` emits every
/// lift and lower — the same path `wrap_http_service` takes.
pub(super) fn wrap_cli(module: &OModule, core: &[u8]) -> Vec<u8> {
    let (resolve, world) = program_world(module);
    let mut core = core.to_vec();
    wit_component::embed_component_metadata(
        &mut core,
        &resolve,
        world,
        wit_component::StringEncoding::UTF8,
    )
    .expect("embed the program world");
    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(&core)
        .expect("core module matches the program world")
        .encode()
        .expect("component encoding succeeds")
}

/// The WIT text of the world a CLI program implements — what `canon
/// build` writes beside the `.wasm`.
pub(super) fn world_wit(module: &OModule) -> String {
    let (resolve, world) = program_world(module);
    let pkg = resolve.worlds[world]
        .package
        .expect("the program world has a package");
    let mut printer = wit_component::WitPrinter::default();
    printer
        .print(&resolve, pkg, &[])
        .expect("print the program world");
    printer.output.to_string()
}
