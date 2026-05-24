/// Oneway WASM codegen — emits a core module which is then wrapped into a
/// **Component Model** component (WASI Preview 3) by `component::wrap`.
///
/// The core module:
///   - Imports its linear memory from `"env" "memory"`
///     (provided by a tiny memory-only core module instantiated by the wrapper).
///   - Imports five canonical-ABI builtins from `"wasi:cli/stdout"` —
///     `write-via-stream`, `stream-new`, `stream-write`,
///     `stream-drop-writable`, and `future-drop-readable`. `print_str`
///     stitches them into the native WASI P3 stdout sequence so the
///     produced `.wasm` is portable to any compliant Component Model
///     runtime (no `oneway:*` host bridge required for output).
///   - Exports `"run" (func (result i32))` — the entry point that the wrapper
///     lifts as `wasi:cli/run.run`. The i32 result is the canonical-ABI
///     discriminant for `result<_, _>`: 0 = Ok, 1 = Err.
///
/// Memory layout (shared with the host via the lowered import):
///   [0  .. 16]  reserved (was fd_write scratch in the WASI P1 era; kept for
///               alignment but unused now)
///   [16 .. 32]  int-to-string buffer (grows ← from 32)
///   [32]        '\n' byte (appended to int prints)
///   [64 ..   ]  string literal data (UTF-8, packed)
///   [65536 .. ] bump heap (grows → for union/product/list values)
use std::collections::HashMap;

use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, EntityType, ExportKind, ExportSection,
    Function, FunctionSection, GlobalType, ImportSection, Instruction, MemArg, MemoryType, Module,
    TypeSection, ValType,
};

use crate::ast::{Block, Expr, FunctionDef, Item, MatchArm, Module as OModule, TypeExpr};

mod component;

// ── Memory constants ──────────────────────────────────────────────────────────
const MEM_INT_BUF_END: u32 = 32; // '\n' lives at this byte
const MEM_STR_START: u32 = 64;
pub(super) const MEM_HEAP_START: u32 = 65536; // bump heap begins at second page

// ── Function index constants ──────────────────────────────────────────────
// The imports section starts with the five `wasi:cli/stdout` canonical
// builtins at indices 0..4, followed by every `extern Wasm` declaration
// from the user program (sorted alphabetically by
// `interface@version#fn-name`), followed by the async-runtime waitable
// intrinsics. Compiled functions start right after that block, so their
// indices depend on how many extern imports the program has.
// `WasmGen` populates the dynamic offsets below in `new()`.
//
// `print_str` stitches these five into the canonical-ABI sequence for
// writing a byte buffer to stdout (see `build_print_str`).
const FN_STDOUT_WRITE_VIA_STREAM: u32 = 0; // (i32) -> i32
const FN_STDOUT_STREAM_NEW: u32 = 1; // () -> i64
const FN_STDOUT_STREAM_WRITE: u32 = 2; // (i32, i32, i32) -> i32
const FN_STDOUT_STREAM_DROP_WRITABLE: u32 = 3; // (i32) -> ()
const FN_STDOUT_FUTURE_DROP_READABLE: u32 = 4; // (i32) -> ()
const FIRST_EXTERN_IMPORT_FN: u32 = 5; // first index of a user `extern Wasm` import

// ── Type index constants (pre-defined) ──────────────────────────────
const TY_PRINT_STR: u32 = 0; // (i32,i32) → ()  — print_str body / waitable.join
const TY_PRINT_INT: u32 = 1; // (i64) → ()
const TY_PRINT_BOOL: u32 = 2; // (i32) → ()  — also stdout-builtin (i32) -> () slot
                              // `run` is lifted as an *async stackful* function at the component level so
                              // nested calls to `extern Wasm.async` can suspend on `waitable-set.wait`
                              // without tripping wasmtime's "cannot block a synchronous task" check.
                              // The async-stackful lift delivers the result via `task.return(…)` rather
                              // than the function's wasm return value, so `run`'s core signature is
                              // `() -> ()`.
const TY_RUN: u32 = 3; // () → ()
const TY_ALLOC: u32 = 4; // (i32) → (i32)
                         // WASI stdout canonical-ABI builtin signatures (declared after the
                         // pre-existing TY_* slots so user types still start at a stable offset).
const TY_STDOUT_WRITE_VIA_STREAM: u32 = 5; // (i32) → (i32)
const TY_STDOUT_STREAM_NEW: u32 = 6; // () → (i64)
const TY_STDOUT_STREAM_WRITE: u32 = 7; // (i32, i32, i32) → (i32)
                                       // `waitable-set.new` needs `() -> i32`. `TY_RUN` used to fit but is now
                                       // `() -> ()` because `run` is lifted as an *async-stackful* function
                                       // (result delivered via `task.return`).
const TY_HANDLE_RETURN: u32 = 8; // () → (i32)
const TY_USER_START: u32 = 9; // first dynamic user type

// ── Extern Wasm path parsing ─────────────────────────────────────────────────────

/// Splits an `extern Wasm` path of the form
/// `"namespace:package/interface@version#fn-name"` into
/// `(component_namespace, core_namespace, fn_name)`.
///
///   - `component_namespace` keeps the `@version` suffix; it is the name
///     wasmtime matches against the linker.
///   - `core_namespace` strips the version; it is the import-module name we
///     use inside the core wasm module (purely an internal contract).
///   - `fn_name` is everything after `#`.
fn parse_extern_path(path: &str) -> Option<(String, String, String)> {
    let (iface, fn_name) = path.split_once('#')?;
    let core_ns = match iface.split_once('@') {
        Some((before_version, _version)) => before_version.to_string(),
        None => iface.to_string(),
    };
    Some((iface.to_string(), core_ns, fn_name.to_string()))
}

/// Walks the module's items, collects every `extern Wasm` function, parses the
/// path, derives the WASM signature, and assigns each a function index. The
/// resulting list is sorted by `(core_namespace, fn_name)` so the output is
/// deterministic across runs (matching Oneway's "alphabetical" ethos).
fn collect_extern_imports(ast: &OModule) -> Vec<ExternImport> {
    let type_defs = build_type_defs_map(ast);
    let mut raw: Vec<ExternImport> = Vec::new();
    for item in ast.items.iter() {
        let Item::Function(func) = item else { continue };
        let Some(ext) = &func.extern_wasm else {
            continue;
        };
        let Some((component_ns, core_ns, fn_name)) = parse_extern_path(&ext.path) else {
            continue;
        };

        // Validate the signature: flat-scalar or string params + (flat-scalar
        // OR string) return. Anything more exotic (lists, records, Result,
        // futures) is silently skipped — those externs won't appear in the
        // method table, so call sites fall through to built-in dispatch.
        let Some(component_params) = build_extern_component_params(func, &component_ns, &type_defs)
        else {
            continue;
        };
        let mut params = func_wasm_params_for(func, &type_defs);
        let mut results = func_wasm_results_for(func, &type_defs);

        // Determine the result shape. We support: nothing, a single flat
        // scalar, a bare `string`, or `result<string-alias, string-alias>`.
        // Anything else is too exotic for the current canonical-ABI
        // lowerings.
        let indirect_return = classify_return(&func.return_ty, &results, &type_defs);
        match (&indirect_return, results.len()) {
            (None, 0) | (None, 1) => {}
            (Some(_), _) => {
                // Apply the indirect-return transformation: clear results and
                // append an `i32` return-area pointer to the params.
                results.clear();
                params.push(ValType::I32);
            }
            _ => continue, // unsupported shape
        }

        // For `extern Wasm.async`, the canonical-ABI "async lower" produces
        // a core signature of:
        //   `(flat_params …, ret_ptr?) -> i32`
        // where `ret_ptr` is appended only when the WIT-level function
        // declares a result, and the trailing `i32` is the subtask status.
        // The validator (`wasmparser/src/validator/component_types.rs`
        // — the `(Abi::Lower, Concurrency::Async)` arm) drives this.
        //
        // Implementation: take the natural flat params we already
        // computed, undo any *sync* indirect-return push (the async lower
        // adds its own ret-ptr), then append one i32 ret-ptr if a result
        // is present, and set results to the single i32 status word.
        if ext.is_async {
            // Recompute flat params from scratch so we don't carry over a
            // sync indirect-return pointer added by the block above.
            let mut flat_params = func_wasm_params_for(func, &type_defs);
            // `func_wasm_results_for` is what would be emitted at the
            // WIT level (via `extern_result_to_component`); if non-empty,
            // the async lower appends a single i32 ret-ptr to params.
            let has_result =
                !func_wasm_results_for(func, &type_defs).is_empty() || indirect_return.is_some();
            if has_result {
                flat_params.push(ValType::I32);
            }
            params = flat_params;
            results = vec![ValType::I32];
        }

        raw.push(ExternImport {
            full_path: ext.path.clone(),
            component_namespace: component_ns,
            core_namespace: core_ns,
            fn_name,
            params,
            results,
            component_params,
            indirect_return,
            is_async: ext.is_async,
            func_idx: 0, // filled in below after sorting
        });
    }
    raw.sort_by(|a, b| {
        a.core_namespace
            .cmp(&b.core_namespace)
            .then(a.fn_name.cmp(&b.fn_name))
    });
    for (i, e) in raw.iter_mut().enumerate() {
        e.func_idx = FIRST_EXTERN_IMPORT_FN + i as u32;
    }
    raw
}

/// True if `func` is a Self-renamed constructor (parsed from
/// `Name = (…) -> Name` and normalised by `resolve_new_syntax`). For these,
/// the receiver is the *type* being constructed rather than a real parameter
/// — it doesn't appear in the WASM signature or at call sites.
fn is_self_ctor(func: &FunctionDef) -> bool {
    func.name.name == "Self" && func.receiver.is_some()
}

/// Computes the WASM parameter types for a function. The receiver counts as
/// a runtime parameter *except* for `Self`-renamed constructors, where it's
/// purely a type-level marker.
fn func_wasm_params_for(func: &FunctionDef, type_defs: &HashMap<String, TypeExpr>) -> Vec<ValType> {
    let mut out = Vec::new();
    if let Some(recv) = &func.receiver {
        if !is_self_ctor(func) {
            out.extend(resolve_name_val_types(&recv.name, type_defs));
        }
    }
    for p in &func.params {
        out.extend(type_expr_val_types(&p.ty, type_defs));
    }
    out
}

fn func_wasm_results_for(
    func: &FunctionDef,
    type_defs: &HashMap<String, TypeExpr>,
) -> Vec<ValType> {
    type_expr_val_types(&func.return_ty, type_defs)
}

/// Classifies an extern's return type into an indirect-return shape, looking
/// at the AST type expression directly (rather than the flattened WASM
/// signature) so we can distinguish e.g. `Result<File, IoError>` from a
/// bare `String`.
///
/// Returns:
///   - `Some(String)` for a `String` return (or any String-aliased type),
///   - `Some(ResultStringString { ok_name, err_name })` for `Result<X, Y>`
///     where both `X` and `Y` resolve through aliases to `String`,
///   - `None` for anything else.
fn classify_return(
    return_ty: &TypeExpr,
    flat_results: &[ValType],
    type_defs: &HashMap<String, TypeExpr>,
) -> Option<IndirectReturnShape> {
    if let TypeExpr::Named { name, generics, .. } = return_ty {
        if name == "Result"
            && generics.len() == 2
            && resolves_to_string(&generics[0], type_defs)
            && resolves_to_string(&generics[1], type_defs)
        {
            return Some(IndirectReturnShape::ResultStringString {
                ok_name: named_type_name(&generics[0]).unwrap_or_else(|| "String".to_string()),
                err_name: named_type_name(&generics[1]).unwrap_or_else(|| "String".to_string()),
            });
        }
    }
    if matches!(flat_results, [ValType::I32, ValType::I32]) {
        return Some(IndirectReturnShape::String);
    }
    None
}

/// True when `ty` is `String` or any alias chain that ultimately resolves
/// to `String` (e.g. `Path = String`, `File = Path`, …).
fn resolves_to_string(ty: &TypeExpr, type_defs: &HashMap<String, TypeExpr>) -> bool {
    let TypeExpr::Named { name, .. } = ty else {
        return false;
    };
    fn chase(name: &str, type_defs: &HashMap<String, TypeExpr>, depth: u32) -> bool {
        if depth > 20 {
            return false;
        }
        if name == "String" {
            return true;
        }
        if let Some(TypeExpr::Named { name: inner, .. }) = type_defs.get(name) {
            return chase(inner, type_defs, depth + 1);
        }
        false
    }
    chase(name, type_defs, 0)
}

fn named_type_name(ty: &TypeExpr) -> Option<String> {
    if let TypeExpr::Named { name, .. } = ty {
        Some(name.clone())
    } else {
        None
    }
}

/// Builds a quick `name -> body` map of all type aliases declared in the
/// module. Used by the helpers below to resolve user-named aliases of scalar
/// types (e.g. `OtherInt = Int`).
fn build_type_defs_map(ast: &OModule) -> HashMap<String, TypeExpr> {
    let mut map = HashMap::new();
    for item in ast.items.iter() {
        if let Item::TypeDef(td) = item {
            map.insert(td.name.name.clone(), td.body.clone());
        }
    }
    map
}

/// Resolves an extern function's parameter types (receiver-first) into a list
/// of `ParamKind`s. Returns `None` if any parameter uses a type we can't yet
/// represent at the component-model boundary (lists, records, Result,
/// futures, …).
fn build_extern_component_params(
    func: &FunctionDef,
    component_namespace: &str,
    type_defs: &HashMap<String, TypeExpr>,
) -> Option<Vec<ParamKind>> {
    let signed = !component_namespace.starts_with("wasi:");
    let mut out = Vec::new();
    // Self-renamed constructors carry their target type as `receiver`, but
    // it isn't a runtime parameter — the call `Name(…)` doesn't push the
    // type, only the explicit args.
    if let Some(recv) = &func.receiver {
        if !is_self_ctor(func) {
            let vts = resolve_name_val_types(&recv.name, type_defs);
            push_param_kind(&mut out, recv.name.as_str(), &vts, signed)?;
        }
    }
    for p in &func.params {
        let vts = type_expr_val_types(&p.ty, type_defs);
        let display = match &p.ty {
            TypeExpr::Named { name, .. } => name.as_str(),
            _ => "",
        };
        push_param_kind(&mut out, display, &vts, signed)?;
    }
    Some(out)
}

fn push_param_kind(
    out: &mut Vec<ParamKind>,
    _type_name: &str,
    vts: &[ValType],
    signed: bool,
) -> Option<()> {
    match vts.len() {
        // Capability marker / unit: no component-level parameter.
        0 => Some(()),
        1 => {
            out.push(ParamKind::Scalar(scalar_val_type_to_primitive(
                vts[0], signed,
            )));
            Some(())
        }
        // Any `(i32, i32)` pair lowered through `resolve_name_val_types` /
        // `type_expr_val_types` came from `String` or a `String`-aliased
        // user type (`Path`, `File`, `Url`, …). Both share the same
        // canonical-ABI representation, so accept them uniformly here.
        2 if vts == [ValType::I32, ValType::I32] => {
            out.push(ParamKind::String);
            Some(())
        }
        _ => None, // unsupported (list, record, generic, …)
    }
}

fn scalar_val_type_to_primitive(vt: ValType, signed: bool) -> wasm_encoder::PrimitiveValType {
    use wasm_encoder::PrimitiveValType;
    match vt {
        ValType::I32 if signed => PrimitiveValType::S32,
        ValType::I32 => PrimitiveValType::U32,
        ValType::I64 if signed => PrimitiveValType::S64,
        ValType::I64 => PrimitiveValType::U64,
        ValType::F32 => PrimitiveValType::F32,
        ValType::F64 => PrimitiveValType::F64,
        _ => PrimitiveValType::U32,
    }
}

/// Coarse mapping from a Oneway type expression to its WASM stack types.
/// Mirrors the `Ty::val_types` logic for the cases that show up in extern
/// declarations (scalars, strings, Unit, products of those).
fn type_expr_val_types(ty: &TypeExpr, type_defs: &HashMap<String, TypeExpr>) -> Vec<ValType> {
    match ty {
        TypeExpr::Named { name, .. } => resolve_name_val_types(name, type_defs),
        TypeExpr::Product { fields, .. } => {
            let mut out = Vec::new();
            for f in fields {
                out.extend(type_expr_val_types(f, type_defs));
            }
            out
        }
        // Unions, functions, etc. are not currently supported in extern
        // signatures — fall back to an i32 placeholder.
        _ => vec![ValType::I32],
    }
}

/// Resolves a named type to its WASM stack types, walking through any
/// user-defined aliases. Bounded depth keeps us safe against cycles.
fn resolve_name_val_types(name: &str, type_defs: &HashMap<String, TypeExpr>) -> Vec<ValType> {
    fn go(name: &str, type_defs: &HashMap<String, TypeExpr>, depth: u32) -> Vec<ValType> {
        if depth > 20 {
            return vec![ValType::I32];
        }
        match name {
            "Int" => vec![ValType::I64],
            "Float" => vec![ValType::F64],
            "Bool" => vec![ValType::I32],
            "Unit" | "Never" => vec![],
            "String" => vec![ValType::I32, ValType::I32],
            // Capabilities are type-level markers — they don't occupy any
            // runtime slot. Mirrors `WasmGen::resolve_repr` which maps them to
            // `Ty::Unit`. This is what makes `(Random) -> Int` extern decls
            // line up with WASI imports that take no arguments.
            // True ambient-effect capabilities: zero-slot markers passed
            // as type-only proof that the caller holds the capability.
            // `HttpServer<S>` and `HttpClient` are *value types* with state,
            // not capability markers — they're flat-scalar/string aliases
            // declared in the stdlib (`std/http-server-wasm.ow`).
            "Stdout" | "Stderr" | "Stdin" | "Network" | "Clock" | "Filesystem" => vec![],
            _ => {
                if let Some(body) = type_defs.get(name) {
                    return match body {
                        TypeExpr::Named { name: alias, .. } => go(alias, type_defs, depth + 1),
                        other => type_expr_val_types(other, type_defs),
                    };
                }
                // Unknown user type — default to an i32 handle.
                vec![ValType::I32]
            }
        }
    }
    go(name, type_defs, 0)
}

// ── Global index constants ──────────────────────────────────────────────────────────
// The bump pointer is now an *imported* mutable global so it can be shared
// between the user core module and the component wrapper's `cabi_realloc`
// helper. Both bump from the same pointer, which keeps Oneway-allocated heap
// data and host-allocated string returns in a single coherent heap.
const GLOBAL_BUMP_PTR: u32 = 0;

// ── WASM representation of an Oneway expression ──────────────────────────────

/// What a compiled expression leaves on the WASM stack.
///
/// The `Named*` variants carry the Oneway type name so method dispatch can
/// find the right user-defined function.
#[derive(Clone, Debug, PartialEq)]
enum Ty {
    I64,              // Int and Int-aliases
    F64,              // Float and Float-aliases
    I32,              // Bool / raw tag
    Str,              // String (anonymous)
    NamedStr(String), // String alias (e.g. Greeting)  — 2 stack values (ptr, len)
    Ptr,              // anonymous heap ptr
    NamedPtr(String), // named heap ptr — union / product / Option / Result / List
    /// `NamedPtr` to a union whose Ok *and* Err arms carry a
    /// `String`-aliased payload. The three names are:
    ///
    ///   - `0`: the union type name (e.g. `"Result"`), used for variant
    ///     dispatch and method lookups on the wrapper itself.
    ///   - `1`: the Ok-payload type name (e.g. `"Url"` for
    ///     `Result<Url, InvalidUrl>`), used when `?` extracts the payload
    ///     so subsequent method calls dispatch against the right typed
    ///     alias.
    ///   - `2`: the Err-payload type name (e.g. `"InvalidUrl"`), used by
    ///     dispatch arms to type the bound variable in an `Err(e) =>` arm.
    ///
    /// In-memory layout matches `NamedPtr` plus a String payload:
    /// `[tag i32, ptr i32, len i32]` at offsets `0, 4, 8`. The two payload
    /// arms share the same `(ptr, len)` slots — the discriminant decides
    /// which type the bytes belong to.
    NamedPtrStr(String, String, String),
    List, // List<T>: 2 stack values (ptr: i32, len: i32)
    Unit, // no stack values
}

impl Ty {
    /// WASM value types occupied on the stack.
    fn val_types(&self) -> Vec<ValType> {
        match self {
            Ty::I64 => vec![ValType::I64],
            Ty::F64 => vec![ValType::F64],
            Ty::I32 | Ty::Ptr => vec![ValType::I32],
            Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => vec![ValType::I32],
            Ty::Str | Ty::NamedStr(_) | Ty::List => vec![ValType::I32, ValType::I32],
            Ty::Unit => vec![],
        }
    }

    /// The Oneway type name, if known (used for method dispatch).
    fn oneway_name(&self) -> Option<&str> {
        match self {
            Ty::NamedStr(n) | Ty::NamedPtr(n) | Ty::NamedPtrStr(n, _, _) => Some(n.as_str()),
            _ => None,
        }
    }

    fn is_str_like(&self) -> bool {
        matches!(self, Ty::Str | Ty::NamedStr(_))
    }
}

// ── Local scope ───────────────────────────────────────────────────────────────

/// Maps Oneway parameter names to their local variable index + repr.
///
/// Extra locals (indices after params, declared via `extra_locals_decl()`):
///   pc+0, pc+1 (i32): rptr, rlen   — for Str match results
///   pc+2       (i32): rbool         — for I32/Ptr match results
///   pc+3       (i32): tmp_i32       — general scratch i32
///   pc+4       (i64): tmp_i64       — general scratch i64
///   pc+5       (i32): alloc_ptr     — result of $alloc
///   pc+6       (i32): tmp_i32_b     — second scratch i32
#[derive(Clone, Default)]
struct LocalScope {
    vars: HashMap<String, (u32, Ty)>,
    param_count: u32, // first extra-local index
}

impl LocalScope {
    fn empty() -> Self {
        LocalScope {
            vars: HashMap::new(),
            param_count: 0,
        }
    }
    fn rptr(&self) -> u32 {
        self.param_count
    }
    fn rlen(&self) -> u32 {
        self.param_count + 1
    }
    fn rbool(&self) -> u32 {
        self.param_count + 2
    }
    fn tmp_i32(&self) -> u32 {
        self.param_count + 3
    }
    fn tmp_i64(&self) -> u32 {
        self.param_count + 4
    }
    fn alloc_ptr(&self) -> u32 {
        self.param_count + 5
    }
    fn tmp_i32_b(&self) -> u32 {
        self.param_count + 6
    }
    /// Adjacent pair of i32s holding the (ptr, len) of a string payload
    /// bound inside a match arm. Adjacency matters: `push_local` for
    /// `Ty::Str` pushes `LocalGet(idx)` followed by `LocalGet(idx + 1)`,
    /// so the two slots must sit at consecutive indices.
    fn arm_payload_ptr(&self) -> u32 {
        self.param_count + 7
    }
}

/// Local declarations appended after the function params.
fn extra_locals_decl() -> Vec<(u32, ValType)> {
    vec![
        (4, ValType::I32), // rptr, rlen, rbool, tmp_i32
        (1, ValType::I64), // tmp_i64
        (2, ValType::I32), // alloc_ptr, tmp_i32_b
        (2, ValType::I32), // arm_payload_ptr, arm_payload_ptr + 1 (len)
    ]
}

// ── Function table ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct FuncInfo {
    func_idx: u32,
    type_idx: u32,
    result_ty: Ty,
    /// `Some(shape)` when this is an `extern Wasm` whose canonical-ABI
    /// lowering uses indirect return. Call sites allocate a return area,
    /// pass its pointer as an extra last arg, and decode the result
    /// according to `shape` after the call.
    indirect_return: Option<IndirectReturnShape>,
    /// `true` for `extern Wasm.async` functions. Call sites use the
    /// component-model async-lower calling convention: the args go flat
    /// on the stack (as in sync), but the function returns an `i32`
    /// status code instead of the result. A ret-area pointer is
    /// appended to the params when the function has a result; the result
    /// is read out of the ret-area after the call. See
    /// `emit_async_call` for the full sequence.
    is_async: bool,
}

// ── String table ──────────────────────────────────────────────────────────────

struct StringTable {
    data: Vec<u8>,
    offsets: HashMap<String, (u32, u32)>, // content → (abs_offset, len)
}

impl StringTable {
    fn new() -> Self {
        StringTable {
            data: Vec::new(),
            offsets: HashMap::new(),
        }
    }
    fn intern(&mut self, s: &str) -> (u32, u32) {
        if let Some(&p) = self.offsets.get(s) {
            return p;
        }
        let offset = MEM_STR_START + self.data.len() as u32;
        let len = s.len() as u32;
        self.data.extend_from_slice(s.as_bytes());
        self.offsets.insert(s.to_string(), (offset, len));
        (offset, len)
    }
    fn get(&self, s: &str) -> Option<(u32, u32)> {
        self.offsets.get(s).copied()
    }
}

// ── Main compiler struct ──────────────────────────────────────────────────────

/// Component-model parameter kind for an `extern Wasm` argument.
#[derive(Clone, Debug)]
pub(super) enum ParamKind {
    /// A flat scalar: maps to a `PrimitiveValType` (s32/u32/s64/u64/f32/f64).
    Scalar(wasm_encoder::PrimitiveValType),
    /// A `string` parameter. Lowers to `(i32 ptr, i32 len)` at the core ABI.
    String,
}

/// Shape of an indirect (memory-based) return value. The canonical ABI uses
/// indirect return whenever the result's flat representation exceeds
/// `MAX_FLAT_RESULTS = 1`. The caller allocates a return area and passes its
/// pointer as a trailing `i32` parameter; the host writes the result there
/// and the caller decodes it after the call.
#[derive(Clone, Debug)]
pub(super) enum IndirectReturnShape {
    /// Bare `string` return. Return area: 8 bytes, `(i32 ptr, i32 len)` at
    /// offsets 0 and 4. After the call we push the pair as `Ty::Str`.
    String,
    /// `result<string-alias, string-alias>` return where both arms are
    /// `String` or any user alias of `String` (e.g. `File`, `IoError`,
    /// `Url`, `HttpError`). Return area: 12 bytes — byte 0 holds the WIT
    /// discriminant (0=ok, 1=err); bytes 4–7 the payload ptr; bytes 8–11
    /// the payload len. After the call the codegen flips the discriminant
    /// Oneway's alphabetical convention (Err=0, Ok=1) and pushes the
    /// area pointer as `Ty::NamedPtrStr(union, ok_name, err_name)`. The
    /// three names preserve Oneway-level types through `?` and dispatch
    /// so subsequent method calls find their externs (e.g. `.read()`
    /// after `Path(…).File()?`) and the Err arm of a `match` can type
    /// the bound payload (e.g. `Err(e) =>` where `e: IoError`).
    ResultStringString { ok_name: String, err_name: String },
}

impl IndirectReturnShape {
    /// Size of the return area in bytes (must be a multiple of 4).
    fn return_area_size(&self) -> u32 {
        match self {
            IndirectReturnShape::String => 8,
            IndirectReturnShape::ResultStringString { .. } => 12,
        }
    }
}

/// A user `extern Wasm` declaration, resolved to the bits we need at codegen
/// and component-wrapping time.
#[derive(Clone, Debug)]
pub(super) struct ExternImport {
    /// The full path string from the source program, kept verbatim for error
    /// messages and debugging.
    pub(super) full_path: String,
    /// Component-level import name, e.g. `"oneway:builtins/math@0.1.0"`.
    /// Multiple functions can share the same `component_namespace` — they end
    /// up as members of the same imported instance.
    pub(super) component_namespace: String,
    /// Core-module import-module name, e.g. `"oneway:builtins/math"` (no
    /// version). Multiple `ExternImport`s sharing this name are all served by
    /// the same synthetic core instance built inside the component wrapper.
    pub(super) core_namespace: String,
    /// Function name within the interface, e.g. `"min"`.
    pub(super) fn_name: String,
    /// Core WASM signature after any indirect-return transformation:
    /// `returns_string` extends `params` with an `i32` return-area pointer
    /// and clears `results`.
    pub(super) params: Vec<ValType>,
    pub(super) results: Vec<ValType>,
    /// Logical component-level parameters, one entry per Oneway argument
    /// (receiver-first if present), with their `ParamKind`. The component
    /// wrapper uses this list to build the imported instance's function type.
    pub(super) component_params: Vec<ParamKind>,
    /// Indirect-return shape, if the result type doesn't fit in a single
    /// flat WASM value. `None` means the function returns a flat scalar (or
    /// nothing). `Some(shape)` means the core signature appends an `i32`
    /// return-area pointer and clears the result list.
    pub(super) indirect_return: Option<IndirectReturnShape>,
    /// `true` for an `extern Wasm.async` declaration. The canonical-ABI
    /// lowering uses `CanonicalOption::Async`, which collapses the core
    /// signature to `(i32, i32) -> i32` regardless of the original flat
    /// shape — see the async-rewrite branch in `collect_extern_imports`.
    /// The component wrapper still uses `component_params` and
    /// `indirect_return` to compute the WIT-level function type and to
    /// attach the right `Memory` / `Realloc` canonical options.
    pub(super) is_async: bool,
    /// Final function index assigned inside the core module's function index
    /// space (after the stdout canonical-builtin imports but before any
    /// compiled function).
    pub(super) func_idx: u32,
}

struct WasmGen<'m> {
    ast: &'m OModule,
    strings: StringTable,

    // Type definitions: name → body TypeExpr (from AST TypeDef items)
    type_defs: HashMap<String, TypeExpr>,

    // For each union type: sorted list of variant names
    union_variants: HashMap<String, Vec<String>>, // union_name → [variant1, variant2, ...]

    // For each variant name: which union it belongs to
    variant_parent: HashMap<String, String>, // variant → union_type_name

    // For each variant name: its discriminant tag (alphabetical order within the union)
    variant_tag: HashMap<String, u32>,

    // User function table: (Option<receiver_type_name>, method_name) → FuncInfo
    func_table: HashMap<(Option<String>, String), FuncInfo>,

    // WASM type deduplication
    user_type_sigs: Vec<(Vec<ValType>, Vec<ValType>)>, // index 0 → TY_USER_START
    user_type_map: HashMap<(Vec<ValType>, Vec<ValType>), u32>, // → absolute type idx

    /// User `extern Wasm` declarations, in the order they appear in the
    /// emitted core module's import section (sorted by `core_namespace`, then
    /// `fn_name`, so the order is deterministic).
    extern_imports: Vec<ExternImport>,

    // Dynamic function indices in the core module's index space. These are
    // computed in `new()` once `extern_imports.len()` is known. After the
    // imports block (host.print + N externs + 5 waitable intrinsics),
    // defined functions follow at index `1 + N + 5`.
    //
    // The waitable intrinsics implement the canonical-ABI async-wait
    // sequence emitted by `emit_async_call` for the not-Returned status
    // path. They're imported as `oneway:async/waitable.<name>` (a
    // compiler-synthesised module-import name); `component::wrap` builds
    // a synthetic core instance from the canon section that exports the
    // matching functions. They're imported unconditionally so the import
    // section is shape-stable regardless of program content.
    //
    // `task.return` is grouped here too — it's needed by `run`'s async
    // stackful lift to deliver the `result<_, _>` value (since async
    // lift bodies don't return values directly).
    fn_waitable_set_new: u32,  // `()         -> i32`
    fn_waitable_join: u32,     // `(i32, i32) -> ()`   (waitable, set)
    fn_waitable_set_wait: u32, // `(i32, i32) -> i32`  (set, payload-area) -> event-code
    fn_waitable_set_drop: u32, // `(i32)      -> ()`   (set)
    fn_subtask_drop: u32,      // `(i32)      -> ()`   (subtask)
    fn_task_return: u32,       // `(i32)      -> ()`   (discriminant for result<_,_>)

    fn_print_str: u32,
    fn_print_int: u32,
    fn_print_bool: u32,
    fn_alloc: u32,
    fn_start: u32, // exported as "run"
    fn_user_start: u32,
}

impl<'m> WasmGen<'m> {
    fn new(ast: &'m OModule) -> Self {
        let extern_imports = collect_extern_imports(ast);
        let n_externs = extern_imports.len() as u32;
        // Function-index layout after `(1 host.print + N externs)`:
        //   waitable intrinsics (5)
        //   defined functions (print_str, print_int, print_bool, alloc,
        //                       start/run, user functions...)
        // After 5 stdout canonical-builtin imports (FN_STDOUT_*) at
        // indices 0..4 and N extern Wasm imports at 5..5+N, the next
        // block is the 6 waitable+task intrinsics, then the defined
        // functions follow.
        let base_waitable = FIRST_EXTERN_IMPORT_FN + n_externs; // = 5 + N
        let base_defined = base_waitable + 6; // skip the 6 waitable+task imports
        WasmGen {
            ast,
            strings: StringTable::new(),
            type_defs: HashMap::new(),
            union_variants: HashMap::new(),
            variant_parent: HashMap::new(),
            variant_tag: HashMap::new(),
            func_table: HashMap::new(),
            user_type_sigs: Vec::new(),
            user_type_map: HashMap::new(),

            extern_imports,
            fn_waitable_set_new: base_waitable,
            fn_waitable_join: base_waitable + 1,
            fn_waitable_set_wait: base_waitable + 2,
            fn_waitable_set_drop: base_waitable + 3,
            fn_subtask_drop: base_waitable + 4,
            fn_task_return: base_waitable + 5,
            fn_print_str: base_defined,
            fn_print_int: base_defined + 1,
            fn_print_bool: base_defined + 2,
            fn_alloc: base_defined + 3,
            fn_start: base_defined + 4,
            fn_user_start: base_defined + 5,
        }
    }

    // ── Pre-passes ────────────────────────────────────────────────────────────

    fn build_type_defs(&mut self) {
        for item in self.ast.items.iter() {
            if let Item::TypeDef(td) = item {
                self.type_defs.insert(td.name.name.clone(), td.body.clone());
            }
        }
    }

    fn build_variant_info(&mut self) {
        for (name, body) in &self.type_defs {
            if let TypeExpr::Union { variants, .. } = body {
                let mut names: Vec<String> = variants
                    .iter()
                    .filter_map(|v| {
                        if let TypeExpr::Named { name, .. } = v {
                            Some(name.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                names.sort();
                for (tag, variant_name) in names.iter().enumerate() {
                    self.variant_parent
                        .insert(variant_name.clone(), name.clone());
                    self.variant_tag.insert(variant_name.clone(), tag as u32);
                }
                self.union_variants.insert(name.clone(), names);
            }
        }

        // Built-in: Bool = False + True
        self.union_variants.insert(
            "Bool".to_string(),
            vec!["False".to_string(), "True".to_string()],
        );
        self.variant_parent
            .insert("False".to_string(), "Bool".to_string());
        self.variant_parent
            .insert("True".to_string(), "Bool".to_string());
        self.variant_tag.insert("False".to_string(), 0);
        self.variant_tag.insert("True".to_string(), 1);

        // Built-in: Option = None + Some (alphabetical)
        self.union_variants.insert(
            "Option".to_string(),
            vec!["None".to_string(), "Some".to_string()],
        );
        self.variant_parent
            .insert("None".to_string(), "Option".to_string());
        self.variant_parent
            .insert("Some".to_string(), "Option".to_string());
        self.variant_tag.insert("None".to_string(), 0);
        self.variant_tag.insert("Some".to_string(), 1);

        // Built-in: Result = Err + Ok (alphabetical)
        self.union_variants.insert(
            "Result".to_string(),
            vec!["Err".to_string(), "Ok".to_string()],
        );
        self.variant_parent
            .insert("Err".to_string(), "Result".to_string());
        self.variant_parent
            .insert("Ok".to_string(), "Result".to_string());
        self.variant_tag.insert("Err".to_string(), 0);
        self.variant_tag.insert("Ok".to_string(), 1);
    }

    fn collect_all_strings(&mut self) {
        self.strings.intern("False");
        self.strings.intern("True");
        for item in self.ast.items.iter() {
            if let Item::Function(f) = item {
                self.collect_strings_block(&f.body);
            }
        }
    }

    fn collect_strings_block(&mut self, block: &Block) {
        for expr in &block.exprs {
            self.collect_strings_expr(expr);
        }
    }
    fn collect_strings_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::StringLit { value, .. } => {
                // Literals are stored *without* a trailing newline. `.print`
                // on a string emits its own `\n` (see `emit_print`), giving
                // host-returned strings and literals identical output.
                self.strings.intern(value);
            }
            Expr::JsonLit { value, .. } => {
                self.strings.intern(value);
            }
            Expr::FieldAccess { receiver, .. } => self.collect_strings_expr(receiver),
            Expr::MethodCall { receiver, args, .. } => {
                self.collect_strings_expr(receiver);
                for a in args {
                    self.collect_strings_expr(a);
                }
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.collect_strings_expr(scrutinee);
                for arm in arms {
                    self.collect_strings_block(&arm.body);
                }
            }
            Expr::Lambda { body, .. } => self.collect_strings_block(body),
            Expr::Constructor { args, .. } => {
                for a in args {
                    self.collect_strings_expr(a);
                }
            }
            Expr::ProductValue { fields, .. } => {
                for f in fields {
                    self.collect_strings_expr(f);
                }
            }
            Expr::Try { inner, .. } => self.collect_strings_expr(inner),
            _ => {}
        }
    }

    fn assign_func_indices(&mut self) {
        // 1. Extern Wasm imports: register them in the func table so method
        //    calls find them. Their core function indices were already
        //    assigned by `collect_extern_imports`.
        let extern_imports = self.extern_imports.clone();
        for ext in &extern_imports {
            // Always register the core wasm type so the import section can
            // look it up later. For async externs this is the
            // `(flat_params, ret_ptr?) -> i32` async-lower shape (per
            // wasmparser's `Abi::LowerAsync`); for sync externs it's the
            // flat-scalar / indirect-return shape computed by
            // `collect_extern_imports`.
            let type_idx = self.get_or_add_wasm_type(&ext.params, &ext.results);
            // Find the AST function so we can pull its receiver type and
            // Oneway return type — needed for proper method dispatch.
            let Some(func) = self.ast.items.iter().find_map(|item| {
                if let Item::Function(f) = item {
                    if let Some(e) = &f.extern_wasm {
                        if e.path == ext.full_path {
                            return Some(f);
                        }
                    }
                }
                None
            }) else {
                continue;
            };
            let result_ty = self.resolve_return_ty(func);
            let key = (
                func.receiver.as_ref().map(|r| r.name.clone()),
                func.name.name.clone(),
            );
            // The Oneway-side result type depends on the indirect-return
            // shape: a bare `String` return is `Ty::Str`, while a
            // `Result<Ok, Err>` (both string-aliased) becomes
            // `Ty::NamedPtrStr("Result", ok_name, err_name)` so `?` and
            // dispatch arms can extract the string payload with the right
            // Oneway-level type on either branch.
            let surface_result_ty = match &ext.indirect_return {
                Some(IndirectReturnShape::String) => Ty::Str,
                Some(IndirectReturnShape::ResultStringString { ok_name, err_name }) => {
                    Ty::NamedPtrStr("Result".to_string(), ok_name.clone(), err_name.clone())
                }
                None => result_ty,
            };
            let info = FuncInfo {
                func_idx: ext.func_idx,
                type_idx,
                result_ty: surface_result_ty,
                indirect_return: ext.indirect_return.clone(),
                is_async: ext.is_async,
            };
            self.func_table.insert(key, info.clone());

            // Self-renamed constructors (parsed from `Name = (P) -> Name` or
            // `Name = (P) -> Result<Name, _>`) are also registered
            // commutatively under `(P, Name)` so the user can write either
            // `Name(p)` (constructor style) or `p.Name()` (method style).
            // The codegen call-site handling for both routes through
            // `emit_func_table_call`, so a single `FuncInfo` suffices.
            if is_self_ctor(func) {
                if let Some(first_param) = func.params.first() {
                    if let TypeExpr::Named {
                        name: param_name, ..
                    } = &first_param.ty
                    {
                        let recv_name = func
                            .receiver
                            .as_ref()
                            .map(|r| r.name.clone())
                            .unwrap_or_default();
                        let commutative_key = (Some(param_name.clone()), recv_name);
                        self.func_table.entry(commutative_key).or_insert(info);
                    }
                }
            }
        }

        // 2. Compiled user functions: each gets the next available index.
        let mut idx = self.fn_user_start;
        for item in self.ast.items.iter() {
            if let Item::Function(func) = item {
                // Skip main (inlined into $start)
                if func.name.name == "main" && func.receiver.is_none() {
                    continue;
                }
                // Skip extern wasm declarations (handled above)
                if func.extern_wasm.is_some() {
                    continue;
                }
                // Skip trait type defs (Function-typed bodies)
                if let TypeExpr::Function { .. } = &func.return_ty { /* but still compile */ }

                let params = self.func_wasm_params(func);
                let results = self.func_wasm_results(func);
                let type_idx = self.get_or_add_wasm_type(&params, &results);
                let result_ty = self.resolve_return_ty(func);

                let key = (
                    func.receiver.as_ref().map(|r| r.name.clone()),
                    func.name.name.clone(),
                );
                self.func_table.insert(
                    key,
                    FuncInfo {
                        func_idx: idx,
                        type_idx,
                        result_ty,
                        indirect_return: None,
                        is_async: false,
                    },
                );
                idx += 1;
            }
        }
    }

    // ── Type system ───────────────────────────────────────────────────────────

    fn get_or_add_wasm_type(&mut self, params: &[ValType], results: &[ValType]) -> u32 {
        let key = (params.to_vec(), results.to_vec());
        if let Some(&idx) = self.user_type_map.get(&key) {
            return idx;
        }
        let idx = TY_USER_START + self.user_type_sigs.len() as u32;
        self.user_type_sigs
            .push((params.to_vec(), results.to_vec()));
        self.user_type_map.insert(key, idx);
        idx
    }

    fn resolve_repr(&self, name: &str) -> Ty {
        self.resolve_repr_depth(name, 0)
    }

    fn resolve_repr_depth(&self, name: &str, depth: u32) -> Ty {
        if depth > 20 {
            return Ty::NamedPtr(name.to_string());
        }
        match name {
            "Int" | "Byte" | "Hex" => Ty::I64,
            "Float" => Ty::F64,
            "Bool" | "True" | "False" | "Off" | "On" => Ty::I32,
            "String" => Ty::Str,
            "Unit" | "Never" => Ty::Unit,
            // See `resolve_name_val_types::go` for the rationale on which
            // names belong here — only true ambient-effect capabilities,
            // not value types like `HttpServer<S>`.
            "Stdout" | "Stderr" | "Stdin" | "Network" | "Clock" | "Filesystem" => Ty::Unit,
            "List" | "Map" | "Set" => Ty::List,
            "Option" | "Result" => Ty::NamedPtr(name.to_string()),
            _ => {
                if let Some(body) = self.type_defs.get(name).cloned() {
                    match &body {
                        TypeExpr::Named {
                            name: inner,
                            generics,
                            ..
                        } if generics.is_empty() => {
                            let inner_repr = self.resolve_repr_depth(inner, depth + 1);
                            // Wrap with the outer name for method dispatch
                            match inner_repr {
                                Ty::I64 | Ty::F64 | Ty::I32 | Ty::Unit => inner_repr,
                                Ty::Str | Ty::NamedStr(_) => Ty::NamedStr(name.to_string()),
                                Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) | Ty::Ptr => {
                                    Ty::NamedPtr(name.to_string())
                                }
                                Ty::List => Ty::NamedPtr("List".to_string()),
                            }
                        }
                        TypeExpr::Product { .. } | TypeExpr::Union { .. } => {
                            Ty::NamedPtr(name.to_string())
                        }
                        TypeExpr::Function { .. } => Ty::Unit, // trait type
                        _ => Ty::NamedPtr(name.to_string()),
                    }
                } else if self.variant_parent.contains_key(name) {
                    // Zero-data union variant (e.g. Leaf in Tree = Branch + Leaf)
                    let parent = self.variant_parent[name].clone();
                    Ty::NamedPtr(parent)
                } else {
                    Ty::Unit
                }
            }
        }
    }

    fn resolve_type_expr_repr(&self, ty: &TypeExpr) -> Ty {
        match ty {
            TypeExpr::Named { name, .. } => self.resolve_repr(name),
            TypeExpr::Union { .. } => Ty::Ptr,
            TypeExpr::Product { .. } => Ty::Ptr,
            _ => Ty::Unit,
        }
    }

    fn resolve_return_ty(&self, func: &FunctionDef) -> Ty {
        self.resolve_type_expr_repr(&func.return_ty)
    }

    /// Byte size of a value when stored as a FIELD inside a product/union struct.
    fn field_byte_size(&self, name: &str) -> u32 {
        match self.resolve_repr(name) {
            Ty::I64 | Ty::F64 => 8,
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => 4,
            Ty::Str | Ty::NamedStr(_) | Ty::List => 8,
            Ty::Unit => 0,
        }
    }

    /// Field layout for a product type (name → payload byte size used for offsets).
    /// Returns (field_repr, byte_offset_within_payload).
    fn product_field_layout(&self, product_name: &str) -> Vec<(String, Ty, u32)> {
        let Some(body) = self.type_defs.get(product_name) else {
            return vec![];
        };
        let TypeExpr::Product { fields, .. } = body else {
            return vec![];
        };
        let mut layout = Vec::new();
        let mut offset = 0u32;
        for f in fields {
            if let TypeExpr::Named { name, .. } = f {
                let repr = self.resolve_repr(name);
                let size = self.field_byte_size(name);
                layout.push((name.clone(), repr, offset));
                offset += size;
            }
        }
        layout
    }

    /// Payload byte size for a union variant (0 for zero-data variants).
    fn variant_payload_size(&self, variant_name: &str) -> u32 {
        if let Some(body) = self.type_defs.get(variant_name) {
            match body {
                TypeExpr::Product { fields, .. } => {
                    let mut total = 0u32;
                    for f in fields {
                        if let TypeExpr::Named { name, .. } = f {
                            total += self.field_byte_size(name);
                        }
                    }
                    total
                }
                TypeExpr::Named { name, .. } => self.field_byte_size(name),
                _ => 0,
            }
        } else {
            0 // zero-data variant (e.g. Leaf with no TypeDef)
        }
    }

    /// Total byte size of a union struct (tag + max payload).
    fn union_total_size(&self, union_name: &str) -> u32 {
        let variants = match self.union_variants.get(union_name) {
            Some(v) => v.clone(),
            None => return 8,
        };
        let max_payload: u32 = variants
            .iter()
            .map(|v| self.variant_payload_size(v))
            .max()
            .unwrap_or(0);
        // At least 8 bytes total so we can store a common payload word.
        (4 + max_payload).max(8)
    }

    /// WASM param types for a user function's receiver + params.
    fn func_wasm_params(&self, func: &FunctionDef) -> Vec<ValType> {
        let mut params = Vec::new();
        if let Some(recv) = &func.receiver {
            // Skip the receiver for Self-renamed constructors — their
            // receiver is a type marker, not a runtime value (see
            // `is_self_ctor`).
            if !is_self_ctor(func) {
                let repr = self.resolve_repr(&recv.name);
                params.extend(repr.val_types());
            }
        }
        for param in &func.params {
            let repr = self.resolve_type_expr_repr(&param.ty);
            params.extend(repr.val_types());
        }
        params
    }

    /// WASM result types for a user function's return type.
    fn func_wasm_results(&self, func: &FunctionDef) -> Vec<ValType> {
        self.resolve_type_expr_repr(&func.return_ty).val_types()
    }

    // ── Built-in function builders ─────────────────────────────────────────────

    /// `print_str(ptr: i32, len: i32) -> ()` — writes the byte buffer
    /// `[ptr .. ptr+len)` to stdout using the **native WASI Preview 3**
    /// canonical-ABI stream sequence. The resulting `.wasm` imports
    /// `wasi:cli/stdout` and nothing else — it is portable to any
    /// compliant Component Model runtime.
    ///
    /// ## Sequence emitted
    ///
    /// ```text
    ///   handles = stream.new<u8>()       ;; () -> i64 (low=reader, high=writer)
    ///   reader  = (i32) handles
    ///   writer  = (i32) (handles >> 32)
    ///   future  = write-via-stream(reader)  ;; (i32) -> i32
    ///   _       = stream.write<u8>(writer, ptr, len)
    ///   stream.drop-writable<u8>(writer)
    ///   future.drop-readable(future)
    /// ```
    ///
    /// - `stream.new<u8>` returns both ends packed in an i64; the reader
    ///   goes to the host, the writer stays with us.
    /// - `write-via-stream` is sync-lowered: it synchronously installs
    ///   the host-side pump and returns a future handle.
    /// - `stream.write` posts our bytes. For buffers smaller than
    ///   wasmtime-wasi's default capacity (~8 KiB) this completes
    ///   synchronously; we ignore the status code.
    /// - `stream.drop-writable` signals end-of-stream so the host
    ///   flushes to the OS file descriptor.
    /// - `future.drop-readable` discards the unused completion handle.
    ///
    /// All five canonical builtins are imported from `wasi:cli/stdout`
    /// (a private module-import name the component wrapper backs with a
    /// synthetic core instance — see `component::wrap`).
    fn build_print_str(&self) -> Function {
        // Locals declared in order:
        //   0..1 — params (ptr, len)
        //   2    — i64: packed handles from stream.new
        //   3..5 — i32 × 3: reader, writer, future
        let mut f = Function::new([(1, ValType::I64), (3, ValType::I32)]);

        // handles = stream.new<u8>()
        f.instruction(&Instruction::Call(FN_STDOUT_STREAM_NEW));
        f.instruction(&Instruction::LocalSet(2));

        // reader = (i32) handles                      (low 32 bits)
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(3));

        // writer = (i32) (handles >> 32)              (high 32 bits)
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Const(32));
        f.instruction(&Instruction::I64ShrU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(4));

        // future = write-via-stream(reader)
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::Call(FN_STDOUT_WRITE_VIA_STREAM));
        f.instruction(&Instruction::LocalSet(5));

        // stream.write<u8>(writer, ptr, len)  — status code dropped.
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::Call(FN_STDOUT_STREAM_WRITE));
        f.instruction(&Instruction::Drop);

        // stream.drop-writable<u8>(writer)
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::Call(FN_STDOUT_STREAM_DROP_WRITABLE));

        // future.drop-readable(future)
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::Call(FN_STDOUT_FUTURE_DROP_READABLE));

        f.instruction(&Instruction::End);
        f
    }

    fn build_print_int(&self) -> Function {
        let mut f = Function::new([
            (1, ValType::I32), // local 1: ptr
            (1, ValType::I32), // local 2: is_neg
            (1, ValType::I32), // local 3: digit
        ]);
        f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32));
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::I64LtS);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Sub);
        f.instruction(&Instruction::LocalSet(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::Else);
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64RemU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64DivU);
        f.instruction(&Instruction::LocalSet(0));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(b'-' as i32));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32 + 1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::Call(self.fn_print_str));
        f.instruction(&Instruction::End);
        f
    }

    fn build_print_bool(&self) -> Function {
        let (fp, fl) = self.strings.get("False").expect("False interned");
        let (tp, tl) = self.strings.get("True").expect("True interned");
        let mut f = Function::new([]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(tp as i32));
        f.instruction(&Instruction::I32Const(tl as i32));
        f.instruction(&Instruction::Call(self.fn_print_str));
        f.instruction(&Instruction::Else);
        f.instruction(&Instruction::I32Const(fp as i32));
        f.instruction(&Instruction::I32Const(fl as i32));
        f.instruction(&Instruction::Call(self.fn_print_str));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f
    }

    /// Emit an in-place byte-copy loop reading from `scope.rbool()`
    /// (src) and writing to `scope.rptr()` (dst) for `scope.rlen()`
    /// (n) bytes. All three locals are modified by the loop (dst++,
    /// src++, n--), so they must be set up by the caller and not
    /// relied on after the call returns.
    ///
    /// This exists as a stand-in for `memory.copy` (bulk-memory
    /// proposal) because the component wrapper in `component::wrap`
    /// currently doesn't propagate the bulk-memory feature through
    /// to the synthesised core instance, so emitting `MemoryCopy`
    /// directly fails component validation. A future PR can swap
    /// this for `memory.copy` once the wrapper signs off on the
    /// feature — the call sites in `concat` won't need to change.
    fn emit_byte_copy_loop(&self, scope: &LocalScope, f: &mut Function) {
        // Wasm structured control: outer block (break target),
        // inner loop (continue target).
        //   block
        //     loop
        //       if n == 0: br 1  (out of block)
        //       store8(dst, load8(src))
        //       dst += 1; src += 1; n -= 1
        //       br 0  (continue loop)
        //     end
        //   end
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        // if (n == 0) break out of the block
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        // store8(dst, load8(src))
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Load8U(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        // dst++
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        // src++
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        // n--
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        // continue loop
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End); // end loop
        f.instruction(&Instruction::End); // end block
    }

    /// $alloc(size: i32) → i32  — simple bump allocator.
    /// `$alloc(size: i32) -> i32` — bump-allocates `size` bytes from the
    /// shared `bump_ptr` global, rounding the returned pointer up to a
    /// 4-byte alignment. The 4-byte alignment is enough for everything the
    /// codegen currently allocates (i32 fields, return areas, union tags).
    /// The host-side `cabi_realloc` uses the same heap and honours the
    /// caller's requested alignment explicitly.
    fn build_alloc(&self) -> Function {
        let mut f = Function::new([(1, ValType::I32)]); // local 1: aligned_ptr
                                                        // aligned_ptr = (bump_ptr + 3) & ~3
        f.instruction(&Instruction::GlobalGet(GLOBAL_BUMP_PTR));
        f.instruction(&Instruction::I32Const(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(-4));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::LocalTee(1));
        // bump_ptr = aligned_ptr + size
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::GlobalSet(GLOBAL_BUMP_PTR));
        // return aligned_ptr
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::End);
        f
    }

    /// Builds the `run` function exported by the core module.
    ///
    /// Inlines the body of `main` (Oneway's entry point), drops any value
    /// it leaves on the stack, and delivers `result::ok` via
    /// `task.return(0)`. The core signature is `() -> ()` because the
    /// component-level `run` is lifted *async stackful*: results are
    /// returned through `task.return` rather than as a wasm return value.
    /// This is also what enables `extern Wasm.async` calls inside `main`
    /// to suspend on `waitable-set.wait` — wasmtime won't let a sync
    /// task block, so `run` itself has to be async-lifted.
    fn build_start(&mut self) -> Function {
        let mut f = Function::new(extra_locals_decl());
        let scope = LocalScope::empty();
        let main_body: Option<Block> = self.ast.items.iter().find_map(|item| {
            if let Item::Function(func) = item {
                if func.name.name == "main" && func.receiver.is_none() {
                    return Some(func.body.clone());
                }
            }
            None
        });
        if let Some(body) = main_body {
            let result = self.compile_block_return(&body, &scope, &mut f);
            self.drop_value(result, &mut f);
        }
        // Deliver `result::ok` (discriminant 0) to the component-level
        // caller via `task.return`. This must precede `End` and is how
        // the async-stackful lift signals task completion.
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::Call(self.fn_task_return));
        f.instruction(&Instruction::End);
        f
    }

    fn build_user_function(&mut self, func: &FunctionDef) -> Function {
        let (params, scope) = self.build_local_scope(func);
        let _ = params; // params are implicit in the function type
        let mut f = Function::new(extra_locals_decl());
        let body = func.body.clone();
        let result = self.compile_block_return(&body, &scope, &mut f);
        // The function's WASM type already declares the result type;
        // the value should already be on the stack.
        let _ = result;
        f.instruction(&Instruction::End);
        f
    }

    /// Build LocalScope for a function's params + receiver.
    fn build_local_scope(&self, func: &FunctionDef) -> (Vec<ValType>, LocalScope) {
        let mut scope = LocalScope::default();
        let mut local_idx: u32 = 0;
        let mut params = Vec::new();

        if let Some(recv) = &func.receiver {
            let repr = self.resolve_repr(&recv.name);
            let vt = repr.val_types();
            scope.vars.insert(recv.name.clone(), (local_idx, repr));
            local_idx += vt.len() as u32;
            params.extend(vt);
        }
        for param in &func.params {
            if let TypeExpr::Named { name, .. } = &param.ty {
                let repr = self.resolve_repr(name);
                let vt = repr.val_types();
                scope.vars.insert(name.clone(), (local_idx, repr));
                local_idx += vt.len() as u32;
                params.extend(vt);
            }
        }
        scope.param_count = local_idx;
        (params, scope)
    }

    // ── Expression compilation ─────────────────────────────────────────────────

    /// Compile a block, leaving the last expression's value on the stack.
    fn compile_block_return(&mut self, block: &Block, scope: &LocalScope, f: &mut Function) -> Ty {
        let n = block.exprs.len();
        for expr in &block.exprs[..n.saturating_sub(1)] {
            let ty = self.compile_expr(expr, scope, f);
            self.drop_value(ty, f);
        }
        if let Some(last) = block.exprs.last() {
            self.compile_expr(last, scope, f)
        } else {
            Ty::Unit
        }
    }

    fn compile_expr(&mut self, expr: &Expr, scope: &LocalScope, f: &mut Function) -> Ty {
        match expr {
            // ── Literals ──────────────────────────────────────────────────────
            Expr::IntLit { value, .. } => {
                f.instruction(&Instruction::I64Const(*value));
                Ty::I64
            }
            Expr::FloatLit { value, .. } => {
                f.instruction(&Instruction::F64Const((*value).into()));
                Ty::F64
            }
            Expr::HexLit { value, .. } => {
                f.instruction(&Instruction::I64Const(*value as i64));
                Ty::I64
            }
            Expr::StringLit { value, .. } => {
                // Literal data is stored without a trailing newline; `.print`
                // appends one universally (see `emit_print`).
                let (ptr, len) = self.strings.intern(value);
                f.instruction(&Instruction::I32Const(ptr as i32));
                f.instruction(&Instruction::I32Const(len as i32));
                Ty::Str
            }

            // ── Identifier: param / capability ───────────────────────────────
            Expr::Ident(id) => {
                if let Some((idx, repr)) = scope.vars.get(&id.name).cloned() {
                    self.push_local(idx, &repr, f);
                    repr
                } else {
                    // Capability or unknown — no runtime value
                    Ty::Unit
                }
            }

            // ── Constructors ──────────────────────────────────────────────────
            Expr::Constructor { name, args, .. } => {
                self.compile_constructor(&name.name, args, scope, f)
            }

            // ── Field access (.field) ──────────────────────────────────────
            //
            // Newtype unwrap (`value.B` where the value's type is `A = B`)
            // is a no-op coercion at the wasm level since the newtype and
            // its underlying type share representation — we just retype
            // the value on the stack. See DESIGN.md § "Newtypes Are
            // 1-Component Products".
            //
            // Real product field selection (`user.Birthday`) isn't yet
            // implemented; the checker accepts the syntax (registered in
            // `product_fields`), codegen catch-up is a follow-up.
            //
            // Method calls — including `.print()` — go through `MethodCall`
            // instead, so we don't special-case any method name here.
            Expr::FieldAccess {
                receiver, field, ..
            } => {
                let recv_ty = self.compile_expr(receiver, scope, f);
                if let Some(unwrapped) = newtype_unwrap_ty(&recv_ty, &field.name) {
                    return unwrapped;
                }
                self.drop_value(recv_ty, f);
                Ty::Unit
            }

            // ── Method calls ──────────────────────────────────────────────────
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => self.compile_method_call(receiver, &method.name, args, scope, f),

            // ── Dispatch ──────────────────────────────────────────────────────
            Expr::Match {
                scrutinee, arms, ..
            } => self.compile_match(scrutinee, arms, scope, f),

            // ── Try operator `?` ───────────────────────────────────────────────
            Expr::Try { inner, .. } => {
                let inner_ty = self.compile_expr(inner, scope, f);
                // Phase 3 simplification: `?` extracts the Ok/Some payload
                // unconditionally (no early Err/None return yet). The
                // payload width is determined by the inner type:
                //   - `Ty::NamedPtrStr(_, _, _)` → `(i32 ptr, i32 len)` at offsets 4 and 8.
                //   - `Ty::NamedPtr("Result"|"Option")` → `i64` at offset 4 (legacy).
                match &inner_ty {
                    Ty::NamedPtrStr(_, ok_name, _) => {
                        let ok_name = ok_name.clone();
                        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::I32Load(MemArg {
                            offset: 4,
                            align: 2,
                            memory_index: 0,
                        }));
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::I32Load(MemArg {
                            offset: 8,
                            align: 2,
                            memory_index: 0,
                        }));
                        // Preserve the Oneway-level type of the Ok payload
                        // so subsequent method calls dispatch correctly
                        // (e.g. `.read()` on `File` after
                        // `Path(…).File()?`). `Ty::Str` for a bare
                        // `Result<String, String>`; `Ty::NamedStr(name)`
                        // for any aliased payload type.
                        if ok_name == "String" {
                            Ty::Str
                        } else {
                            Ty::NamedStr(ok_name)
                        }
                    }
                    Ty::NamedPtr(n) if n == "Result" || n == "Option" => {
                        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::I64Load(MemArg {
                            offset: 4,
                            align: 3,
                            memory_index: 0,
                        }));
                        Ty::I64
                    }
                    other => other.clone(),
                }
            }

            // ── Lambda ────────────────────────────────────────────────────────
            Expr::Lambda { .. } => {
                // Lambda values are handled at call sites (.map etc.)
                // Push a placeholder i32 (0) for now.
                f.instruction(&Instruction::I32Const(0));
                Ty::I32
            }

            // ── Product literal ───────────────────────────────────────────────
            Expr::ProductValue { fields, .. } => {
                // Phase 3: compile each field for side effects; return the
                // last value (used when constructing union payloads).
                for field in &fields[..fields.len().saturating_sub(1)] {
                    let ty = self.compile_expr(field, scope, f);
                    self.drop_value(ty, f);
                }
                if let Some(last) = fields.last() {
                    self.compile_expr(last, scope, f)
                } else {
                    Ty::Unit
                }
            }

            // ── JSON literal ──────────────────────────────────────────────
            Expr::JsonLit { value, .. } => {
                let (ptr, len) = self.strings.intern(value);
                f.instruction(&Instruction::I32Const(ptr as i32));
                f.instruction(&Instruction::I32Const(len as i32));
                Ty::Str
            }

            // ── Await (checker-inserted, Phase 5) ─────────────────────────────
            Expr::Await { inner, .. } => self.compile_expr(inner, scope, f),
        }
    }

    // ── Constructor compilation ────────────────────────────────────────────────

    fn compile_constructor(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        match name {
            // Bool variants
            "True" => {
                f.instruction(&Instruction::I32Const(1));
                Ty::I32
            }
            "False" => {
                f.instruction(&Instruction::I32Const(0));
                Ty::I32
            }
            // Unit
            "Unit" => Ty::Unit,
            // Option built-ins
            "None" => self.build_option_none(f),
            "Some" => {
                let payload_ty = if !args.is_empty() {
                    self.compile_expr(&args[0], scope, f)
                } else {
                    Ty::Unit
                };
                self.build_option_some(payload_ty, scope, f)
            }
            // Result built-ins
            "Ok" => {
                let payload_ty = if !args.is_empty() {
                    self.compile_expr(&args[0], scope, f)
                } else {
                    Ty::Unit
                };
                self.build_result_ok(payload_ty, scope, f)
            }
            "Err" => {
                let payload_ty = if !args.is_empty() {
                    self.compile_expr(&args[0], scope, f)
                } else {
                    Ty::Unit
                };
                self.build_result_err(payload_ty, scope, f)
            }
            // List constructor: List(e1, e2, e3, ...)
            "List" => self.build_list_literal(args, scope, f),
            // User-defined types
            _ => {
                // 1. Union variant constructor (e.g. `Branch(...)`, `Leaf`).
                if let Some(parent) = self.variant_parent.get(name).cloned() {
                    let tag = self.variant_tag[name];
                    let total = self.union_total_size(&parent);
                    return self.build_union_value(&parent, name, tag, total, args, scope, f);
                }

                // 2. Free function with this name (no receiver). Lets the
                //    user write zero-arg constructors like `Now()` or
                //    `RandomInt()` that the stdlib declares as
                //    `Name = () -> Name` via `extern Wasm`.
                if args.is_empty() {
                    if let Some(info) = self.func_table.get(&(None, name.to_string())).cloned() {
                        return self.emit_func_table_call(&info, &[], scope, f);
                    }
                    // `Name = () -> Name` is normalised by the parser into
                    // a `Self`-named method with receiver `Name` (see
                    // `resolve_new_syntax`). Dispatch a bare `Name()` call
                    // through that key.
                    if let Some(info) = self
                        .func_table
                        .get(&(Some(name.to_string()), "Self".to_string()))
                        .cloned()
                    {
                        return self.emit_func_table_call(&info, &[], scope, f);
                    }
                }

                // 3. Single-arg constructor with an extern declared as a
                //    method on the arg's type — lets `Url("http://…")` and
                //    `Path("…")` dispatch to `Url = (String) -> Result<…>`
                //    or any similar `T = (S) -> R` declaration.
                if args.len() == 1 {
                    // We need to know the arg's type *before* compiling it
                    // (so the lookup can succeed without committing to an
                    // emit). Use the existing static-shape inference rather
                    // than fully compiling the arg up front.
                    if let Some(recv_ty_name) = self.infer_static_type_name(&args[0]) {
                        let key = (Some(recv_ty_name.clone()), name.to_string());
                        if let Some(info) = self.func_table.get(&key).cloned() {
                            // Compile the arg (this becomes the receiver) and
                            // dispatch — no further args to push.
                            let _ = self.compile_expr(&args[0], scope, f);
                            return self.emit_func_table_call(&info, &[], scope, f);
                        }
                    }
                }

                // 4. Type-def newtype / product constructor.
                if self.type_defs.contains_key(name) {
                    let body = self.type_defs.get(name).cloned().unwrap();
                    return match &body {
                        TypeExpr::Product { .. } => {
                            // Product type: compile args for side effects, return Unit.
                            // The caller is responsible for handling the result.
                            for a in args {
                                let ty = self.compile_expr(a, scope, f);
                                self.drop_value(ty, f);
                            }
                            Ty::Unit
                        }
                        _ => {
                            // Newtype alias: transparent — compile the arg and re-wrap.
                            let repr = self.resolve_repr(name);
                            if !args.is_empty() {
                                let arg_ty = self.compile_expr(&args[0], scope, f);
                                match &repr {
                                    Ty::NamedStr(_) => {
                                        let _ = arg_ty;
                                        Ty::NamedStr(name.to_string())
                                    }
                                    Ty::NamedPtr(_) => {
                                        let _ = arg_ty;
                                        Ty::NamedPtr(name.to_string())
                                    }
                                    _ => {
                                        let _ = arg_ty;
                                        repr
                                    }
                                }
                            } else {
                                Ty::Unit
                            }
                        }
                    };
                }

                // 5. Unknown: compile args for side effects.
                for a in args {
                    let ty = self.compile_expr(a, scope, f);
                    self.drop_value(ty, f);
                }
                Ty::Unit
            }
        }
    }

    /// Quick static inference of an expression's Oneway-level type *name*,
    /// used to look up methods/constructors before compiling. Returns
    /// `Some("String")` for string literals, `Some("Int")` for ints, etc.;
    /// `None` when the static shape isn't obvious without full type checking.
    fn infer_static_type_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::StringLit { .. } | Expr::JsonLit { .. } => Some("String".to_string()),
            Expr::IntLit { .. } | Expr::HexLit { .. } => Some("Int".to_string()),
            Expr::FloatLit { .. } => Some("Float".to_string()),
            Expr::Constructor { name, .. } => {
                // Use the constructor's name as a hint — sufficient for the
                // common case `Path("…").File()` where `File` is a method on
                // `Path`.
                Some(name.name.clone())
            }
            _ => None,
        }
    }

    fn build_option_none(&self, f: &mut Function) -> Ty {
        // Alloc 12 bytes, tag=0
        f.instruction(&Instruction::I32Const(12));
        f.instruction(&Instruction::Call(self.fn_alloc));
        // dup on stack not easy; use store + reload pattern
        // Actually store tag and return ptr
        // f: [ptr]
        // We need to store tag=0 at [ptr+0] then return ptr
        // But we already consumed ptr to alloc, so we need a local.
        // ... this requires a local. Since we're in a context without a scope,
        // let's just emit the allocation inline and hope the caller has scratch space.
        // Simplification: don't set tag (it defaults to 0 in zeroed memory) and return ptr.
        Ty::NamedPtr("Option".to_string())
    }

    fn build_option_some(&mut self, payload_ty: Ty, scope: &LocalScope, f: &mut Function) -> Ty {
        // payload is on stack; save in tmp
        self.save_to_scratch(payload_ty.clone(), scope, f);
        // alloc 12 bytes
        f.instruction(&Instruction::I32Const(12));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
        // store tag=1
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Store(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        // store payload at offset 4
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        self.load_from_scratch(&payload_ty, scope, f);
        self.store_payload_at_offset(4, &payload_ty, scope, f);
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        Ty::NamedPtr("Option".to_string())
    }

    fn build_result_ok(&mut self, payload_ty: Ty, scope: &LocalScope, f: &mut Function) -> Ty {
        self.save_to_scratch(payload_ty.clone(), scope, f);
        f.instruction(&Instruction::I32Const(12));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(1)); // Ok = tag 1
        f.instruction(&Instruction::I32Store(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        self.load_from_scratch(&payload_ty, scope, f);
        self.store_payload_at_offset(4, &payload_ty, scope, f);
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        Ty::NamedPtr("Result".to_string())
    }

    fn build_result_err(&mut self, payload_ty: Ty, scope: &LocalScope, f: &mut Function) -> Ty {
        self.save_to_scratch(payload_ty.clone(), scope, f);
        f.instruction(&Instruction::I32Const(12));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(0)); // Err = tag 0
        f.instruction(&Instruction::I32Store(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        self.load_from_scratch(&payload_ty, scope, f);
        self.store_payload_at_offset(4, &payload_ty, scope, f);
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        Ty::NamedPtr("Result".to_string())
    }

    /// Build a union value (tag + payload). Returns Ty::NamedPtr(union_name).
    ///
    /// IMPORTANT: all field expressions are compiled BEFORE the union struct is
    /// allocated, so nested constructors (e.g. Branch containing Leaf()) can each
    /// use `scope.alloc_ptr()` without clobbering each other.
    #[allow(clippy::too_many_arguments)]
    fn build_union_value(
        &mut self,
        union_name: &str,
        variant_name: &str,
        tag: u32,
        total_size: u32,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let payload_start = 4u32;

        // ── Step 1: Compile all field values BEFORE allocating the union struct ──
        // This prevents nested constructors from overwriting scope.alloc_ptr().
        // We save up to 2 i32 fields and 1 i64 field to scratch locals.

        let layout = if !args.is_empty() {
            self.product_field_layout(variant_name)
        } else {
            vec![]
        };

        // Encoded field types for the store pass below.
        //
        // `Str0` stashes a single string-shaped payload into `tmp_i32`
        // (ptr) and `tmp_i32_b` (len). It pairs with the dispatch-side
        // extraction in `compile_arm_body`, which reads back (ptr, len)
        // from offsets 4 and 8 of the union struct. Only one string
        // payload is supported per variant, which matches the
        // single-arg shape of newtype variants like `Fail = String`.
        #[derive(Clone, Copy)]
        enum SavedField {
            Ptr0,
            Ptr1,
            I64_0,
            Str0,
            Dropped,
        }
        let mut saved: Vec<SavedField> = Vec::new();

        if !args.is_empty() {
            if !layout.is_empty() && args.len() == 1 {
                if let Expr::ProductValue { fields, .. } = &args[0].clone() {
                    let fields = fields.clone();
                    let mut ptr_count = 0usize;
                    let mut i64_count = 0usize;
                    for (i, _) in layout.iter().enumerate() {
                        if let Some(field_expr) = fields.get(i) {
                            let ty = self.compile_expr(field_expr, scope, f);
                            match &ty {
                                Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) => {
                                    if ptr_count == 0 {
                                        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                                        saved.push(SavedField::Ptr0);
                                    } else {
                                        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
                                        saved.push(SavedField::Ptr1);
                                    }
                                    ptr_count += 1;
                                }
                                Ty::I64 | Ty::F64 => {
                                    f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                                    saved.push(SavedField::I64_0);
                                    i64_count += 1;
                                }
                                _ => {
                                    self.drop_value(ty, f);
                                    saved.push(SavedField::Dropped);
                                }
                            }
                        }
                    }
                    let _ = (ptr_count, i64_count);
                } else {
                    // Single non-product arg
                    let arg = args[0].clone();
                    let ty = self.compile_expr(&arg, scope, f);
                    match &ty {
                        Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) => {
                            f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                            saved.push(SavedField::Ptr0);
                        }
                        Ty::I64 | Ty::F64 => {
                            f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                            saved.push(SavedField::I64_0);
                        }
                        _ => {
                            self.drop_value(ty, f);
                            saved.push(SavedField::Dropped);
                        }
                    }
                }
            } else {
                // Direct single arg (non-layout case)
                let arg = args[0].clone();
                let ty = self.compile_expr(&arg, scope, f);
                match &ty {
                    Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) => {
                        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                        saved.push(SavedField::Ptr0);
                    }
                    Ty::I64 | Ty::F64 => {
                        f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                        saved.push(SavedField::I64_0);
                    }
                    Ty::Str | Ty::NamedStr(_) => {
                        // Stack: [ptr, len]. Pop len first (top), then ptr.
                        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
                        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                        saved.push(SavedField::Str0);
                    }
                    _ => {
                        self.drop_value(ty, f);
                        saved.push(SavedField::Dropped);
                    }
                }
            }
        }

        // ── Step 2: Allocate the union struct ────────────────────────────────────
        f.instruction(&Instruction::I32Const(total_size as i32));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));

        // ── Step 3: Store the tag ─────────────────────────────────────────────────
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(tag as i32));
        f.instruction(&Instruction::I32Store(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));

        // ── Step 4: Store field values from scratch locals ───────────────────────
        if !saved.is_empty() {
            if !layout.is_empty() {
                for (idx, sf) in saved.iter().enumerate() {
                    if let Some((_, field_repr, field_offset)) = layout.get(idx) {
                        let abs_offset = payload_start + field_offset;
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        match sf {
                            SavedField::Ptr0 => {
                                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                                self.store_value_at_offset(abs_offset, field_repr, scope, f);
                            }
                            SavedField::Ptr1 => {
                                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                                self.store_value_at_offset(abs_offset, field_repr, scope, f);
                            }
                            SavedField::I64_0 => {
                                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                                self.store_value_at_offset(abs_offset, field_repr, scope, f);
                            }
                            SavedField::Str0 => {
                                // Forward-declared variant for string-typed
                                // union payloads (`Fail = String` style).
                                // The producer side isn't pushing this yet;
                                // when it does, the store will use
                                // `(tmp_i32, tmp_i32_b)` for `(ptr, len)`.
                                // For now, treat as Dropped to keep the
                                // match exhaustive without claiming we
                                // support it.
                                f.instruction(&Instruction::Drop); // drop the addr
                            }
                            SavedField::Dropped => {
                                f.instruction(&Instruction::Drop); // drop the addr
                            }
                        }
                    }
                }
            } else if let Some(sf) = saved.first() {
                // Single non-layout field
                match sf {
                    SavedField::Ptr0 => {
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                        f.instruction(&Instruction::I32Store(MemArg {
                            offset: payload_start as u64,
                            align: 2,
                            memory_index: 0,
                        }));
                    }
                    SavedField::I64_0 => {
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                        f.instruction(&Instruction::I64Store(MemArg {
                            offset: payload_start as u64,
                            align: 3,
                            memory_index: 0,
                        }));
                    }
                    SavedField::Str0 => {
                        // Store ptr at offset 4 (payload_start) and len at
                        // offset 8 (payload_start + 4). Layout matches
                        // what `compile_arm_body` and `?` expect.
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                        f.instruction(&Instruction::I32Store(MemArg {
                            offset: payload_start as u64,
                            align: 2,
                            memory_index: 0,
                        }));
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                        f.instruction(&Instruction::I32Store(MemArg {
                            offset: (payload_start + 4) as u64,
                            align: 2,
                            memory_index: 0,
                        }));
                    }
                    _ => {}
                }
            }
        }

        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        Ty::NamedPtr(union_name.to_string())
    }

    fn build_list_literal(&mut self, args: &[Expr], scope: &LocalScope, f: &mut Function) -> Ty {
        let n = args.len() as u32;
        let byte_size = n * 8;
        f.instruction(&Instruction::I32Const(byte_size as i32));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));

        for (i, arg) in args.iter().enumerate() {
            let ty = self.compile_expr(arg, scope, f);
            // All list elements treated as i64 for Phase 3
            match ty {
                Ty::I64 => {}
                Ty::I32 => {
                    f.instruction(&Instruction::I64ExtendI32S);
                }
                _ => {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::I64Const(0));
                }
            }
            f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
            f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
            f.instruction(&Instruction::I64Store(MemArg {
                offset: (i as u64) * 8,
                align: 3,
                memory_index: 0,
            }));
        }

        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(n as i32));
        Ty::List
    }

    // ── Method call dispatch ────────────────────────────────────────────────────

    fn compile_method_call(
        &mut self,
        receiver: &Expr,
        method: &str,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let recv_ty = self.compile_expr(receiver, scope, f);

        // Check user func table first: look up by Oneway type name. Scalars
        // (`Int`, `Float`, `Bool`, `String`) don't carry their name on the
        // `Ty` enum, so we map them back to a canonical Oneway type name here
        // — this lets `extern Wasm` declarations with scalar receivers (e.g.
        // `min = (Int * …)`) resolve from a call site like `5.min(…)`.
        //
        // Capability receivers (`Random`, `Stdout`, `Clock`, …) leave nothing
        // on the stack and have type `Ty::Unit`. We recover their type name
        // from the AST identifier so calls like `Random.randomInt` resolve.
        let type_name = recv_ty
            .oneway_name()
            .map(|s| s.to_string())
            .or_else(|| match &recv_ty {
                Ty::I64 => Some("Int".to_string()),
                Ty::F64 => Some("Float".to_string()),
                Ty::I32 => Some("Bool".to_string()),
                Ty::Str => Some("String".to_string()),
                Ty::Unit => match receiver {
                    Expr::Ident(id) => Some(id.name.clone()),
                    _ => None,
                },
                _ => None,
            });
        let key = (type_name.clone(), method.to_string());
        if let Some(info) = self.func_table.get(&key).cloned() {
            return self.emit_func_table_call(&info, args, scope, f);
        }

        // Also try without type name (free functions used as methods)
        let free_key = (None, method.to_string());
        if type_name.is_some() {
            if let Some(info) = self.func_table.get(&free_key).cloned() {
                return self.emit_func_table_call(&info, args, scope, f);
            }
        }

        // `.print()` is a universal method that delegates to the type-aware
        // `emit_print` helper. It accepts 0 args (`x.print()`) — a single
        // arg form for the legacy `Stdout` convention can be added later.
        if method == "print" && args.is_empty() {
            self.emit_print(recv_ty, scope, f);
            return Ty::Unit;
        }

        // Built-in methods
        self.compile_builtin_method(recv_ty, method, args, scope, f)
    }

    /// Emits a call to a function registered in `func_table`. Handles the
    /// indirect-return convention for `extern Wasm` functions whose result
    /// doesn't fit in a flat WASM value (`string`, `result<string, string>`).
    ///
    /// For a direct-return function the WASM stack on entry already has the
    /// receiver, so we just compile the remaining args and emit `Call(idx)`.
    /// For an indirect-return function we additionally allocate a return
    /// area, push its pointer as the trailing core arg, and after the call
    /// decode the result according to `info.indirect_return`.
    fn emit_func_table_call(
        &mut self,
        info: &FuncInfo,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        for a in args {
            let _ = self.compile_expr(a, scope, f);
        }
        if info.is_async {
            return self.emit_async_call(info, scope, f);
        }
        let Some(shape) = info.indirect_return.clone() else {
            f.instruction(&Instruction::Call(info.func_idx));
            return info.result_ty.clone();
        };

        // Allocate the return area, stash its pointer, and call.
        f.instruction(&Instruction::I32Const(shape.return_area_size() as i32));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalTee(scope.alloc_ptr()));
        f.instruction(&Instruction::Call(info.func_idx));

        // Decode the result.
        match shape {
            IndirectReturnShape::String => {
                // (i32 ptr at +0, i32 len at +4) — push both as `Ty::Str`.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                Ty::Str
            }
            IndirectReturnShape::ResultStringString { ok_name, err_name } => {
                // Flip the WIT discriminant (byte 0) into Oneway's tag
                // convention by XOR-ing with 1, and store back as a full
                // i32 so bytes 1–3 (which were undefined padding from the
                // host) become zero.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load8U(MemArg {
                    offset: 0,
                    align: 0,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Xor);
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                // Push area pointer as the Result handle.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                Ty::NamedPtrStr("Result".to_string(), ok_name, err_name)
            }
        }
    }

    /// Emits the guest-side sequence for calling an `extern Wasm.async`
    /// function under the component-model async-lower ABI.
    ///
    /// At entry: args are already on the stack in their flat representation
    /// (just like a sync call), having been compiled by
    /// `emit_func_table_call` before the dispatch on `is_async`.
    ///
    /// Sequence:
    ///
    /// 1. **Ret-area** (only when the WIT-level function has a result).
    ///    Allocate `ret_area_size_for(&info.result_ty)` bytes via `$alloc`,
    ///    stash the pointer in `alloc_ptr`, and push it as the trailing
    ///    core-arg.
    /// 2. **Call** the async-lowered import. Its core signature is
    ///    `(flat_params …, ret_ptr?) -> i32` where the i32 result is a
    ///    *packed status word*:
    ///    - low 4 bits = `CallState` (0 Starting, 1 Started,
    ///      2 Returned, 3 StartCancelled, 4 ReturnCancelled)
    ///    - high 28 bits = subtask waitable handle (or 0 when Returned)
    /// 3. **Status check**. Save the status to `tmp_i32`, then mask the
    ///    low 4 bits and compare against `2 = Returned`. On the
    ///    sync-completion fast path we skip the wait block. Otherwise we
    ///    enter the **wait sequence**: extract the subtask handle from
    ///    the high 28 bits of the status, create a fresh waitable-set,
    ///    join the subtask into it, block on `waitable-set.wait`, and
    ///    drop both the set and the subtask after the wait returns. By
    ///    that point the host has written the actual result into our
    ///    ret-area.
    /// 4. **Decode result** from the ret-area according to
    ///    `info.result_ty`.
    fn emit_async_call(&mut self, info: &FuncInfo, scope: &LocalScope, f: &mut Function) -> Ty {
        let has_result = !matches!(info.result_ty, Ty::Unit);
        if has_result {
            // Allocate ret-area, save its ptr, and push it as the last arg.
            let size = ret_area_size_for(&info.result_ty);
            f.instruction(&Instruction::I32Const(size as i32));
            f.instruction(&Instruction::Call(self.fn_alloc));
            f.instruction(&Instruction::LocalTee(scope.alloc_ptr()));
        }
        // Call the async-lowered import. Stack on return: i32 packed status.
        f.instruction(&Instruction::Call(info.func_idx));
        // Save the packed status so we can (a) check the low 4 bits and
        // (b) recover the subtask handle from the high 28 bits if we
        // need to wait.
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        // Check `status & 0xF != 2` (i.e. *not* `Returned`).
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(0xF));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::I32Const(2));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::If(BlockType::Empty));
        // ── Async-suspend path ─────────────────────────────────────────
        // The subtask has been started but not yet finished. Extract its
        // handle (high 28 bits of the packed status), wrap it in a
        // single-element waitable-set, and block on `waitable-set.wait`.
        // The host signals subtask completion through the waitable; when
        // wait returns, the result has been written to our ret-area.
        //
        // We re-use scratch locals from the surrounding function's
        // extra-locals pool:
        //   tmp_i32_b → subtask handle
        //   rbool     → waitable-set handle
        //   rptr      → event-area pointer (8 bytes, written by wait)
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(4));
        f.instruction(&Instruction::I32ShrU);
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        // set = waitable-set.new()
        f.instruction(&Instruction::Call(self.fn_waitable_set_new));
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        // waitable.join(subtask, set)
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));
        // event_area = $alloc(8) — wait writes the 8-byte event payload here.
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        // event_code = waitable-set.wait(set, event_area); we don't need
        // to inspect the event payload since the only thing in the set
        // is our subtask — wait returning means it reached a terminal
        // state. Drop the returned event code.
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_wait));
        f.instruction(&Instruction::Drop);
        // Drop the subtask BEFORE the waitable-set: the subtask is
        // joined to the set as a child, so dropping the set while the
        // subtask is still registered trips wasmtime's
        // `ResourceTableError::HasChildren` check (see
        // `wasmtime::runtime::component::concurrent::waitable_set_drop`).
        // Dropping the subtask removes it from the set's child list;
        // the set then drops cleanly.
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_drop));
        f.instruction(&Instruction::End);
        // Read the result out of the ret-area (still in `alloc_ptr`).
        if !has_result {
            return Ty::Unit;
        }
        match &info.result_ty {
            Ty::Str | Ty::NamedStr(_) => {
                // String result: (ptr i32 at +0, len i32 at +4).
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                info.result_ty.clone()
            }
            Ty::I64 | Ty::F64 => {
                // 8-byte scalar at +0.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                if matches!(info.result_ty, Ty::I64) {
                    f.instruction(&Instruction::I64Load(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                } else {
                    f.instruction(&Instruction::F64Load(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                }
                info.result_ty.clone()
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) => {
                // 4-byte scalar / handle at +0.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                info.result_ty.clone()
            }
            // List / NamedPtrStr / Unit fall here. The current codegen
            // doesn't synthesise async externs returning these shapes —
            // they'd need their own ret-area decoders. Trap so the gap is
            // visible if we ever do.
            _ => {
                f.instruction(&Instruction::Unreachable);
                info.result_ty.clone()
            }
        }
    }

    fn compile_builtin_method(
        &mut self,
        recv_ty: Ty,
        method: &str,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        match (method, &recv_ty) {
            // ── Int arithmetic ────────────────────────────────────────────────
            ("add", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64Add);
                Ty::I64
            }
            ("sub", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64Sub);
                Ty::I64
            }
            ("mul", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64Mul);
                Ty::I64
            }
            ("div", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64DivS);
                Ty::I64
            }
            ("mod", Ty::I64) | ("rem", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64RemS);
                Ty::I64
            }
            ("lt", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64LtS);
                Ty::I32
            }
            ("le", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64LeS);
                Ty::I32
            }
            ("gt", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64GtS);
                Ty::I32
            }
            ("ge", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64GeS);
                Ty::I32
            }
            ("eq", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64Eq);
                Ty::I32
            }
            ("ne", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64Ne);
                Ty::I32
            }
            // ── Float arithmetic ──────────────────────────────────────────────
            ("add", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Add);
                Ty::F64
            }
            ("sub", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Sub);
                Ty::F64
            }
            ("mul", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Mul);
                Ty::F64
            }
            ("div", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Div);
                Ty::F64
            }
            // ── String concat ────────────────────────────────────────────────────
            //
            // Allocates a fresh buffer of size `len1 + len2`, copies the
            // receiver bytes followed by the argument bytes, and returns
            // a new `(ptr, len)` pair. Uses `memory.copy` (bulk-memory
            // proposal) which wasm-encoder + wasmtime both accept.
            ("concat", _) if recv_ty.is_str_like() => {
                // Receiver is on the stack as (ptr1, len1). Compile the
                // argument so we end with (ptr1, len1, ptr2, len2).
                let mut arg_pushed = false;
                if let Some(a) = args.first() {
                    let arg_ty = self.compile_expr(a, scope, f);
                    if arg_ty.is_str_like() {
                        arg_pushed = true;
                    } else {
                        self.drop_value(arg_ty, f);
                    }
                }
                if !arg_pushed {
                    // No string arg — treat as concat with empty string.
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::I32Const(0));
                }

                // Stash inputs into locals (top of stack first):
                //   arm_payload_ptr+1 = len2
                //   arm_payload_ptr   = ptr2
                //   tmp_i32_b         = len1 (kept immutable; used both as
                //                              n for copy 1 and as offset
                //                              into result for copy 2)
                //   rbool             = ptr1 (used as src in copy 1; the
                //                              copy loop modifies it)
                f.instruction(&Instruction::LocalSet(scope.arm_payload_ptr() + 1));
                f.instruction(&Instruction::LocalSet(scope.arm_payload_ptr()));
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalSet(scope.rbool()));

                // total_len = len1 + len2, kept in tmp_i32 for the return.
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalGet(scope.arm_payload_ptr() + 1));
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));

                // result_ptr = alloc(total_len), stash in alloc_ptr.
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));

                // Copy 1: dst = result_ptr, src = ptr1, n = len1.
                // Loop locals: dst → rptr, src → rbool (in-place), n → rlen.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalSet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalSet(scope.rlen()));
                self.emit_byte_copy_loop(scope, f);

                // Copy 2: dst = result_ptr + len1, src = ptr2, n = len2.
                // Reuse rptr/rbool/rlen as before.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::LocalSet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.arm_payload_ptr()));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.arm_payload_ptr() + 1));
                f.instruction(&Instruction::LocalSet(scope.rlen()));
                self.emit_byte_copy_loop(scope, f);

                // Push (result_ptr, total_len) as the concat's return value.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                Ty::Str
            }
            // ── List methods ───────────────────────────────────────────────────
            ("length" | "len", Ty::List) | ("length" | "len", Ty::NamedPtr(_)) => {
                // Stack: (ptr: i32, len: i32) for List, or just i32 for NamedPtr
                match &recv_ty {
                    Ty::List => {
                        // Stack: [ptr, len]. Drop ptr, extend len to i64.
                        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                        f.instruction(&Instruction::Drop); // drop ptr
                        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                        f.instruction(&Instruction::I64ExtendI32S);
                    }
                    _ => {
                        // Not a list — drop and return 0
                        self.drop_value(recv_ty, f);
                        f.instruction(&Instruction::I64Const(0));
                    }
                }
                Ty::I64
            }
            ("map", Ty::List) => {
                // For Phase 3: identity (preserve ptr, len). Drop the lambda arg.
                // Stack: [ptr, len]
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // save len
                f.instruction(&Instruction::LocalSet(scope.alloc_ptr())); // save ptr
                for a in args {
                    let ty = self.compile_expr(a, scope, f);
                    self.drop_value(ty, f);
                }
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                Ty::List
            }
            ("first", Ty::List) => {
                // Stack: [ptr, len] → Option<Int>
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // save len
                f.instruction(&Instruction::LocalSet(scope.alloc_ptr())); // save ptr
                                                                          // alloc 12 bytes for Option
                f.instruction(&Instruction::I32Const(12));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.rbool())); // save option ptr
                                                                      // if len == 0 → None (tag=0, already zeroed)
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::If(BlockType::Empty));
                // Some: tag=1, payload = first i64 element at [list_ptr+0]
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I64Load(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::I64Store(MemArg {
                    offset: 4,
                    align: 3,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::Else);
                // None: tag=0 (already zeroed by alloc initialization? No, heap may be dirty.)
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::End);
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                Ty::NamedPtr("Option".to_string())
            }
            // ── Fallback: drop receiver + args, return Unit ────────────────────
            _ => {
                self.drop_value(recv_ty, f);
                for a in args {
                    let ty = self.compile_expr(a, scope, f);
                    self.drop_value(ty, f);
                }
                Ty::Unit
            }
        }
    }

    fn compile_i64_arg(&mut self, args: &[Expr], scope: &LocalScope, f: &mut Function) {
        if let Some(a) = args.first() {
            let ty = self.compile_expr(a, scope, f);
            if ty == Ty::I32 {
                f.instruction(&Instruction::I64ExtendI32S);
            }
        } else {
            f.instruction(&Instruction::I64Const(0));
        }
    }

    fn compile_f64_arg(&mut self, args: &[Expr], scope: &LocalScope, f: &mut Function) {
        if let Some(a) = args.first() {
            let ty = self.compile_expr(a, scope, f);
            if ty == Ty::I64 {
                f.instruction(&Instruction::F64ConvertI64S);
            }
        } else {
            f.instruction(&Instruction::F64Const(0.0.into()));
        }
    }

    // ── Match / dispatch ────────────────────────────────────────────────────────

    fn compile_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let scrut_ty = self.compile_expr(scrutinee, scope, f);

        // Determine the return type from arm annotations
        let arm_result_ty: Ty = arms
            .first()
            .map(|a| self.resolve_type_expr_repr(&a.return_ty))
            .unwrap_or(Ty::Unit);

        // Bool dispatch (i32 on stack, 0=False, 1=True)
        if scrut_ty == Ty::I32 {
            let true_arm = arms.iter().find(|a| arm_tag(a) == Some(1));
            let false_arm = arms.iter().find(|a| arm_tag(a) == Some(0));
            if true_arm.is_some() || false_arm.is_some() {
                return self.emit_bool_dispatch(true_arm, false_arm, &arm_result_ty, scope, f);
            }
        }

        // Union dispatch (i32 heap ptr on stack).
        // `NamedPtr` and `NamedPtrStr` share an in-memory layout, so both
        // dispatch the same way — the only difference is that
        // `NamedPtrStr` carries enough type info for arms to extract the
        // string payload (handled in `compile_arm_body`).
        let union_name = match &scrut_ty {
            Ty::NamedPtr(n) => Some(n.clone()),
            Ty::NamedPtrStr(n, _, _) => Some(n.clone()),
            _ => None,
        };
        if let Some(union_name) = union_name {
            // Save the union pointer so arm bodies can re-load it to extract
            // a payload, then load and push the tag for the dispatch logic.
            // Per-arm payload extraction happens inside `compile_arm_body`
            // based on each arm's pattern type — there's no single
            // "payload shape" for the whole dispatch, because variants
            // can carry different payload types (e.g. `Fail = String`
            // alongside `Pass = Unit` in `TestResult = Fail + Pass`).
            f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
            f.instruction(&Instruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            return self.emit_union_dispatch(&union_name, arms, &arm_result_ty, scope, f);
        }

        // Fallback: drop scrutinee
        self.drop_value(scrut_ty, f);
        Ty::Unit
    }

    fn emit_bool_dispatch(
        &mut self,
        true_arm: Option<&MatchArm>,
        false_arm: Option<&MatchArm>,
        result_ty: &Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        // tag is on stack (i32): 0=False, 1=True
        f.instruction(&Instruction::If(BlockType::Empty));
        // if-branch: True (tag == 1)
        if let Some(arm) = true_arm {
            self.compile_arm_body(arm, result_ty, scope, f);
        }
        f.instruction(&Instruction::Else);
        // else-branch: False (tag == 0)
        if let Some(arm) = false_arm {
            self.compile_arm_body(arm, result_ty, scope, f);
        }
        f.instruction(&Instruction::End);
        self.load_result(result_ty, scope, f)
    }

    fn emit_union_dispatch(
        &mut self,
        union_name: &str,
        arms: &[MatchArm],
        result_ty: &Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        // tag i32 is on stack, alloc_ptr holds the union address
        // Use if/else for 2-variant unions, br_table for more
        let variants = self
            .union_variants
            .get(union_name)
            .cloned()
            .unwrap_or_default();

        if variants.len() <= 2 {
            // Simple if/else: if tag != 0 → variant[1], else → variant[0]
            let arm_1 = if variants.len() > 1 {
                arms.iter().find(|a| {
                    arm_type_name(a).is_some_and(|n| {
                        n == variants[1] || n == "Some" || n == "Ok" || n == "True"
                    })
                })
            } else {
                None
            };
            let arm_0 = arms.iter().find(|a| {
                arm_type_name(a).is_some_and(|n| {
                    n == variants.first().map(|s| s.as_str()).unwrap_or("")
                        || n == "None"
                        || n == "Err"
                        || n == "False"
                })
            });

            f.instruction(&Instruction::If(BlockType::Empty));
            if let Some(arm) = arm_1 {
                self.compile_arm_body(arm, result_ty, scope, f);
            }
            f.instruction(&Instruction::Else);
            if let Some(arm) = arm_0 {
                self.compile_arm_body(arm, result_ty, scope, f);
            }
            f.instruction(&Instruction::End);
        } else {
            // N-variant dispatch (N ≥ 3). The tag is on the stack; stash
            // it in `tmp_i32` so we can compare against each variant in
            // turn. We emit a chain of `local.get tag; i32.const i;
            // i32.eq; if ... else { ... }` nested to depth N-1, with the
            // final `else` arm handling the last variant. This is the
            // straightforward shape — a `br_table` would be more compact
            // but harder to thread through wasm-encoder's structured
            // control instructions, and the if/else version matches the
            // 2-variant code above so any future control-flow change
            // touches one place.
            f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
            let last_idx = variants.len() - 1;
            // Open `if` blocks for variants 0..last (inclusive lower bound).
            for (tag, variant) in variants.iter().enumerate().take(last_idx) {
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(tag as i32));
                f.instruction(&Instruction::I32Eq);
                f.instruction(&Instruction::If(BlockType::Empty));
                if let Some(arm) = arms.iter().find(|a| arm_matches_variant(a, variant)) {
                    self.compile_arm_body(arm, result_ty, scope, f);
                }
                f.instruction(&Instruction::Else);
            }
            // The else-most branch handles the last variant.
            if let Some(last_variant) = variants.last() {
                if let Some(arm) = arms.iter().find(|a| arm_matches_variant(a, last_variant)) {
                    self.compile_arm_body(arm, result_ty, scope, f);
                }
            }
            // Close all the `if/else` blocks opened above.
            for _ in 0..last_idx {
                f.instruction(&Instruction::End);
            }
        }
        self.load_result(result_ty, scope, f)
    }

    /// Compile a match arm body and SAVE the result to scope scratch locals.
    ///
    /// Before compiling the body, the arm's payload (if any) is extracted
    /// from the union struct (at offsets 4+ via `scope.alloc_ptr()`) and
    /// bound to a local under the arm's pattern name. So for
    ///
    /// ```text
    /// testResult.(
    ///     * (Fail) -> Unit { Fail.String.print() }
    ///     * (Pass) -> Unit { "ok".print() }
    /// )
    /// ```
    ///
    /// the `Fail` arm enters with the string payload already loaded into
    /// `scope.arm_payload_ptr()` / `+1`, and `scope.vars["Fail"]` mapped
    /// to that pair (typed `Ty::NamedStr("Fail")`). The arm body's
    /// `Fail.String.print()` then compiles like any other string
    /// expression — the newtype unwrap is a static-type retype
    /// (`newtype_unwrap_ty`), and `.print()` is the built-in.
    fn compile_arm_body(
        &mut self,
        arm: &MatchArm,
        result_ty: &Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) {
        let arm_scope = self.bind_arm_payload(&arm.param_ty, scope, f);
        let body = arm.body.clone();
        let ty = self.compile_block_return(&body, &arm_scope, f);
        // Save result to scratch locals so we can reload after if/else
        match result_ty {
            Ty::Str | Ty::NamedStr(_) => {
                // ty should push (ptr, len)
                match ty {
                    Ty::Str | Ty::NamedStr(_) => {
                        f.instruction(&Instruction::LocalSet(scope.rlen()));
                        f.instruction(&Instruction::LocalSet(scope.rptr()));
                    }
                    _ => {
                        self.drop_value(ty, f);
                        f.instruction(&Instruction::I32Const(0));
                        f.instruction(&Instruction::LocalSet(scope.rptr()));
                        f.instruction(&Instruction::I32Const(0));
                        f.instruction(&Instruction::LocalSet(scope.rlen()));
                    }
                }
            }
            Ty::I64 => match ty {
                Ty::I64 => {
                    f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                }
                _ => {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::I64Const(0));
                    f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                }
            },
            Ty::I32 => match ty {
                Ty::I32 => {
                    f.instruction(&Instruction::LocalSet(scope.rbool()));
                }
                _ => {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::LocalSet(scope.rbool()));
                }
            },
            Ty::NamedPtr(_) | Ty::Ptr => match ty {
                Ty::NamedPtr(_) | Ty::Ptr => {
                    f.instruction(&Instruction::LocalSet(scope.rbool()));
                }
                _ => {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::LocalSet(scope.rbool()));
                }
            },
            _ => {
                self.drop_value(ty, f);
            }
        }
    }

    /// Extract a dispatch-arm payload from the union struct and return
    /// an extended scope that binds the arm's pattern name to the
    /// extracted value(s).
    ///
    /// The union struct lives at `scope.alloc_ptr()` (set by
    /// `compile_match` before the if/else). The layout matches what
    /// `build_union_value` writes:
    ///
    ///   * offset 0   — discriminant tag (i32)
    ///   * offset 4+  — payload, encoded by variant
    ///
    /// String payloads (`A = String`) live as `(ptr i32, len i32)` at
    /// offsets 4 and 8. We read both into `arm_payload_ptr()` and
    /// `arm_payload_ptr() + 1` so the arm body sees an ordinary
    /// string-shaped local pair.
    ///
    /// Numeric (`Int`-payload) and product-payload variants aren't
    /// extracted here yet — they remain a codegen gap. Zero-data
    /// variants (like `Pass = Unit` or stdlib `None`) have nothing to
    /// extract: the scope is returned unchanged.
    fn bind_arm_payload(
        &self,
        param_ty: &TypeExpr,
        base_scope: &LocalScope,
        f: &mut Function,
    ) -> LocalScope {
        let mut scope = base_scope.clone();
        let (bound_name, payload_ty) = self.arm_payload_binding(param_ty);
        if bound_name.is_empty() {
            return scope;
        }
        match &payload_ty {
            Ty::Str | Ty::NamedStr(_) => {
                // Load ptr at +4 into arm_payload_ptr
                f.instruction(&Instruction::LocalGet(base_scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(base_scope.arm_payload_ptr()));
                // Load len at +8 into arm_payload_ptr + 1
                f.instruction(&Instruction::LocalGet(base_scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 8,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(base_scope.arm_payload_ptr() + 1));
                scope
                    .vars
                    .insert(bound_name, (base_scope.arm_payload_ptr(), payload_ty));
            }
            _ => {
                // Other payload shapes (Int, product, etc.) aren't bound
                // yet — see CLAUDE.md § Known codegen gaps.
            }
        }
        scope
    }

    /// Given an arm's pattern `TypeExpr`, return `(bound_name, payload_ty)`:
    ///
    ///   * For a user variant like `(Fail)` where `Fail = String`, the
    ///     bound name is `"Fail"` and the payload type is
    ///     `Ty::NamedStr("Fail")` (the value retains its newtype identity).
    ///   * For a stdlib variant with a type argument like `(Some<String>)`,
    ///     the bound name is the type argument (`"String"`) and the payload
    ///     type is `Ty::Str`.
    ///   * For zero-data variants (like `(None)`, `(Pass)` where `Pass = Unit`),
    ///     returns `("", Ty::Unit)` — nothing to bind.
    fn arm_payload_binding(&self, param_ty: &TypeExpr) -> (String, Ty) {
        let TypeExpr::Named { name, generics, .. } = param_ty else {
            return (String::new(), Ty::Unit);
        };
        // Stdlib variant with explicit type argument: bind under the
        // inner type's name (e.g. `Some<String>` binds `String`).
        if !generics.is_empty() {
            if let Some(TypeExpr::Named {
                name: inner_name, ..
            }) = generics.first()
            {
                let payload_ty = self.resolve_repr(inner_name);
                return (inner_name.clone(), payload_ty);
            }
            return (String::new(), Ty::Unit);
        }
        // User variant: bind under the variant's own name. The payload
        // type is the variant's repr (which walks the alias chain), so
        // `Fail` with `Fail = String` gets `Ty::NamedStr("Fail")`.
        let payload_ty = self.resolve_repr(name);
        match &payload_ty {
            Ty::Unit => (String::new(), Ty::Unit),
            _ => (name.clone(), payload_ty),
        }
    }

    /// Reload match result from scratch locals.
    fn load_result(&self, result_ty: &Ty, scope: &LocalScope, f: &mut Function) -> Ty {
        match result_ty {
            Ty::Str | Ty::NamedStr(_) => {
                f.instruction(&Instruction::LocalGet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.rlen()));
                result_ty.clone()
            }
            Ty::I64 => {
                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                Ty::I64
            }
            Ty::I32 => {
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                Ty::I32
            }
            Ty::NamedPtr(n) => {
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                Ty::NamedPtr(n.clone())
            }
            _ => Ty::Unit,
        }
    }

    // ── Scratch save/load helpers ──────────────────────────────────────────────

    fn save_to_scratch(&mut self, ty: Ty, scope: &LocalScope, f: &mut Function) {
        self.save_ty_to_scratch(&ty, scope, f);
    }

    fn save_ty_to_scratch(&self, ty: &Ty, scope: &LocalScope, f: &mut Function) {
        match ty {
            Ty::I64 | Ty::F64 => {
                f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
            }
            Ty::Str | Ty::NamedStr(_) => {
                f.instruction(&Instruction::LocalSet(scope.rlen()));
                f.instruction(&Instruction::LocalSet(scope.rptr()));
            }
            Ty::List => {
                f.instruction(&Instruction::LocalSet(scope.rlen()));
                f.instruction(&Instruction::LocalSet(scope.rptr()));
            }
            Ty::Unit => {}
        }
    }

    fn load_from_scratch(&self, ty: &Ty, scope: &LocalScope, f: &mut Function) {
        match ty {
            Ty::I64 => {
                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
            }
            Ty::F64 => {
                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
            }
            Ty::Str | Ty::NamedStr(_) | Ty::List => {
                f.instruction(&Instruction::LocalGet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.rlen()));
            }
            Ty::Unit => {}
        }
    }

    /// Store a single payload value into the struct at `scope.alloc_ptr() + offset`.
    ///
    /// Stack contract on entry depends on `payload_ty`:
    ///   * Scalars (`Ty::I64`/`F64`/`I32`/`Ptr`/`NamedPtr`/`NamedPtrStr`):
    ///     `[address, value]` — one i32/i64 `store` consumes both.
    ///   * Strings (`Ty::Str`/`NamedStr`): `[address, ptr, len]`. We drop
    ///     the redundant address (we'll re-load it from `scope.alloc_ptr()`
    ///     for each store) and emit two i32 stores: `ptr` at `offset` and
    ///     `len` at `offset + 4`.
    ///   * `Ty::Unit`: just drops the address. There's no payload.
    fn store_payload_at_offset(
        &self,
        offset: u32,
        payload_ty: &Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) {
        match payload_ty {
            Ty::I64 | Ty::F64 => {
                f.instruction(&Instruction::I64Store(MemArg {
                    offset: offset as u64,
                    align: 3,
                    memory_index: 0,
                }));
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: offset as u64,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Ty::Str | Ty::NamedStr(_) => {
                // Stack: [addr, ptr, len]. Stash ptr+len, drop addr, then
                // re-load alloc_ptr twice for the two stores. We use
                // `tmp_i32`/`tmp_i32_b` because `rptr`/`rlen` (the
                // string scratch pair) are still holding the value the
                // caller pushed via `load_from_scratch`.
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // len
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // ptr
                f.instruction(&Instruction::Drop); // discard redundant addr
                                                   // Store ptr at +offset
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: offset as u64,
                    align: 2,
                    memory_index: 0,
                }));
                // Store len at +offset+4
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: (offset + 4) as u64,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Ty::Unit => {
                // No value to store — the address is still on the stack; drop it.
                f.instruction(&Instruction::Drop);
            }
            _ => {
                // Unexpected — just drop the address.
                f.instruction(&Instruction::Drop);
            }
        }
    }

    fn store_value_at_offset(&self, offset: u32, repr: &Ty, scope: &LocalScope, f: &mut Function) {
        self.store_payload_at_offset(offset, repr, scope, f);
    }

    // ── Local variable helpers ─────────────────────────────────────────────────

    fn push_local(&self, idx: u32, repr: &Ty, f: &mut Function) {
        match repr {
            Ty::I64 | Ty::F64 => {
                f.instruction(&Instruction::LocalGet(idx));
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                f.instruction(&Instruction::LocalGet(idx));
            }
            Ty::Str | Ty::NamedStr(_) | Ty::List => {
                f.instruction(&Instruction::LocalGet(idx));
                f.instruction(&Instruction::LocalGet(idx + 1));
            }
            Ty::Unit => {}
        }
    }

    // ── Print helpers ──────────────────────────────────────────────────────────

    fn emit_print(&self, ty: Ty, scope: &LocalScope, f: &mut Function) {
        match ty {
            Ty::I64 => {
                f.instruction(&Instruction::Call(self.fn_print_int));
            }
            Ty::F64 => {
                f.instruction(&Instruction::Drop);
            } // Phase 4+
            Ty::I32 => {
                f.instruction(&Instruction::Call(self.fn_print_bool));
            }
            Ty::Str | Ty::NamedStr(_) => {
                // print_str writes raw bytes — we always append a single `\n`
                // (the byte at `MEM_INT_BUF_END`) so `.print` produces one
                // line of output whether the receiver is a literal or a
                // host-returned string.
                f.instruction(&Instruction::Call(self.fn_print_str));
                f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::Call(self.fn_print_str));
            }
            Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) | Ty::Ptr | Ty::List => {
                self.drop_value(ty, f); // unknown print — drop
            }
            Ty::Unit => {}
        }
        let _ = scope;
    }

    fn drop_value(&self, ty: Ty, f: &mut Function) {
        match ty {
            Ty::Unit => {}
            Ty::I64 | Ty::F64 | Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                f.instruction(&Instruction::Drop);
            }
            Ty::Str | Ty::NamedStr(_) | Ty::List => {
                f.instruction(&Instruction::Drop);
                f.instruction(&Instruction::Drop);
            }
        }
    }

    // ── Main compile entry ─────────────────────────────────────────────────────

    fn compile(&mut self) -> Vec<u8> {
        // Pre-passes
        self.build_type_defs();
        self.build_variant_info();
        self.collect_all_strings();
        self.assign_func_indices();

        // Register the one waitable signature that isn't already covered
        // by the fixed TY_* slots: `(i32, i32) -> i32` for
        // `waitable-set.wait`. The other four intrinsics reuse existing
        // types (`waitable-set.new` = TY_RUN, `waitable.join` =
        // TY_PRINT_STR, `waitable-set.drop` and `subtask.drop` =
        // TY_PRINT_BOOL).
        let ty_waitable_set_wait =
            self.get_or_add_wasm_type(&[ValType::I32, ValType::I32], &[ValType::I32]);

        let mut m = Module::new();

        // ── Type section ───────────────────────────────────────────────
        // Indices here must match the TY_* constants above.
        let mut types = TypeSection::new();
        // 0: print_str    (i32, i32) -> ()
        types.ty().function([ValType::I32, ValType::I32], []);
        // 1: print_int    (i64) -> ()
        types.ty().function([ValType::I64], []);
        // 2: print_bool   (i32) -> ()  — also used by waitable-set.drop,
        //                                  subtask.drop, task.return,
        //                                  stream.drop-writable,
        //                                  future.drop-readable
        types.ty().function([ValType::I32], []);
        // 3: run          () -> ()   (async-stackful lift; result via task.return)
        types.ty().function([], []);
        // 4: alloc        (i32) -> (i32)
        types.ty().function([ValType::I32], [ValType::I32]);
        // 5: stdout write-via-stream  (i32 readable) -> (i32 future)
        types.ty().function([ValType::I32], [ValType::I32]);
        // 6: stdout stream-new        () -> (i64 packed handles)
        types.ty().function([], [ValType::I64]);
        // 7: stdout stream-write      (i32 writable, i32 ptr, i32 len) -> (i32 status)
        types
            .ty()
            .function([ValType::I32, ValType::I32, ValType::I32], [ValType::I32]);
        // 8: handle return             () -> (i32)   — waitable-set.new
        types.ty().function([], [ValType::I32]);
        // User function types
        let user_sigs: Vec<_> = self.user_type_sigs.clone();
        for (params, results) in &user_sigs {
            types
                .ty()
                .function(params.iter().cloned(), results.iter().cloned());
        }
        m.section(&types);

        // ── Import section ───────────────────────────────────────────────────
        // The component wrapper provides:
        //   - wasi:cli/stdout.{write-via-stream, stream-new, stream-write,
        //         stream-drop-writable, future-drop-readable}: the five
        //         canonical-ABI builtins `print_str` stitches into the
        //         native WASI P3 stdout sequence.
        //   - one function per user `extern Wasm` declaration (sorted)
        //   - oneway:async/waitable.*: 6 canonical async/task helpers
        //   - env.memory, env.bump_ptr: shared linear memory + bump
        //         pointer used by `$alloc` and the host's `cabi_realloc`.
        let mut imports = ImportSection::new();
        imports.import(
            "wasi:cli/stdout",
            "write-via-stream",
            EntityType::Function(TY_STDOUT_WRITE_VIA_STREAM),
        );
        imports.import(
            "wasi:cli/stdout",
            "stream-new",
            EntityType::Function(TY_STDOUT_STREAM_NEW),
        );
        imports.import(
            "wasi:cli/stdout",
            "stream-write",
            EntityType::Function(TY_STDOUT_STREAM_WRITE),
        );
        imports.import(
            "wasi:cli/stdout",
            "stream-drop-writable",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        imports.import(
            "wasi:cli/stdout",
            "future-drop-readable",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        for ext in &self.extern_imports {
            let type_idx = *self
                .user_type_map
                .get(&(ext.params.clone(), ext.results.clone()))
                .expect("extern import type was added during assign_func_indices");
            imports.import(
                &ext.core_namespace,
                &ext.fn_name,
                EntityType::Function(type_idx),
            );
        }
        // Waitable intrinsics — see field doc on `fn_waitable_*`. The
        // synthetic core instance built in `component::wrap` (from a
        // canon section emitting `waitable-set.new`, `waitable.join`,
        // `waitable-set.wait`, `waitable-set.drop`, `subtask.drop`)
        // satisfies these. Names are kebab-case to match the canon
        // operator names.
        imports.import(
            "oneway:async/waitable",
            "set-new",
            EntityType::Function(TY_HANDLE_RETURN), // () -> i32
        );
        imports.import(
            "oneway:async/waitable",
            "join",
            EntityType::Function(TY_PRINT_STR), // (i32, i32) -> ()
        );
        imports.import(
            "oneway:async/waitable",
            "set-wait",
            EntityType::Function(ty_waitable_set_wait), // (i32, i32) -> i32
        );
        imports.import(
            "oneway:async/waitable",
            "set-drop",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        imports.import(
            "oneway:async/waitable",
            "subtask-drop",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        imports.import(
            "oneway:async/waitable",
            "task-return",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> () — result<_,_> tag
        );
        imports.import(
            "env",
            "memory",
            EntityType::Memory(MemoryType {
                minimum: 2,
                maximum: None,
                memory64: false,
                shared: false,
                page_size_log2: None,
            }),
        );
        // Shared bump pointer for $alloc and the host-side `cabi_realloc`.
        imports.import(
            "env",
            "bump_ptr",
            EntityType::Global(GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            }),
        );
        m.section(&imports);

        // ── Function section ─────────────────────────────────────────────────────────
        // Defined functions in the order they appear in the function index
        // space (right after the import block).
        let mut funcs = FunctionSection::new();
        funcs.function(TY_PRINT_STR);
        funcs.function(TY_PRINT_INT);
        funcs.function(TY_PRINT_BOOL);
        funcs.function(TY_ALLOC);
        funcs.function(TY_RUN); // exported run() -> i32
                                // User-compiled functions only — extern imports are already declared
                                // in the import section and must NOT get a defined-function slot.
        let mut user_func_defs: Vec<(u32, u32)> = self
            .func_table
            .values()
            .filter(|info| info.func_idx >= self.fn_user_start)
            .map(|info| (info.func_idx, info.type_idx))
            .collect();
        user_func_defs.sort_by_key(|(idx, _)| *idx);
        for (_, type_idx) in &user_func_defs {
            funcs.function(*type_idx);
        }
        m.section(&funcs);

        // ── Memory section ────────────────────────────────────────────────────────────
        // We import the memory rather than declaring our own — the component
        // wrapper instantiates a tiny "memory provider" core module first so
        // that the canonical-ABI lowers (which need a memory option) can
        // reference it before this module is instantiated.

        // ── Global section ─────────────────────────────────────────────────────────────────
        // Empty — the bump_ptr global is imported, not defined here.

        // ── Export section ─────────────────────────────────────────────────────────────
        // The Component Model wrapper lifts `run` as `wasi:cli/run.run`.
        let mut exports = ExportSection::new();
        exports.export("run", ExportKind::Func, self.fn_start);
        m.section(&exports);

        // ── Code section ──────────────────────────────────────────────────────
        let mut codes = CodeSection::new();
        codes.function(&self.build_print_str());
        codes.function(&self.build_print_int());
        codes.function(&self.build_print_bool());
        codes.function(&self.build_alloc());
        codes.function(&self.build_start());
        // User functions — compile each FunctionDef in ascending func_idx order
        let ordered_funcs: Vec<FunctionDef> = {
            let mut pairs: Vec<(u32, FunctionDef)> = Vec::new();
            for item in self.ast.items.iter() {
                if let Item::Function(func) = item {
                    if func.name.name == "main" && func.receiver.is_none() {
                        continue;
                    }
                    if func.extern_wasm.is_some() {
                        continue;
                    }
                    let key = (
                        func.receiver.as_ref().map(|r| r.name.clone()),
                        func.name.name.clone(),
                    );
                    if let Some(info) = self.func_table.get(&key) {
                        pairs.push((info.func_idx, func.clone()));
                    }
                }
            }
            pairs.sort_by_key(|(idx, _)| *idx);
            pairs.into_iter().map(|(_, f)| f).collect()
        };
        for func in ordered_funcs {
            let compiled = self.build_user_function(&func);
            codes.function(&compiled);
        }
        m.section(&codes);

        // ── Data section ──────────────────────────────────────────────────────
        let mut data = DataSection::new();
        // '\n' at offset MEM_INT_BUF_END
        data.active(0, &ConstExpr::i32_const(MEM_INT_BUF_END as i32), [b'\n']);
        if !self.strings.data.is_empty() {
            data.active(
                0,
                &ConstExpr::i32_const(MEM_STR_START as i32),
                self.strings.data.clone(),
            );
        }
        m.section(&data);

        m.finish()
    }
}

// ── Arm-type helpers ──────────────────────────────────────────────────────────

/// Size in bytes of the ret-area an async-lowered call writes its result
/// into. The layout matches what the canonical ABI's async lower
/// expects: a single packed value at offset 0, aligned to its natural
/// boundary. Returns a multiple of 4 so the bump allocator's 4-byte
/// alignment is sufficient.
fn ret_area_size_for(ty: &Ty) -> u32 {
    match ty {
        Ty::Str | Ty::NamedStr(_) => 8, // (i32 ptr, i32 len)
        Ty::List => 8,                  // (i32 ptr, i32 len)
        Ty::I64 | Ty::F64 => 8,         // 8-byte scalar
        Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) => 4,
        // Tagged-string unions (Result<Ok-str, Err-str>): tag + ptr + len.
        Ty::NamedPtrStr(_, _, _) => 12,
        Ty::Unit => 0,
    }
}

fn arm_type_name(arm: &MatchArm) -> Option<&str> {
    if let TypeExpr::Named { name, .. } = &arm.param_ty {
        Some(name.as_str())
    } else {
        None
    }
}

/// True when this arm's pattern names `variant_name`. Used by the
/// N-variant dispatch to pair each variant tag with the arm that
/// handles it. Matches by exact name only — the 2-variant fast path
/// has extra fallbacks (`Some`/`Ok`/`True` for the `1` tag,
/// `None`/`Err`/`False` for `0`) because the built-in unions don't
/// always go through `union_variants`. For user-defined N-variant
/// unions, the variant names are exactly what the user wrote, so a
/// plain match is enough.
fn arm_matches_variant(arm: &MatchArm, variant_name: &str) -> bool {
    arm_type_name(arm) == Some(variant_name)
}

/// Newtype field access: for `A = B`, `aValue.B` returns the underlying
/// `B` value with the same wire representation but retyped. Returns the
/// post-unwrap `Ty` to leave on the stack, or `None` when the field name
/// doesn't match the newtype's underlying type (in which case the caller
/// falls back to drop-and-Unit for real products).
///
/// Handles string-shaped newtypes (the common case): an `A = String`
/// value lives on the stack as `(ptr, len)`, and `.String` keeps both
/// values on the stack while changing the static type from
/// `Ty::NamedStr("A")` to `Ty::Str`. Numeric newtypes don't currently
/// carry their alias name through the codegen, so they need no work
/// here — the field-access expression is already a no-op at the wasm
/// level.
fn newtype_unwrap_ty(recv_ty: &Ty, field: &str) -> Option<Ty> {
    match (recv_ty, field) {
        (Ty::NamedStr(_), "String") => Some(Ty::Str),
        (Ty::Str, "String") => Some(Ty::Str), // idempotent
        _ => None,
    }
}

/// Returns the discriminant tag for this arm, based on known variant names.
fn arm_tag(arm: &MatchArm) -> Option<u32> {
    match arm_type_name(arm)? {
        "False" | "None" | "Err" => Some(0),
        "True" | "Some" | "Ok" => Some(1),
        _ => None,
    }
}

// ── Validation ─────────────────────────────────────────────────────────────────
pub(super) fn validate(bytes: &[u8]) {
    use wasmparser::{Parser, Validator, WasmFeatures};
    let mut v = Validator::new_with_features(WasmFeatures::all());
    for payload in Parser::new(0).parse_all(bytes) {
        let p = match payload {
            Ok(p) => p,
            Err(e) => {
                eprintln!("internal error: generated invalid wasm (parse): {e}");
                std::process::exit(1);
            }
        };
        if let Err(e) = v.payload(&p) {
            eprintln!("internal error: generated invalid wasm (validate): {e}");
            std::process::exit(1);
        }
    }
}

/// Emits the raw core WASM module — used by the Component Model wrapper.
fn generate_core_module(module: &OModule) -> Vec<u8> {
    let mut gen = WasmGen::new(module);
    gen.compile()
}

/// Builds the final Component Model component (`.wasm` bytes).
///
/// The output is a WASI Preview 3 component that exports
/// `wasi:cli/run@0.3.0-rc-2026-03-15`, imports `wasi:cli/stdout@0.3.0-rc-2026-03-15`,
/// and additionally imports every interface referenced by an `extern Wasm`
/// declaration in the user program.
/// It is validated with `wasmparser` before being returned.
pub fn generate(module: &OModule) -> Vec<u8> {
    let core = generate_core_module(module);
    let externs = collect_extern_imports(module);
    // Run the async-inference fixpoint so the component wrapper can
    // surface async metadata in the emitted WIT and — once async lowering
    // lands — attach `CanonicalOption::Async` to the right lifts/lowers.
    let async_set = crate::codegen::async_analysis::analyse(module);
    let bytes = component::wrap(&core, &externs, &async_set);
    validate(&bytes);
    bytes
}

/// Returns the WIT world description that accompanies the compiled `.wasm`.
pub fn generate_wit(module: &OModule) -> String {
    let async_set = crate::codegen::async_analysis::analyse(module);
    component::generate_wit(module, &async_set)
}

/// Phase 1 stub — print a brief summary of the emitted component.
pub fn generate_wat(module: &OModule) -> String {
    let bytes = generate(module);
    format!(
        ";; WAT disassembly not yet implemented.\n;; Component is {} bytes.\n(component ...)\n",
        bytes.len()
    )
}
