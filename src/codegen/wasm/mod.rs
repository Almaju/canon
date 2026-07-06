/// Canon WASM codegen — emits a core module which is then wrapped into a
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
///     runtime (no `canon:*` host bridge required for output).
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
    Function, FunctionSection, GlobalSection, GlobalType, ImportSection, Instruction, MemArg,
    MemorySection, MemoryType, Module, TypeSection, ValType,
};

use crate::ast::{
    ArmLiteral, Block, Expr, FunctionDef, Item, MatchArm, Module as OModule, TypeExpr,
};

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

// ── HTTP-mode import indices ─────────────────────────────────────────
// In HTTP encoder mode (`http_mode`, see `compile_http`) the import
// space is fixed: the five stdout builtins keep indices 0..4 (under
// `wit-component` naming conventions), then the `wasi:http/types`
// functions/intrinsics plus the task-return intrinsic for the
// async-stackful `handle` lift. No extern-Wasm or waitable imports
// exist in this mode; defined functions start at `HTTP_BASE_DEFINED`.
const FN_HTTP_FIELDS_CTOR: u32 = 5; // [constructor]fields          () -> i32
const FN_HTTP_RESPONSE_NEW: u32 = 6; // [static]response.new        (i32 x5) -> ()
const FN_HTTP_FUTURE_NEW: u32 = 7; // [future-new-1][static]response.new  () -> i64
const FN_HTTP_FUTURE_WRITE: u32 = 8; // [future-write-1]… (sync)    (i32,i32) -> i32
const FN_HTTP_FUTURE_DROP_READABLE: u32 = 9; // [future-drop-readable-2]…  (i32) -> ()
const FN_HTTP_FUTURE_DROP_WRITABLE: u32 = 10; // [future-drop-writable-1]… (i32) -> ()
const FN_HTTP_REQUEST_DROP: u32 = 11; // [resource-drop]request      (i32) -> ()
const FN_HTTP_SET_STATUS: u32 = 12; // [method]response.set-status-code (i32,i32) -> i32
const FN_HTTP_STREAM_NEW: u32 = 13; // [stream-new-0][static]response.new () -> i64
const FN_HTTP_STREAM_WRITE: u32 = 14; // [stream-write-0]… (sync)   (i32,i32,i32) -> i32
const FN_HTTP_STREAM_DROP_WRITABLE: u32 = 15; // [stream-drop-writable-0]… (i32) -> ()
const FN_HTTP_TASK_RETURN: u32 = 16; // [task-return]handle
const FN_HTTP_GET_PATH: u32 = 17; // [method]request.get-path-with-query (i32,i32) -> ()
const FN_HTTP_FIELDS_APPEND: u32 = 18; // [method]fields.append (i32 x6) -> ()
const FN_HTTP_GET_METHOD: u32 = 19; // [method]request.get-method (i32,i32) -> ()
const HTTP_BASE_DEFINED: u32 = 20;

// ── Web-mode import indices ──────────────────────────────────────────
// In web encoder mode (`compile_web`, see the web target, docs/src/reference/web-target.md) the import
// space is just the five stdout builtins at 0..4 — the bundled JS host
// (`canon-web.js`) stubs them onto `console.log`. Defined functions
// start right after.
const WEB_BASE_DEFINED: u32 = 5;

/// Flat core shape of a web app's model value (what the user's `init`
/// returns), used by the export wrappers to normalize the model to
/// the single opaque i64 the JS host threads through `update`/`view`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebModelShape {
    /// `Int`-aliased model — already i64.
    I64,
    /// `Float`-aliased model — reinterpreted to i64.
    F64,
    /// Product/union/option model — a heap pointer, zero-extended.
    Ptr,
    /// `String`-aliased model — (ptr, len) boxed into an 8-byte cell.
    Str,
}

// Fixed scratch addresses used by HTTP-mode response construction.
// `build_http_response` runs *inside* the user function; the sync
// body/trailer writes must happen *after* `task.return` hands the
// response to the host (they block until the host consumes them), so
// construction stashes the write-phase state at fixed addresses for
// the `handle` wrapper to pick up. Addresses 0..16 are otherwise
// unused (the int buffer is 16..32, '\n' at 32, strings from 64) and
// no user code runs between the stores and the loads.
const MEM_HTTP_BODY_WRITER: u32 = 0; // contents-stream writer (0 = no body)
const MEM_HTTP_BODY_PTR: u32 = 4;
const MEM_HTTP_BODY_LEN: u32 = 8;
const MEM_HTTP_TRAILERS_WRITER: u32 = 12;
const MEM_HTTP_RET: u32 = 16; // response.new tuple ret (int-buffer reuse is safe)
const MEM_HTTP_TRAILERS_ZERO: u32 = 40; // `ok(none)` — all zero bytes, never written

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
/// deterministic across runs (matching Canon's "alphabetical" ethos).
/// The `(start, end)` bound expressions of a `substring`/`slice` call.
/// Canonically the bounds arrive as a `From * To` product, which is
/// *positionless*: the start is whichever component is `From(…)` and the
/// end whichever is `To(…)`, regardless of written order. Two positional
/// args are still accepted during migration (start, then end).
fn substring_bounds(args: &[Expr]) -> Option<(&Expr, &Expr)> {
    fn ctor_name(e: &Expr) -> Option<&str> {
        match e {
            Expr::Constructor { name, .. } => Some(name.name.as_str()),
            _ => None,
        }
    }
    match args {
        [Expr::ProductValue { fields, .. }] if fields.len() == 2 => {
            let (a, b) = (&fields[0], &fields[1]);
            if ctor_name(a) == Some("To") || ctor_name(b) == Some("From") {
                Some((b, a))
            } else {
                Some((a, b))
            }
        }
        [a, b] => Some((a, b)),
        _ => None,
    }
}

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
        // `canon:builtins/concurrent` is a *synthetic* interface — the
        // codegen recognises `parallel(…)` and `race(…)` as built-in
        // combinators and emits the multi-subtask wait sequence inline
        // (see `compile_parallel` / `compile_race`). It has no host
        // implementation; skipping it from the import collection prevents
        // the linker from looking for one.
        if component_ns.starts_with("canon:builtins/concurrent") {
            continue;
        }

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
        let mut component_params = component_params;
        let mut component_result: Option<wasm_encoder::PrimitiveValType> = None;
        let mut narrow_params: Vec<bool> = Vec::new();
        let mut narrow_result_signed: Option<bool> = None;

        // WIT-informed lowering: for `wasi:*` imports the vendored WIT
        // is the source of truth for integer widths and signedness —
        // Canon's single `Int` erases both. Narrow widths (u8..u32,
        // s8..s32) lower to core i32; `emit_func_table_call` inserts
        // the i64↔i32 conversions at call sites.
        if component_ns.starts_with("wasi:") {
            if let Some((wit_params, wit_result)) = component::vendored_extern_prim_sig(&ext.path) {
                let mut flat = 0usize;
                for (i, kind) in component_params.iter_mut().enumerate() {
                    let slots = match kind {
                        ParamKind::Scalar(_) => 1,
                        ParamKind::String => 2,
                    };
                    let mut converts = false;
                    if let (ParamKind::Scalar(_), Some(Some(prim))) = (&*kind, wit_params.get(i)) {
                        *kind = ParamKind::Scalar(*prim);
                        if is_narrow_prim(*prim)
                            && flat < params.len()
                            && params[flat] == ValType::I64
                        {
                            params[flat] = ValType::I32;
                            converts = true;
                        }
                    }
                    narrow_params.push(converts);
                    flat += slots;
                }
                if let Some(Some(prim)) = wit_result {
                    if results.len() == 1 {
                        component_result = Some(prim);
                        if is_narrow_prim(prim) && results[0] == ValType::I64 {
                            results[0] = ValType::I32;
                            use wasm_encoder::PrimitiveValType as P;
                            narrow_result_signed = Some(matches!(prim, P::S8 | P::S16 | P::S32));
                        }
                    }
                }
            }
        }

        // Record-of-scalars returns (e.g. `instant`): the vendored WIT
        // tells us the exact canonical layout; the decode rebuilds the
        // Canon product. Takes precedence over `classify_return`, which
        // would otherwise misread the product's flat i32 pointer repr
        // as a scalar.
        let mut record_shape: Option<IndirectReturnShape> = None;
        if component_ns.starts_with("wasi:") {
            if let (Some((wit_name, wit_fields)), Some(product)) = (
                component::vendored_extern_record_return(&ext.path),
                named_type_name(&func.return_ty),
            ) {
                let mut off = 0u32;
                let mut max_align = 1u32;
                let mut fields = Vec::new();
                for (fname, prim) in &wit_fields {
                    let (fsize, falign) = prim_size_align(*prim);
                    off = off.div_ceil(falign) * falign;
                    max_align = max_align.max(falign);
                    fields.push(RecordField {
                        wit_name: fname.clone(),
                        canon_name: format!(
                            "{}{}",
                            product,
                            crate::bindgen::naming::kebab_to_pascal(fname)
                        ),
                        prim: *prim,
                        offset: off,
                    });
                    off += fsize;
                }
                let size = off.div_ceil(max_align) * max_align;
                record_shape = Some(IndirectReturnShape::ScalarRecord {
                    wit_name,
                    product,
                    fields,
                    size,
                });
                results = vec![ValType::I32];
            }
        }

        // Determine the result shape. We support: nothing, a single flat
        // scalar, a bare `string`, or `result<string-alias, string-alias>`.
        // Anything else is too exotic for the current canonical-ABI
        // lowerings.
        let indirect_return =
            record_shape.or_else(|| classify_return(&func.return_ty, &results, &type_defs));
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
            component_result,
            narrow_params,
            narrow_result_signed,
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
        if name == "Option" && generics.len() == 1 && resolves_to_string(&generics[0], type_defs) {
            return Some(IndirectReturnShape::OptionString);
        }
        if name == "List" && generics.len() == 1 && resolves_to_string(&generics[0], type_defs) {
            return Some(IndirectReturnShape::ListString);
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

/// True for WIT integer widths below 64 bits — these lower to core
/// `i32` while Canon's `Int` is `i64`, so call sites wrap/extend.
fn is_narrow_prim(p: wasm_encoder::PrimitiveValType) -> bool {
    use wasm_encoder::PrimitiveValType as P;
    matches!(p, P::U8 | P::U16 | P::U32 | P::S8 | P::S16 | P::S32)
}

/// Canonical-ABI size and alignment of a scalar primitive.
fn prim_size_align(p: wasm_encoder::PrimitiveValType) -> (u32, u32) {
    use wasm_encoder::PrimitiveValType as P;
    match p {
        P::Bool | P::U8 | P::S8 => (1, 1),
        P::U16 | P::S16 => (2, 2),
        P::U32 | P::S32 | P::F32 | P::Char => (4, 4),
        _ => (8, 8),
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

/// Coarse mapping from a Canon type expression to its WASM stack types.
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
            // declared in the stdlib (`std/http-server-wasm.can`).
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
// helper. Both bump from the same pointer, which keeps Canon-allocated heap
// data and host-allocated string returns in a single coherent heap.
const GLOBAL_BUMP_PTR: u32 = 0;

// ── WASM representation of a Canon expression ──────────────────────────────

/// What a compiled expression leaves on the WASM stack.
///
/// The `Named*` variants carry the Canon type name so method dispatch can
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

    /// The Canon type name, if known (used for method dispatch).
    fn canon_name(&self) -> Option<&str> {
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

/// Maps Canon parameter names to their local variable index + repr.
///
/// Extra locals (indices after params, declared via `extra_locals_decl()`):
///   pc+0, pc+1  (i32): rptr, rlen   — for Str match results
///   pc+2        (i32): rbool         — for I32/Ptr match results
///   pc+3        (i32): tmp_i32       — general scratch i32
///   pc+4        (i64): tmp_i64       — general scratch i64
///   pc+5        (i32): alloc_ptr     — result of $alloc
///   pc+6        (i32): tmp_i32_b     — second scratch i32
///   pc+7, pc+8  (i32): arm_payload_ptr (+1) — bound arm payload
///   pc+9, pc+10 (i32): str_scratch_ptr (+1) — string-builtin scratch
///   pc+11..pc+18 (i32): par_subtask_a/b, par_retarea_a/b, par_set,
///                       par_event_ptr, par_seen_a/b — parallel/race state.
///                       Eight locals, kept always-on so the wasm validator
///                       sees a stable local layout regardless of whether
///                       the function actually uses concurrency combinators.
///                       Cost: ~32 bytes of dead locals per non-using
///                       function, which is fine.
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
    /// Adjacent pair of i32s reserved as scratch for string-shaped
    /// builtins (`concat`, `substring`, …) that need to stash a
    /// `(ptr, len)` pair across an `$alloc` + copy loop. Kept distinct
    /// from `arm_payload_ptr` so a builtin call inside a dispatch arm
    /// body can't corrupt the bound payload — see the
    /// "Heap allocations inside `Ok`/`Err` dispatch arm bodies" gap in
    /// CLAUDE.md.
    fn str_scratch_ptr(&self) -> u32 {
        self.param_count + 9
    }

    // ── Parallel / race scratch locals ───────────────────────────────
    //
    // Eight i32s used by `compile_parallel` and `compile_race` to thread
    // the multi-subtask wait state through the emitted instruction stream.
    // Kept in a contiguous block from `pc+11..pc+18` so the wasm validator
    // can statically prove they exist regardless of the call site.
    fn par_subtask_a(&self) -> u32 {
        self.param_count + 11
    }
    fn par_subtask_b(&self) -> u32 {
        self.param_count + 12
    }
    fn par_retarea_a(&self) -> u32 {
        self.param_count + 13
    }
    fn par_retarea_b(&self) -> u32 {
        self.param_count + 14
    }
    fn par_set(&self) -> u32 {
        self.param_count + 15
    }
    fn par_event_ptr(&self) -> u32 {
        self.param_count + 16
    }
    fn par_seen_a(&self) -> u32 {
        self.param_count + 17
    }
    fn par_seen_b(&self) -> u32 {
        self.param_count + 18
    }

    /// Single i32 scratch holding a store-target address for the
    /// duration of one `store_payload_at_offset` string store. Only
    /// ever live between adjacent instructions (never across a nested
    /// `compile_expr`), so it can't be clobbered by nested
    /// constructors the way `alloc_ptr` can.
    fn addr_scratch(&self) -> u32 {
        self.param_count + 19
    }

    /// f64 scratch, the floating-point sibling of `tmp_i64`. Kept
    /// separate because wasm locals are monomorphically typed — an f64
    /// value cannot pass through the i64-typed `tmp_i64` without an
    /// explicit reinterpret, and mixing the two was exactly the bug
    /// that made `Float` union payloads emit invalid wasm.
    fn tmp_f64(&self) -> u32 {
        self.param_count + 20
    }

    /// i64 local holding the current element while a `list.map` lambda
    /// body runs. The lambda's parameter name binds to this slot.
    /// Caveat: a `.map` nested inside another `.map`'s lambda body
    /// reuses the slot, clobbering the outer element — acceptable
    /// until real iteration state lands.
    fn map_elem_i64(&self) -> u32 {
        self.param_count + 21
    }

    /// Adjacent i32 pair holding the current `(ptr, len)` string
    /// element during `list.map`, and doubling as the result stash
    /// between the lambda body finishing and the store into the
    /// destination list. Same nesting caveat as `map_elem_i64`.
    fn map_elem_ptr(&self) -> u32 {
        self.param_count + 22
    }

    /// Adjacent i32 pair holding the scrutinee `(ptr, len)` across a
    /// string literal-dispatch compare chain (`* ("/notes") -> …`).
    /// Kept distinct from `arm_payload_ptr` and the eq-compare scratch
    /// (`rptr`/`rbool`/`tmp_i32`/`tmp_i32_b`) so each successive
    /// compare — and the scrutinee binding inside arm bodies — reads
    /// an unclobbered value. Same single-slot nesting caveat as
    /// `arm_payload_ptr`: a literal dispatch nested inside another
    /// literal dispatch's arm body reuses the pair.
    fn lit_scrut_ptr(&self) -> u32 {
        self.param_count + 24
    }

    /// i64 sibling of `lit_scrut_ptr` for `Int` literal dispatch.
    fn lit_scrut_i64(&self) -> u32 {
        self.param_count + 26
    }

    /// Second f64 scratch. `Float.rem` needs both operands available
    /// twice (`a - trunc(a/b) * b`), and wasm has no stack dup — the
    /// pair of f64 locals holds `a`/`b` across the sequence.
    fn tmp_f64_b(&self) -> u32 {
        self.param_count + 27
    }
}

/// Local declarations appended after the function params.
fn extra_locals_decl() -> Vec<(u32, ValType)> {
    vec![
        (4, ValType::I32), // rptr, rlen, rbool, tmp_i32
        (1, ValType::I64), // tmp_i64
        (2, ValType::I32), // alloc_ptr, tmp_i32_b
        (2, ValType::I32), // arm_payload_ptr, arm_payload_ptr + 1 (len)
        (2, ValType::I32), // str_scratch_ptr, str_scratch_ptr + 1 (len)
        (8, ValType::I32), // par_subtask_a/b, par_retarea_a/b, par_set,
        // par_event_ptr, par_seen_a/b (parallel/race state)
        (1, ValType::I32), // addr_scratch (store-target address)
        (1, ValType::F64), // tmp_f64
        (1, ValType::I64), // map_elem_i64 (list.map current element)
        (2, ValType::I32), // map_elem_ptr, map_elem_ptr + 1 (len)
        (2, ValType::I32), // lit_scrut_ptr, lit_scrut_ptr + 1 (len)
        (1, ValType::I64), // lit_scrut_i64 (Int literal-dispatch scrutinee)
        (1, ValType::F64), // tmp_f64_b (Float.rem second operand)
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
    /// Per-component-parameter conversion flags for extern functions
    /// (empty for user body functions): true where the WIT-informed
    /// lowering narrowed Canon's i64 `Int` slot to core i32, so the
    /// call site must `i32.wrap_i64` that argument.
    narrow_params: Vec<bool>,
    /// `Some(signed)` when the extern's result narrowed from i64 to
    /// i32 — the call site extends back to Canon's i64.
    narrow_result_signed: Option<bool>,
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
    /// Canon's alphabetical convention (Err=0, Ok=1) and pushes the
    /// area pointer as `Ty::NamedPtrStr(union, ok_name, err_name)`. The
    /// three names preserve Canon-level types through `?` and dispatch
    /// so subsequent method calls find their externs (e.g. `.read()`
    /// after `Path(…).File()?`) and the Err arm of a `match` can type
    /// the bound payload (e.g. `Err(e) =>` where `e: IoError`).
    ResultStringString { ok_name: String, err_name: String },
    /// `option<string>` return. Return area: 12 bytes — byte 0 the
    /// discriminant (0=none, 1=some), bytes 4–7 the payload ptr, 8–11
    /// the payload len. Decoded into a fresh Canon `Option` struct
    /// (i32 tag at +0, payload at +4/+8) so ordinary
    /// `(None, Some<String>)` dispatch works.
    OptionString,
    /// `list<string>` return. Return area: 8 bytes — (i32 list ptr,
    /// i32 element count). The canonical-ABI element layout (8-byte
    /// stride, i32 ptr + i32 len per element) is byte-identical to
    /// Canon's `List<String>` representation, so the pair is pushed
    /// directly as `Ty::List`.
    ListString,
    /// A record whose fields are all scalar primitives (e.g.
    /// `wasi:clocks/system_clock#now`'s `instant`). The host writes
    /// the canonical record layout into the ret area; the decode
    /// copies each field into a fresh Canon product struct (the
    /// bindgen renders the record as `Product = ProductFieldA *
    /// ProductFieldB` with `Int`-newtype fields), widening narrow
    /// ints to i64 on the way.
    ScalarRecord {
        /// WIT type name in kebab (`"instant"`) — the component-level
        /// record type is exported under this name.
        wit_name: String,
        /// Canon product type name (`"Instant"`).
        product: String,
        /// Per-field decode info, in WIT declaration order.
        fields: Vec<RecordField>,
        /// Canonical size of the record (ret-area allocation).
        size: u32,
    },
}

/// One field of a `ScalarRecord` indirect return.
#[derive(Clone, Debug)]
pub(super) struct RecordField {
    /// WIT field name in kebab (`"nanoseconds"`).
    pub(super) wit_name: String,
    /// Canon product field name (`"InstantNanoseconds"`).
    pub(super) canon_name: String,
    pub(super) prim: wasm_encoder::PrimitiveValType,
    /// Byte offset within the canonical record layout.
    pub(super) offset: u32,
}

impl IndirectReturnShape {
    /// Size of the return area in bytes (must be a multiple of 4).
    fn return_area_size(&self) -> u32 {
        match self {
            IndirectReturnShape::String => 8,
            IndirectReturnShape::ResultStringString { .. } => 12,
            IndirectReturnShape::OptionString => 12,
            IndirectReturnShape::ListString => 8,
            IndirectReturnShape::ScalarRecord { size, .. } => (*size).max(4),
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
    /// Component-level import name, e.g. `"canon:builtins/math@0.1.0"`.
    /// Multiple functions can share the same `component_namespace` — they end
    /// up as members of the same imported instance.
    pub(super) component_namespace: String,
    /// Core-module import-module name, e.g. `"canon:builtins/math"` (no
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
    /// Logical component-level parameters, one entry per Canon argument
    /// (receiver-first if present), with their `ParamKind`. The component
    /// wrapper uses this list to build the imported instance's function type.
    pub(super) component_params: Vec<ParamKind>,
    /// The WIT-declared primitive result type, when the vendored WIT
    /// knows better than the Canon-derived guess (narrow ints, exact
    /// signedness). `extern_result_to_component` prefers this.
    pub(super) component_result: Option<wasm_encoder::PrimitiveValType>,
    /// Per-component-parameter: true when the WIT-informed override
    /// changed the core slot from Canon's i64 `Int` to a narrow i32 —
    /// call sites must `i32.wrap_i64` that argument. Bool params are
    /// i32 on both sides and never set this.
    pub(super) narrow_params: Vec<bool>,
    /// `Some(signed)` when the result slot narrowed from i64 to i32 —
    /// call sites extend back (sign- or zero-extending).
    pub(super) narrow_result_signed: Option<bool>,
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

    // Every compiled user function in func-index order: (func_idx,
    // type_idx, def). The single source of truth for the emitted
    // function and code sections — `func_table` is a *lookup* structure
    // whose keys can collide (constructor families register several
    // bodies for one type name; the commutative aliases point several
    // keys at one body), so deriving section contents from it lets the
    // function-section and code-section lengths drift apart, which is
    // invalid wasm. This list cannot: one entry per compiled body.
    compiled_user_funcs: Vec<(u32, u32, FunctionDef)>,

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
    // path. They're imported as `canon:async/waitable.<name>` (a
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
    fn_subtask_cancel: u32,    // `(i32)      -> i32`  (subtask) -> new state
    //                                                  Used by `compile_race` to
    //                                                  abandon the loser. The
    //                                                  i32 result is the new
    //                                                  CallState; we drop it.
    fn_print_str: u32,
    fn_print_int: u32,
    fn_print_bool: u32,
    fn_alloc: u32,
    fn_start: u32, // exported as "run"
    /// Helper that converts a `List<String>` (list of pre-encoded JSON
    /// values) into a single `Json` string, joining elements with `,`
    /// and wrapping with `[`/`]`. Always emitted — unused programs pay
    /// a few hundred bytes of dead code, which is acceptable for now.
    /// Core signature: `(list_ptr: i32, list_len: i32) -> (i32, i32)`.
    /// See `build_list_to_json_array` for the body.
    fn_list_to_json_array: u32,
    /// Formats and prints an `f64` (fixed-point, up to 6 fraction
    /// digits, trailing zeros trimmed; `NaN` / `Inf` / `-Inf` for the
    /// specials). Core signature: `(f64) -> ()`. See
    /// `build_print_float`.
    fn_print_float: u32,
    /// Renders an `i64` as its decimal string in a fresh heap
    /// allocation — the value half of `String(Int)` / `Int.String()`
    /// (conversion-is-construction, the language spec (docs/src/spec/)). Same
    /// digit loop as `build_print_int` but the bytes are copied out of
    /// the shared int buffer into an `$alloc` block so later renders
    /// can't clobber the result. Core signature: `(i64) -> (i32, i32)`.
    fn_int_to_str: u32,
    /// Byte-wise lexicographic string compare returning -1/0/1 —
    /// backs `String.lt/le/gt/ge/ne` (and the alphabetical-order rule
    /// the language is built on). Core signature:
    /// `(ptr1, len1, ptr2, len2) -> i32`.
    fn_str_cmp: u32,
    /// `(list_ptr, count, slot: i64) -> (ptr, count)` — fresh list
    /// with `slot` appended. The call site packs the element into the
    /// 8-byte slot (i64 as-is; strings as `ptr | len << 32`, matching
    /// the `build_list_literal` layout).
    fn_list_append: u32,
    /// `(ptr1, count1, ptr2, count2) -> (ptr, count)` — fresh list
    /// with the second list's slots after the first's.
    fn_list_concat: u32,
    fn_user_start: u32,
    /// `Some("Result")` / `Some("Option")` while compiling the body
    /// of a function whose declared return type is that shape (one
    /// i32 pointer at the core level). Gates `?`'s early return: an
    /// `Err`/`None` propagates unchanged when the inner and enclosing
    /// kinds match (both tag 0 at offset 0); in any other context
    /// (e.g. `main`), `?` extracts unconditionally as before.
    cur_fn_early_return: Option<&'static str>,
    /// HTTP encoder mode: the module is self-contained (own memory,
    /// own bump global, exported `cabi_realloc`), imports follow
    /// `wit-component` naming conventions, and the entry export is
    /// `wasi:http/handler@…#handle` instead of `run`. Constructors
    /// `Headers()` / `Response(…)` compile to `wasi:http/types` calls.
    http_mode: bool,
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
        let base_defined = base_waitable + 7; // skip the 7 waitable+task imports
                                              // (set-new, join, set-wait,
                                              //  set-drop, subtask-drop,
                                              //  task-return, subtask-cancel)
        WasmGen {
            ast,
            strings: StringTable::new(),
            type_defs: HashMap::new(),
            union_variants: HashMap::new(),
            variant_parent: HashMap::new(),
            variant_tag: HashMap::new(),
            func_table: HashMap::new(),
            compiled_user_funcs: Vec::new(),
            user_type_sigs: Vec::new(),
            user_type_map: HashMap::new(),

            extern_imports,
            fn_waitable_set_new: base_waitable,
            fn_waitable_join: base_waitable + 1,
            fn_waitable_set_wait: base_waitable + 2,
            fn_waitable_set_drop: base_waitable + 3,
            fn_subtask_drop: base_waitable + 4,
            fn_task_return: base_waitable + 5,
            fn_subtask_cancel: base_waitable + 6,
            fn_print_str: base_defined,
            fn_print_int: base_defined + 1,
            fn_print_bool: base_defined + 2,
            fn_alloc: base_defined + 3,
            fn_start: base_defined + 4,
            fn_list_to_json_array: base_defined + 5,
            fn_print_float: base_defined + 6,
            fn_int_to_str: base_defined + 7,
            fn_str_cmp: base_defined + 8,
            fn_list_append: base_defined + 9,
            fn_list_concat: base_defined + 10,
            fn_user_start: base_defined + 11,
            cur_fn_early_return: None,
            http_mode: false,
        }
    }

    /// Constructor for HTTP encoder mode. The `wasi:http/types`
    /// binding declarations from `canon/std/http` are consumed by the
    /// mode's own constructor special-cases, not the generic extern
    /// machinery, so they're filtered out here. Any *other* extern
    /// import can't be satisfied by the `wasi:http/service` world and
    /// is a hard error at this stage of the migration.
    fn new_http(ast: &'m OModule) -> Self {
        let mut gen = Self::new(ast);
        gen.http_mode = true;
        gen.extern_imports.retain(|ext| {
            !ext.component_namespace.starts_with("wasi:http/types")
                && !ext
                    .component_namespace
                    .starts_with("canon:builtins/concurrent")
        });
        if !gen.extern_imports.is_empty() {
            let names: Vec<String> = gen
                .extern_imports
                .iter()
                .map(|e| e.full_path.clone())
                .collect();
            eprintln!(
                "error: HTTP handler programs can only import `wasi:http/types` for now: \
                 found extern imports the `wasi:http/service` world can't satisfy: {}. \
                 (Lifting the remaining WASI surface into HTTP handlers is not yet implemented.)",
                names.join(", ")
            );
            std::process::exit(1);
        }
        // No extern or waitable imports in this mode: defined functions
        // start right after the fixed import block. Poison the waitable
        // indices so an accidental call fails validation loudly instead
        // of silently calling the wrong import.
        gen.fn_print_str = HTTP_BASE_DEFINED;
        gen.fn_print_int = HTTP_BASE_DEFINED + 1;
        gen.fn_print_bool = HTTP_BASE_DEFINED + 2;
        gen.fn_alloc = HTTP_BASE_DEFINED + 3;
        gen.fn_start = HTTP_BASE_DEFINED + 4; // the `handle` wrapper slot
        gen.fn_list_to_json_array = HTTP_BASE_DEFINED + 5;
        gen.fn_print_float = HTTP_BASE_DEFINED + 6;
        gen.fn_int_to_str = HTTP_BASE_DEFINED + 7;
        gen.fn_str_cmp = HTTP_BASE_DEFINED + 8;
        gen.fn_list_append = HTTP_BASE_DEFINED + 9;
        gen.fn_list_concat = HTTP_BASE_DEFINED + 10;
        gen.fn_user_start = HTTP_BASE_DEFINED + 11;
        gen.fn_waitable_set_new = u32::MAX;
        gen.fn_waitable_join = u32::MAX;
        gen.fn_waitable_set_wait = u32::MAX;
        gen.fn_waitable_set_drop = u32::MAX;
        gen.fn_subtask_drop = u32::MAX;
        gen.fn_task_return = u32::MAX;
        gen.fn_subtask_cancel = u32::MAX;
        gen
    }

    /// Constructor for web encoder mode (the web target, docs/src/reference/web-target.md). The browser
    /// host implements only the stdout print stubs, so any extern
    /// import is a hard error at this stage.
    fn new_web(ast: &'m OModule) -> Self {
        let mut gen = Self::new(ast);
        gen.extern_imports.retain(|ext| {
            !ext.component_namespace
                .starts_with("canon:builtins/concurrent")
        });
        if !gen.extern_imports.is_empty() {
            let names: Vec<String> = gen
                .extern_imports
                .iter()
                .map(|e| e.full_path.clone())
                .collect();
            eprintln!(
                "error: web-app programs can't use extern imports yet: the browser host \
                 implements only the print surface. Found: {}. (Extending the web host's \
                 import surface is not yet implemented.)",
                names.join(", ")
            );
            std::process::exit(1);
        }
        // Defined functions start right after the five stdout imports.
        // The three export wrappers (init/update/view) take the slots
        // after `alloc`; `fn_start` doubles as the init-wrapper index.
        gen.fn_print_str = WEB_BASE_DEFINED;
        gen.fn_print_int = WEB_BASE_DEFINED + 1;
        gen.fn_print_bool = WEB_BASE_DEFINED + 2;
        gen.fn_alloc = WEB_BASE_DEFINED + 3;
        gen.fn_start = WEB_BASE_DEFINED + 4; // init wrapper; update/view at +5/+6
        gen.fn_list_to_json_array = WEB_BASE_DEFINED + 7;
        gen.fn_print_float = WEB_BASE_DEFINED + 8;
        gen.fn_int_to_str = WEB_BASE_DEFINED + 9;
        gen.fn_str_cmp = WEB_BASE_DEFINED + 10;
        gen.fn_list_append = WEB_BASE_DEFINED + 11;
        gen.fn_list_concat = WEB_BASE_DEFINED + 12;
        gen.fn_user_start = WEB_BASE_DEFINED + 13;
        gen.fn_waitable_set_new = u32::MAX;
        gen.fn_waitable_join = u32::MAX;
        gen.fn_waitable_set_wait = u32::MAX;
        gen.fn_waitable_set_drop = u32::MAX;
        gen.fn_subtask_drop = u32::MAX;
        gen.fn_task_return = u32::MAX;
        gen.fn_subtask_cancel = u32::MAX;
        gen
    }

    /// Build the `fn_list_to_json_array` helper function.
    ///
    /// Core signature: `(list_ptr: i32, list_len: i32) -> (i32, i32)`
    /// returning `(out_ptr, out_len)` of a freshly-allocated string
    /// containing `[elem0,elem1,…,elemN]`. Element slots in the list
    /// follow the storage convention of `build_list_literal`:
    /// `(i32 ptr, i32 len)` at offsets 0 / 4 of an 8-byte slot.
    ///
    /// Algorithm: two passes. Pass 1 sums the byte budget
    /// (`2 + sum(elem_len) + max(0, len-1)`), so we allocate the
    /// output buffer exactly once. Pass 2 fills it by walking the
    /// list, writing `[`, comma separators, each element body, and
    /// finally `]`.
    fn build_list_to_json_array(&self) -> Function {
        // Locals declared in order. Indices follow the params (2 i32s),
        // so the first local is index 2.
        //   0: list_ptr  (param)
        //   1: list_len  (param)
        //   2: total     (output size accumulator / final length)
        //   3: i         (loop counter)
        //   4: out_ptr   (allocated buffer)
        //   5: out_pos   (write offset within buffer)
        //   6: elem_ptr  (per-iteration element pointer)
        //   7: elem_len  (per-iteration element length)
        //   8: slot_addr (list_ptr + i*8, reused twice per iteration)
        let mut f = Function::new([(7, ValType::I32)]);

        // ── Pass 1: total = 2 + sum(elem_len) + max(0, len-1) ─────────────────
        // Start total with 2 (for `[` and `]`).
        f.instruction(&Instruction::I32Const(2));
        f.instruction(&Instruction::LocalSet(2));
        // If len > 1, add (len - 1) for the commas.
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32GtS);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::End);
        // Loop: i = 0; while i < len: total += elem_len[i]; i++
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        // if i >= len: break
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32GeS);
        f.instruction(&Instruction::BrIf(1));
        // slot_addr = list_ptr + i*8
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalTee(8));
        // total += i32.load offset=4 (slot_addr) = elem_len
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 4,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(2));
        // i++
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End); // end loop
        f.instruction(&Instruction::End); // end block

        // ── Allocate output buffer (size = total) ──────────────────────────────
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(4));

        // Write `[` at out_ptr+0
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(b'[' as i32));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        // out_pos = 1
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(5));

        // ── Pass 2: walk elements, write to buffer ─────────────────────────
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        // if i >= len: break
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32GeS);
        f.instruction(&Instruction::BrIf(1));
        // if i > 0: write `,` at out_ptr+out_pos, out_pos++
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32GtS);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(b',' as i32));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::End);
        // slot_addr = list_ptr + i*8
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(8));
        // elem_ptr = i32.load(slot_addr+0)
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalSet(6));
        // elem_len = i32.load(slot_addr+4)
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 4,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalSet(7));
        // Inline byte-copy loop: copy elem_len bytes from elem_ptr to
        // out_ptr+out_pos. We use local 6 (elem_ptr) as src cursor,
        // local 8 as dst cursor (= out_ptr+out_pos initially), local 7
        // as remaining count.
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        // *dst = *src
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::LocalGet(6));
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
        // dst++, src++, n--
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End); // end inner loop
        f.instruction(&Instruction::End); // end inner block
                                          // out_pos += original elem_len (re-load from slot+4)
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 4,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));
        // i++
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End); // end outer loop
        f.instruction(&Instruction::End); // end outer block

        // Write `]` at out_ptr+out_pos
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(b']' as i32));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));

        // Return (out_ptr, total). `total` was the pass-1 budget,
        // which equals the final length — we wrote exactly that many
        // bytes.
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::End);
        f
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

        // Newtype aliases of unions inherit their variant set so that
        // dispatching on a value whose static type is the alias resolves
        // correctly. E.g. `MessageContent = Option<Content>` is
        // registered with the same variants as `Option`, letting
        // `someMessageContent.(None => ..., Some<Content> => ...)`
        // compile through the same `emit_union_dispatch` path as a raw
        // `Option`.
        //
        // The alias chain is walked through `type_defs` until we hit a
        // name that's already in `union_variants` (either a user union
        // or a builtin like `Option`/`Result`/`Bool`). The bound of 20
        // hops guards against a malformed cyclic alias.
        let alias_defs: Vec<(String, String)> = self
            .type_defs
            .iter()
            .filter_map(|(alias_name, body)| {
                if let TypeExpr::Named { name: target, .. } = body {
                    Some((alias_name.clone(), target.clone()))
                } else {
                    None
                }
            })
            .collect();
        for (alias_name, initial_target) in alias_defs {
            if self.union_variants.contains_key(&alias_name) {
                continue; // already a union itself, not an alias
            }
            let mut current = initial_target;
            for _ in 0..20 {
                if let Some(variants) = self.union_variants.get(&current) {
                    let variants = variants.clone();
                    self.union_variants.insert(alias_name.clone(), variants);
                    break;
                }
                match self.type_defs.get(&current) {
                    Some(TypeExpr::Named { name: next, .. }) => current = next.clone(),
                    _ => break,
                }
            }
        }
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
            Expr::JsonLit { parts, .. } => {
                // Intern every Static fragment so the `compile_expr`
                // path — which lowers a mixed JsonLit to a concat
                // chain over synthesized `StringLit`s — finds each
                // fragment in the intern table. Recurse into Interp
                // expressions so their strings are also interned.
                for p in parts {
                    match p {
                        crate::ast::JsonLitPart::Static(s) => {
                            self.strings.intern(s);
                        }
                        crate::ast::JsonLitPart::Interp(e) => self.collect_strings_expr(e),
                    }
                }
            }
            Expr::HtmlLit { parts, .. } => {
                // Same as JsonLit: pre-intern Static fragments for the
                // concat-chain lowering, recurse into interpolations.
                for p in parts {
                    match p {
                        crate::ast::HtmlLitPart::Static(s) => {
                            self.strings.intern(s);
                        }
                        crate::ast::HtmlLitPart::Interp(e) => self.collect_strings_expr(e),
                    }
                }
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
            // Canon return type — needed for proper method dispatch.
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
            // The Canon-side result type depends on the indirect-return
            // shape: a bare `String` return is `Ty::Str`, while a
            // `Result<Ok, Err>` (both string-aliased) becomes
            // `Ty::NamedPtrStr("Result", ok_name, err_name)` so `?` and
            // dispatch arms can extract the string payload with the right
            // Canon-level type on either branch.
            let surface_result_ty = match &ext.indirect_return {
                Some(IndirectReturnShape::String) => {
                    // Preserve any String-alias name (e.g. `HttpServer`,
                    // `Now`, `Url`) so subsequent method dispatch finds
                    // the right key. `resolve_return_ty` already wraps
                    // String-aliased types as `Ty::NamedStr(name)`.
                    match &result_ty {
                        Ty::NamedStr(_) => result_ty.clone(),
                        _ => Ty::Str,
                    }
                }
                Some(IndirectReturnShape::ResultStringString { ok_name, err_name }) => {
                    Ty::NamedPtrStr("Result".to_string(), ok_name.clone(), err_name.clone())
                }
                Some(IndirectReturnShape::OptionString) => Ty::NamedPtrStr(
                    "Option".to_string(),
                    "String".to_string(),
                    "String".to_string(),
                ),
                Some(IndirectReturnShape::ListString) => Ty::List,
                Some(IndirectReturnShape::ScalarRecord { product, .. }) => {
                    Ty::NamedPtr(product.clone())
                }
                None => result_ty,
            };
            let info = FuncInfo {
                func_idx: ext.func_idx,
                type_idx,
                result_ty: surface_result_ty,
                narrow_params: ext.narrow_params.clone(),
                narrow_result_signed: ext.narrow_result_signed,
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
                // Surface result type: classify `Result<String-aliased,
                // String-aliased>` returns the same way as externs so
                // `?` and dispatch arms can extract string payloads via
                // the `Ty::NamedPtrStr` path. The function body itself
                // returns an i32 pointer (via `build_result_ok` /
                // `build_result_err`) whose memory layout matches the
                // extern indirect-return area (tag at +0, ptr at +4,
                // len at +8), so no calling-convention change is needed
                // — only the type label.
                let result_ty = match classify_return(&func.return_ty, &results, &self.type_defs) {
                    Some(IndirectReturnShape::ResultStringString { ok_name, err_name }) => {
                        Ty::NamedPtrStr("Result".to_string(), ok_name, err_name)
                    }
                    // `Option<String-alias>` bodies keep the payload's
                    // string-ness in the surface type so `?` extracts a
                    // (ptr, len) pair instead of misreading the slot as
                    // one i64. Dispatch is unaffected — it keys on the
                    // container name, exactly like the Result case.
                    Some(IndirectReturnShape::OptionString) => {
                        let payload = match &func.return_ty {
                            TypeExpr::Named { generics, .. } if !generics.is_empty() => {
                                named_type_name(&generics[0])
                                    .unwrap_or_else(|| "String".to_string())
                            }
                            _ => "String".to_string(),
                        };
                        Ty::NamedPtrStr("Option".to_string(), payload.clone(), payload)
                    }
                    _ => self.resolve_return_ty(func),
                };

                let key = (
                    func.receiver.as_ref().map(|r| r.name.clone()),
                    func.name.name.clone(),
                );
                let info = FuncInfo {
                    func_idx: idx,
                    type_idx,
                    result_ty,
                    narrow_params: Vec::new(),
                    narrow_result_signed: None,
                    indirect_return: None,
                    is_async: false,
                };
                if is_self_ctor(func) {
                    // Constructor families: several `Self`-renamed bodies
                    // share the `(Type, "Self")` primary key. The zero-arg
                    // member owns it (it's what a bare `Type()` call
                    // dispatches through); parameterized members are
                    // reached via the per-param commutative keys below,
                    // so they only fill the primary slot when nothing
                    // else has.
                    if func.params.is_empty() {
                        self.func_table.insert(key, info.clone());
                    } else {
                        self.func_table.entry(key).or_insert_with(|| info.clone());
                    }
                } else {
                    self.func_table.insert(key, info.clone());
                }

                // Self-ctor commutative registration (mirrors the extern
                // block above). After `resolve_new_syntax`, a function
                // declared as `Name = (P) -> R<Name, E>` is rewritten to
                // `Self = (P) -> ...` with receiver `Name`. We also need
                // to make it reachable from `p.Name()` — the natural
                // method-call form on a value of type `P` — by adding
                // an alias entry under `(Some(P), Name)`. Without this,
                // body-defined validating constructors (`Json = (String)
                // -> Result<Json, MalformedJson> { … }`) fall through
                // to the type-newtype constructor path in
                // `compile_constructor`, silently dropping the body.
                // Every param component registers (not just the first):
                // commutative calling lets any component be the
                // receiver, and for a constructor family the per-param
                // keys are what keep members distinct — the checker
                // guards that no two members collide on a component
                // type.
                if is_self_ctor(func) {
                    let recv_name = func
                        .receiver
                        .as_ref()
                        .map(|r| r.name.clone())
                        .unwrap_or_default();
                    let mut components: Vec<String> = Vec::new();
                    for param in &func.params {
                        match &param.ty {
                            TypeExpr::Named {
                                name: param_name, ..
                            } => components.push(param_name.clone()),
                            TypeExpr::Product { fields, .. } => {
                                for field in fields {
                                    if let TypeExpr::Named {
                                        name: field_name, ..
                                    } = field
                                    {
                                        components.push(field_name.clone());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    for param_name in components {
                        let commutative_key = (Some(param_name), recv_name.clone());
                        self.func_table
                            .entry(commutative_key)
                            .or_insert_with(|| info.clone());
                    }
                }
                self.compiled_user_funcs.push((idx, type_idx, func.clone()));
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
            "Bool" | "True" | "False" => Ty::I32,
            "String" => Ty::Str,
            "Unit" | "Never" => Ty::Unit,
            // Canonical-ABI resource handle — one i32. Newtypes over it
            // (`Request = Handle`, `Response = Handle`) wrap into
            // `Ty::NamedPtr(name)` via the alias arm below, which keeps
            // the type name for method dispatch.
            "Handle" => Ty::Ptr,
            // See `resolve_name_val_types::go` for the rationale on which
            // names belong here — only true ambient-effect capabilities,
            // not value types like `HttpServer<S>`.
            "Stdout" | "Stderr" | "Stdin" | "Network" | "Clock" | "Filesystem" => Ty::Unit,
            // `Map` / `Set` are NOT here — they are pure-Canon stdlib
            // unions whose repr resolves through `type_defs` below.
            "List" => Ty::List,
            "Option" | "Result" => Ty::NamedPtr(name.to_string()),
            _ => {
                if let Some(body) = self.type_defs.get(name).cloned() {
                    match &body {
                        TypeExpr::Named { name: inner, .. } => {
                            // Newtype alias — generic args on the RHS don't
                            // change the repr (`Keys = List<Key>` is a list
                            // at the value level; the element type is a
                            // checker-side fact).
                            let inner_repr = self.resolve_repr_depth(inner, depth + 1);
                            // Wrap with the outer name for method dispatch
                            match inner_repr {
                                Ty::I64 | Ty::F64 | Ty::I32 | Ty::Unit => inner_repr,
                                Ty::Str | Ty::NamedStr(_) => Ty::NamedStr(name.to_string()),
                                Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) | Ty::Ptr => {
                                    Ty::NamedPtr(name.to_string())
                                }
                                // Keep the two-slot list repr — wrapping it
                                // in a one-slot `NamedPtr` desynchronizes
                                // the wasm function type from the body
                                // ("values remaining on stack"). Like
                                // scalar newtypes, list newtypes erase to
                                // `List` at the value level.
                                Ty::List => Ty::List,
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

    /// Build the `fn_int_to_str` helper: `(i64) -> (i32, i32)`.
    ///
    /// Renders the value's decimal digits into the shared int buffer
    /// (same backward digit loop as `build_print_int`, minus the
    /// trailing-newline convention), then copies them into a fresh
    /// `$alloc` block so the result survives later renders. Returns
    /// the `(ptr, len)` pair of the copy.
    fn build_int_to_str(&self) -> Function {
        let store8 = MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        };
        let load8 = store8;
        // Locals: 0 = value (param, i64); 1 = ptr; 2 = is_neg;
        // 3 = digit; 4 = len; 5 = dst; 6 = out_ptr.
        let mut f = Function::new([(6, ValType::I32)]);
        f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32));
        f.instruction(&Instruction::LocalSet(1));
        // Negative? Remember the sign, work on the magnitude. (i64::MIN
        // survives this: `0 - i64::MIN` wraps back to itself and the
        // unsigned digit loop below reads it as the correct magnitude.)
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
        // Zero short-circuits to a single '0' digit.
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::I32Store8(store8));
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
        f.instruction(&Instruction::I32Store8(store8));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        // Leading '-' for negatives.
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(b'-' as i32));
        f.instruction(&Instruction::I32Store8(store8));
        f.instruction(&Instruction::End);
        // len = MEM_INT_BUF_END - ptr (the '\n' at END is print_int's
        // convention, not part of the rendered value).
        f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(4));
        // out_ptr = alloc(len); dst = out_ptr.
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalTee(6));
        f.instruction(&Instruction::LocalSet(5));
        // Copy loop: while ptr < MEM_INT_BUF_END { *dst++ = *ptr++ }.
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Load8U(load8));
        f.instruction(&Instruction::I32Store8(store8));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        // Return (out_ptr, len).
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::End);
        f
    }

    /// Build the `fn_str_cmp` helper: `(ptr1, len1, ptr2, len2) -> i32`
    /// returning -1 / 0 / 1 — byte-wise lexicographic order, with the
    /// shorter string ordering first on a shared prefix. Backs the
    /// `String.lt/le/gt/ge/ne` builtins (and, transitively, the
    /// alphabetical-order rule the language enforces everywhere else).
    fn build_str_cmp(&self) -> Function {
        let load8 = MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        };
        // Locals: 0..3 = params (ptr1, len1, ptr2, len2);
        // 4 = i; 5 = b1; 6 = b2; 7 = minlen.
        let mut f = Function::new([(4, ValType::I32)]);
        // minlen = min(len1, len2)
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32LtU);
        f.instruction(&Instruction::Select);
        f.instruction(&Instruction::LocalSet(7));
        // for i in 0..minlen: compare bytes, early-return on mismatch.
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(load8));
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(load8));
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32LtU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(-1));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32GtU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(4));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        // Shared prefix — order by length: len1 < len2 → -1,
        // len1 > len2 → 1, equal → 0.
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32LtU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(-1));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32GtU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::End);
        f
    }

    /// Build the `fn_list_append` helper:
    /// `(list_ptr, count, slot: i64) -> (ptr, count)` — fresh list with
    /// `slot` in the last position. The call site packs the element
    /// (i64 verbatim; strings as `ptr | len << 32`, the
    /// `build_list_literal` slot layout).
    fn build_list_append(&self) -> Function {
        let mem64 = MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        };
        // Locals: 0 = ptr, 1 = count, 2 = slot (i64); 3 = new_ptr, 4 = j.
        let mut f = Function::new([(2, ValType::I32)]);
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I64Load(mem64));
        f.instruction(&Instruction::I64Store(mem64));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(4));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Store(mem64));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::End);
        f
    }

    /// Build the `fn_list_concat` helper:
    /// `(ptr1, count1, ptr2, count2) -> (ptr, count)`.
    fn build_list_concat(&self) -> Function {
        let mem64 = MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        };
        // Locals: 0..3 = params; 4 = new_ptr, 5 = j.
        let mut f = Function::new([(2, ValType::I32)]);
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(4));
        // First list.
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I64Load(mem64));
        f.instruction(&Instruction::I64Store(mem64));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        // Second list: j runs 0..count2, dst index = count1 + j.
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I64Load(mem64));
        f.instruction(&Instruction::I64Store(mem64));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Add);
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
        // Trailing newline (the shared '\n' byte), same as every other
        // `.print` path — one call, one line.
        f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::Call(self.fn_print_str));
        f.instruction(&Instruction::End);
        f
    }

    /// Formats an `f64` into the shared int buffer and prints it.
    ///
    /// Output format: fixed-point with up to 6 fraction digits,
    /// trailing zeros trimmed; whole values print with no fraction
    /// (`2.0` → `2`); specials print as `NaN`, `Inf`, `-Inf`. Values
    /// whose integer part exceeds `u64::MAX` saturate (the buffer is
    /// sized for 20 integer digits). This is a pragmatic decimal
    /// rendering, not shortest-round-trip dtoa — good enough until a
    /// proper Grisu/Ryū port becomes worth the code size.
    ///
    /// Locals: 0 = value (param, f64), 1 = int_part (i64),
    /// 2 = frac (i64), 3 = ptr, 4 = digit, 5 = neg, 6 = started,
    /// 7 = counter (i32).
    fn build_print_float(&self) -> Function {
        const PTR: u32 = 3;
        let mut f = Function::new([(2, ValType::I64), (5, ValType::I32)]);
        let store8 = MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        };
        // Helper closure: ptr -= 1; mem[ptr] = byte.
        let push_byte = |f: &mut Function, byte: u8| {
            f.instruction(&Instruction::LocalGet(PTR));
            f.instruction(&Instruction::I32Const(1));
            f.instruction(&Instruction::I32Sub);
            f.instruction(&Instruction::LocalSet(PTR));
            f.instruction(&Instruction::LocalGet(PTR));
            f.instruction(&Instruction::I32Const(byte as i32));
            f.instruction(&Instruction::I32Store8(store8));
        };
        // Helper closure: print buffer [ptr, MEM_INT_BUF_END] (the byte
        // at MEM_INT_BUF_END is the shared '\n').
        let flush = |f: &mut Function, print_str: u32| {
            f.instruction(&Instruction::LocalGet(PTR));
            f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32 + 1));
            f.instruction(&Instruction::LocalGet(PTR));
            f.instruction(&Instruction::I32Sub);
            f.instruction(&Instruction::Call(print_str));
        };

        // ptr = MEM_INT_BUF_END
        f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32));
        f.instruction(&Instruction::LocalSet(PTR));

        // NaN: value != value.
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Ne);
        f.instruction(&Instruction::If(BlockType::Empty));
        push_byte(&mut f, b'N');
        push_byte(&mut f, b'a');
        push_byte(&mut f, b'N');
        flush(&mut f, self.fn_print_str);
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);

        // neg = value < 0; value = |value|
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Const(0.0.into()));
        f.instruction(&Instruction::F64Lt);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Neg);
        f.instruction(&Instruction::LocalSet(0));
        f.instruction(&Instruction::End);

        // Inf.
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Const(f64::INFINITY.into()));
        f.instruction(&Instruction::F64Eq);
        f.instruction(&Instruction::If(BlockType::Empty));
        push_byte(&mut f, b'f');
        push_byte(&mut f, b'n');
        push_byte(&mut f, b'I');
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::If(BlockType::Empty));
        push_byte(&mut f, b'-');
        f.instruction(&Instruction::End);
        flush(&mut f, self.fn_print_str);
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);

        // int_part = trunc_sat(value)
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64TruncSatF64U);
        f.instruction(&Instruction::LocalSet(1));
        // frac = trunc_sat((value - int_part) * 1e6 + 0.5)
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::F64ConvertI64U);
        f.instruction(&Instruction::F64Sub);
        f.instruction(&Instruction::F64Const(1e6.into()));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Const(0.5.into()));
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::I64TruncSatF64U);
        f.instruction(&Instruction::LocalSet(2));
        // Rounding carry: frac == 1_000_000 → bump int_part.
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Const(1_000_000));
        f.instruction(&Instruction::I64GeU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Const(1));
        f.instruction(&Instruction::I64Add);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::End);

        // Fraction digits, least-significant first, trailing zeros
        // skipped until the first significant digit (`started`).
        f.instruction(&Instruction::I32Const(6));
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64RemU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(4));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64DivU);
        f.instruction(&Instruction::LocalSet(2));
        // Skip while nothing started and digit is zero.
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::Br(1)); // continue the loop
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::LocalGet(PTR));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(PTR));
        f.instruction(&Instruction::LocalGet(PTR));
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(store8));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // Decimal point (only when fraction digits were written).
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::If(BlockType::Empty));
        push_byte(&mut f, b'.');
        f.instruction(&Instruction::End);

        // Integer digits (same shape as `build_print_int`).
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        push_byte(&mut f, b'0');
        f.instruction(&Instruction::Else);
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64RemU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(4));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64DivU);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(PTR));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(PTR));
        f.instruction(&Instruction::LocalGet(PTR));
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(store8));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // Sign.
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::If(BlockType::Empty));
        push_byte(&mut f, b'-');
        f.instruction(&Instruction::End);

        flush(&mut f, self.fn_print_str);
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
        // locals: 1 = aligned_ptr, 2 = new bump (allocation end)
        let mut f = Function::new([(2, ValType::I32)]);
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
        f.instruction(&Instruction::LocalTee(2));
        f.instruction(&Instruction::GlobalSet(GLOBAL_BUMP_PTR));
        // Grow memory when the allocation end passes the current
        // size. Long-lived instances (web apps dispatching events,
        // HTTP handlers) outlive the initial two pages; short-lived
        // CLI runs never hit this branch. A failed grow is ignored —
        // the subsequent store traps, which is the honest failure.
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::MemorySize(0));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Shl);
        f.instruction(&Instruction::I32GtU);
        f.instruction(&Instruction::If(BlockType::Empty));
        // pages = (end - mem_bytes + 65535) >> 16
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::MemorySize(0));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Shl);
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I32Const(65535));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32ShrU);
        f.instruction(&Instruction::MemoryGrow(0));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::End);
        // return aligned_ptr
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::End);
        f
    }

    /// Builds the `run` function exported by the core module.
    ///
    /// Inlines the body of `main` (Canon's entry point), drops any value
    /// it leaves on the stack, and delivers `result::ok` via
    /// `task.return(0)`. The core signature is `() -> ()` because the
    /// component-level `run` is lifted *async stackful*: results are
    /// returned through `task.return` rather than as a wasm return value.
    /// This is also what enables `extern Wasm.async` calls inside `main`
    /// to suspend on `waitable-set.wait` — wasmtime won't let a sync
    /// task block, so `run` itself has to be async-lifted.
    fn build_start(&mut self) -> Function {
        // Locate the entry (`main`) and detect the canonical CLI shape
        // `Args => Exit`, whose single `Args` param is the argument
        // vector (`Unit => Program` and friends have no param).
        let main_func: Option<FunctionDef> = self.ast.items.iter().find_map(|item| {
            if let Item::Function(func) = item {
                if func.name.name == "main" && func.receiver.is_none() {
                    return Some(func.clone());
                }
            }
            None
        });
        let has_args = main_func
            .as_ref()
            .is_some_and(|func| crate::ast::is_args_entry_param(&func.params));

        // `Args` is `List<String>` — two i32 locals (ptr, len) laid down
        // before the scratch block. Shifting `param_count` keeps the
        // scratch-local accessors (`rptr()` = `param_count`, …) aligned
        // with the two prepended slots.
        let mut locals = extra_locals_decl();
        let mut scope = LocalScope::empty();
        if has_args {
            locals.insert(0, (2, ValType::I32)); // Args: (ptr, len)
            scope.param_count = 2;
        }
        let mut f = Function::new(locals);

        if has_args {
            // Populate the `Args` local by invoking the `Args` nullary
            // constructor (`Unit => Args { getArguments() }` in
            // `canon/std`), which reads argv via
            // `wasi:cli/environment#get-arguments` and leaves the decoded
            // `List<String>` (ptr, len) on the stack. Compiled before
            // `Args` is registered in `scope` so the constructor call
            // can't alias the local it fills.
            let call = Expr::Constructor {
                name: crate::ast::Ident {
                    name: "Args".to_string(),
                    span: crate::error::Span::default(),
                },
                args: Vec::new(),
                span: crate::error::Span::default(),
            };
            let ty = self.compile_expr(&call, &scope, &mut f);
            debug_assert!(matches!(ty, Ty::List), "Args() must produce a list");
            f.instruction(&Instruction::LocalSet(1)); // len (top of stack)
            f.instruction(&Instruction::LocalSet(0)); // ptr
            scope.vars.insert("Args".to_string(), (0, Ty::List));
            // `List` is the underlying-type alias, so a body that pipes
            // the argv through a list builtin (`Args -> At(1)`) resolves.
            scope.vars.insert("List".to_string(), (0, Ty::List));
        }

        let result_ty = main_func
            .as_ref()
            .map(|func| self.compile_block_return(&func.body, &scope, &mut f));
        // Deliver the run `result` discriminant to the component-level
        // caller via `task.return` (0 = ok, 1 = err). This must precede
        // `End` and is how the async-stackful lift signals completion.
        match result_ty {
            // `Args => Exit`: the returned `Exit` (`= Int`) is the exit
            // status. WASI `run` returns a bare `result`, which can only
            // encode success/failure — so `Exit(0)` maps to ok (exit 0)
            // and any nonzero code to err (exit 1). An exact nonzero code
            // needs the hard `Exited(n)` (`exit-with-code`) escape hatch.
            Some(Ty::I64) => {
                f.instruction(&Instruction::I64Const(0));
                f.instruction(&Instruction::I64Ne); // i32: 1 when nonzero
                f.instruction(&Instruction::Call(self.fn_task_return));
            }
            // `Unit => Program` and other Unit-world entries: nothing to
            // report — always ok.
            Some(other) => {
                self.drop_value(other, &mut f);
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::Call(self.fn_task_return));
            }
            None => {
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::Call(self.fn_task_return));
            }
        }
        f.instruction(&Instruction::End);
        f
    }

    fn build_user_function(&mut self, func: &FunctionDef) -> Function {
        let (params, scope) = self.build_local_scope(func);
        let _ = params; // params are implicit in the function type
        let mut f = Function::new(extra_locals_decl());
        let body = func.body.clone();
        // `?` may early-return the whole Result/Option value, but only
        // when the enclosing function itself returns the same shape
        // (one i32 pointer at the core level). Record which kind for
        // the duration of this body.
        let ret = self.resolve_return_ty(func);
        self.cur_fn_early_return = match &ret {
            Ty::NamedPtrStr(n, _, _) | Ty::NamedPtr(n) if n == "Result" => Some("Result"),
            Ty::NamedPtr(n) if n == "Option" => Some("Option"),
            _ => None,
        };
        let result = self.compile_block_return(&body, &scope, &mut f);
        self.cur_fn_early_return = None;
        // The function's WASM type already declares the result type;
        // the value should already be on the stack.
        let _ = result;
        f.instruction(&Instruction::End);
        f
    }

    /// Collect every name in a type's alias chain. For `Json = String`,
    /// returns `["Json", "String"]`. For a base type like `String`,
    /// returns `["String"]`. Bounded by `resolve_repr_depth`'s 20-step
    /// guard so a malformed cycle can't infinite-loop.
    fn collect_alias_chain(&self, name: &str) -> Vec<String> {
        let mut out = vec![name.to_string()];
        let mut current = name.to_string();
        for _ in 0..20 {
            let body = match self.type_defs.get(&current) {
                Some(b) => b.clone(),
                None => break,
            };
            if let TypeExpr::Named {
                name: next,
                generics,
                ..
            } = &body
            {
                if !generics.is_empty() {
                    break;
                }
                if out.iter().any(|n| n == next) {
                    break;
                }
                out.push(next.clone());
                current = next.clone();
            } else {
                break;
            }
        }
        out
    }

    /// Build LocalScope for a function's params + receiver.
    fn build_local_scope(&self, func: &FunctionDef) -> (Vec<ValType>, LocalScope) {
        let mut scope = LocalScope::default();
        let mut local_idx: u32 = 0;
        let mut params = Vec::new();

        // For Self-ctor functions (`Name = (P) -> R<Name, E>` after
        // `resolve_new_syntax`), the WASM signature omits the receiver
        // — the value lives as the first param. The receiver name is
        // a type-level handle, not a runtime value. We still register
        // it under the *param's* local index (so the body can reference
        // it by either the newtype name like `Json` or the underlying
        // type name like `String`) but we don't allocate a separate
        // slot for it.
        // Exact declared names always win; alias-chain names (the
        // newtype's underlying types) only fill slots no exact name
        // claims, receiver-first. Without this precedence, a later
        // param whose newtype erases to the same underlying type
        // clobbers an earlier exact param — in
        // `elAttr = (Attr * String * Tag)`, `Tag`'s alias registration
        // used to steal the body's `String` references.
        let mut alias_pending: Vec<(String, u32, Ty)> = Vec::new();
        let skip_receiver_slot = is_self_ctor(func);
        if let Some(recv) = &func.receiver {
            if !skip_receiver_slot {
                let repr = self.resolve_repr(&recv.name);
                let vt = repr.val_types();
                let mut chain = self.collect_alias_chain(&recv.name).into_iter();
                if let Some(exact) = chain.next() {
                    scope.vars.insert(exact, (local_idx, repr.clone()));
                }
                for alias in chain {
                    alias_pending.push((alias, local_idx, repr.clone()));
                }
                local_idx += vt.len() as u32;
                params.extend(vt);
            }
        }
        for param in &func.params {
            if let TypeExpr::Named { name, .. } = &param.ty {
                let repr = self.resolve_repr(name);
                let vt = repr.val_types();
                let mut chain = self.collect_alias_chain(name).into_iter();
                if let Some(exact) = chain.next() {
                    scope.vars.insert(exact, (local_idx, repr.clone()));
                }
                for alias in chain {
                    alias_pending.push((alias, local_idx, repr.clone()));
                }
                // For a Self-ctor, also register the receiver-type name
                // (`Json` for `Self = (String) -> ...`) as an alias of
                // the first param so `Json` inside the body refers to
                // the same value as `String`.
                if skip_receiver_slot && local_idx == 0 {
                    if let Some(recv) = &func.receiver {
                        scope
                            .vars
                            .insert(recv.name.clone(), (local_idx, repr.clone()));
                    }
                }
                local_idx += vt.len() as u32;
                params.extend(vt);
            }
        }
        for (alias, idx, repr) in alias_pending {
            scope.vars.entry(alias).or_insert((idx, repr));
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
            // the value on the stack. See the language spec
            // (docs/src/spec/) on newtypes as 1-component products.
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
                // Product field access: the receiver is a heap pointer
                // to a struct laid out by `build_product_value`. Read
                // back from the matching byte offset.
                if let Ty::NamedPtr(product_name) = &recv_ty {
                    if self
                        .type_defs
                        .get(product_name)
                        .is_some_and(|t| matches!(t, TypeExpr::Product { .. }))
                    {
                        if let Some(ty) =
                            self.load_product_field(product_name, &field.name, scope, f)
                        {
                            return ty;
                        }
                    }
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
                // `?` extracts the Ok/Some payload; when the enclosing
                // function itself returns a Result (same core shape:
                // one i32 pointer), an Err short-circuits by returning
                // the whole Result value unchanged. In non-Result
                // contexts (e.g. `main`) extraction is unconditional,
                // as before. Payload width by inner type:
                //   - `Ty::NamedPtrStr(_, _, _)` → `(i32 ptr, i32 len)` at offsets 4 and 8.
                //   - `Ty::NamedPtr("Result"|"Option")` → `i64` at offset 4 (legacy).
                match &inner_ty {
                    Ty::NamedPtrStr(container, ok_name, _) => {
                        let container = container.clone();
                        let ok_name = ok_name.clone();
                        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
                        if self.cur_fn_early_return == Some(container.as_str()) {
                            // tag == 0 (Err) → return the Result as-is.
                            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                            f.instruction(&Instruction::I32Load(MemArg {
                                offset: 0,
                                align: 2,
                                memory_index: 0,
                            }));
                            f.instruction(&Instruction::I32Eqz);
                            f.instruction(&Instruction::If(BlockType::Empty));
                            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                            f.instruction(&Instruction::Return);
                            f.instruction(&Instruction::End);
                        }
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
                        // Preserve the Canon-level type of the Ok payload
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
                        if self.cur_fn_early_return == Some(n.as_str()) {
                            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                            f.instruction(&Instruction::I32Load(MemArg {
                                offset: 0,
                                align: 2,
                                memory_index: 0,
                            }));
                            f.instruction(&Instruction::I32Eqz);
                            f.instruction(&Instruction::If(BlockType::Empty));
                            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                            f.instruction(&Instruction::Return);
                            f.instruction(&Instruction::End);
                        }
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
            // ── JSON literal ──────────────────────────────────
            //
            // All-static fast path: collapse the parts into one string
            // literal and push directly — zero runtime cost.
            //
            // Mixed (with interpolations): synthesize a left-associated
            // chain of `String.concat` calls over alternating `StringLit`
            // (Static fragments) and `MethodCall { method: "ToJson" }`
            // (Interp expressions), then compile that. This reuses the
            // existing `concat` builtin and the existing `ToJson` trait
            // dispatch so we don't need new codegen for either; the
            // surface-syntax `{"k": foo}` is purely parser sugar over
            // machinery that already exists.
            Expr::JsonLit { parts, span } => {
                let all_static = parts
                    .iter()
                    .all(|p| matches!(p, crate::ast::JsonLitPart::Static(_)));
                if all_static {
                    let mut merged = String::new();
                    for p in parts {
                        if let crate::ast::JsonLitPart::Static(s) = p {
                            merged.push_str(s);
                        }
                    }
                    let (ptr, len) = self.strings.intern(&merged);
                    f.instruction(&Instruction::I32Const(ptr as i32));
                    f.instruction(&Instruction::I32Const(len as i32));
                    Ty::Str
                } else {
                    let chain = json_lit_to_concat_chain(parts, *span);
                    self.compile_expr(&chain, scope, f)
                }
            }

            // ── HTML literal ──────────────────────────────────
            //
            // Same two-tier lowering as the JSON literal above: an
            // all-static literal collapses to one interned string; a
            // literal with interpolation holes becomes a
            // `String.concat` chain whose `Interp` links are
            // `.ToHtml()` calls (escaping for `String`/`Int` via the
            // stdlib's `text()`, identity for `Html` — see
            // `packages/canon/std/src/web/html.can`).
            Expr::HtmlLit { parts, span } => {
                let all_static = parts
                    .iter()
                    .all(|p| matches!(p, crate::ast::HtmlLitPart::Static(_)));
                if all_static {
                    let mut merged = String::new();
                    for p in parts {
                        if let crate::ast::HtmlLitPart::Static(s) = p {
                            merged.push_str(s);
                        }
                    }
                    let (ptr, len) = self.strings.intern(&merged);
                    f.instruction(&Instruction::I32Const(ptr as i32));
                    f.instruction(&Instruction::I32Const(len as i32));
                    Ty::Str
                } else {
                    let chain = html_lit_to_concat_chain(parts, *span);
                    self.compile_expr(&chain, scope, f)
                }
            }

            // ── Await (checker-inserted, Phase 5) ─────────────────────────────
            Expr::Await { inner, .. } => self.compile_expr(inner, scope, f),
        }
    }

    // ── Constructor compilation ────────────────────────────────────────────────

    /// Static byte-ness test for the `String(Byte)` conversion. `Byte`
    /// erases to i64 at the value level (same repr as `Int`), so the
    /// two Int→String conversions — decimal rendering vs. single-byte
    /// string — are told apart by the *declared* type at the call
    /// site: a `Byte(…)` constructor, an identifier bound under a
    /// name whose alias chain passes through `Byte`, or a field
    /// access unwrapping to `Byte`. A method chain that returns
    /// `Byte` erases before it gets here — wrap it
    /// (`Byte(x).String()`) to pick the byte reading; needing the
    /// wrap to mean the other thing is exactly why the newtype
    /// exists.
    fn expr_is_byte(&self, e: &Expr) -> bool {
        let name = match e {
            Expr::Constructor { name, .. } => &name.name,
            Expr::Ident(id) => &id.name,
            Expr::FieldAccess { field, .. } => &field.name,
            // Piped construction: `65 -> Byte` builds a `Byte` just like
            // `Byte(65)`, but scalar-newtype erasure drops the name from
            // the value, so recover it from the constructor's spelling.
            Expr::MethodCall {
                method,
                piped: true,
                ..
            } => &method.name,
            _ => return false,
        };
        self.collect_alias_chain(name).iter().any(|n| n == "Byte")
    }

    /// Converts the i64 byte value on the stack into a fresh one-byte
    /// string — the value half of `String(Byte)`. The value is masked
    /// to its low 8 bits.
    fn emit_byte_to_str(&mut self, scope: &LocalScope, f: &mut Function) -> Ty {
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::I32Const(0xFF));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalTee(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(1));
        Ty::Str
    }

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
            // ── HTTP-mode constructors ────────────────────────────────
            // `Headers()` and `Response(Headers * Status)` compile to
            // real `wasi:http/types` calls (see `compile_http`). The
            // stdlib's binding declarations for these names exist only
            // for the checker; codegen owns the calling convention.
            "Headers" if self.http_mode => {
                f.instruction(&Instruction::Call(FN_HTTP_FIELDS_CTOR));
                Ty::NamedPtr("Headers".to_string())
            }
            "Response" if self.http_mode => self.build_http_response(args, scope, f),
            // Primitive constructors. Identity when the argument
            // already has the target representation (`Int(1)`,
            // `String("x")`) — compiling it IS the construction —
            // and *conversion* when it doesn't (`String(42)` renders
            // decimal, `String(Byte(65))` is the one-byte string
            // "A"): conversion is construction, see the language spec
            // (docs/src/spec/). The zero-arg forms produce the type's
            // zero value.
            "Int" | "Float" | "String" => {
                if let Some(a) = args.first() {
                    let is_byte = name == "String" && self.expr_is_byte(a);
                    let ty = self.compile_expr(a, scope, f);
                    match (name, &ty) {
                        // Tolerate `Int(bool)` / `Float(int)` shape
                        // drift by widening rather than corrupting the
                        // stack.
                        ("Int", Ty::I32) => {
                            f.instruction(&Instruction::I64ExtendI32S);
                            Ty::I64
                        }
                        ("Float", Ty::I64) => {
                            f.instruction(&Instruction::F64ConvertI64S);
                            Ty::F64
                        }
                        ("String", Ty::I64) => {
                            if is_byte {
                                self.emit_byte_to_str(scope, f)
                            } else {
                                f.instruction(&Instruction::Call(self.fn_int_to_str));
                                Ty::Str
                            }
                        }
                        ("Int", ty) if ty.is_str_like() => {
                            // `Int("42")` — the fallible parse constructor
                            // from `canon/std/Int`. The compiled string is
                            // already on the stack, exactly where
                            // `emit_func_table_call` expects the receiver.
                            if let Some(info) = self
                                .func_table
                                .get(&(Some("String".to_string()), "Int".to_string()))
                                .cloned()
                            {
                                return self.emit_func_table_call(&info, &[], scope, f);
                            }
                            // Parser not in scope — the checker rejects
                            // this; keep the stack shape sane regardless.
                            self.drop_value(Ty::Str, f);
                            f.instruction(&Instruction::I64Const(0));
                            Ty::I64
                        }
                        _ => ty,
                    }
                } else {
                    match name {
                        "Int" => {
                            f.instruction(&Instruction::I64Const(0));
                            Ty::I64
                        }
                        "Float" => {
                            f.instruction(&Instruction::F64Const(0.0.into()));
                            Ty::F64
                        }
                        _ => {
                            let (ptr, len) = self.strings.intern("");
                            f.instruction(&Instruction::I32Const(ptr as i32));
                            f.instruction(&Instruction::I32Const(len as i32));
                            Ty::Str
                        }
                    }
                }
            }
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
            // NOTE: `Map()` / `Set()` are NOT built in — they are the
            // pure-Canon `canon/std/Map` / `canon/std/Set` recursive
            // unions, whose zero-arg `Self` constructors resolve
            // through the ordinary user-defined path below.
            // NOTE: the concurrency combinators (`parallel` / `race`) are
            // *methods* — `a.parallel(b)` — handled at the top of
            // `compile_method_call`. The checker rejects the bare call
            // form, so no Constructor arm exists for them here.
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

                // 3. Constructor declared as a method on the first arg's
                //    type — lets `Url("http://…")` dispatch to
                //    `Url = (String) -> Result<…>`, and selects the right
                //    member of a constructor *family* (`Json = (Bool) ->
                //    Json` vs `Json = (Int) -> Json`) by the argument's
                //    type. Both call shapes reach here: `Value(map, k)`
                //    (positional) and `Value(map * k)` (product value) —
                //    the product form is flattened so its first field
                //    drives the lookup and the rest ride as ordinary
                //    trailing args.
                if !args.is_empty() {
                    let flat: Vec<Expr> = if args.len() == 1 {
                        if let Expr::ProductValue { fields, .. } = &args[0] {
                            fields.clone()
                        } else {
                            args.to_vec()
                        }
                    } else {
                        args.to_vec()
                    };
                    if let Some(first_ty) = self.infer_ctor_arg_type_name(&flat[0]) {
                        // The declared param type may sit anywhere on the
                        // arg's widening chain: the exact name, the
                        // variant's parent union (`True()` fills a `Bool`
                        // param), or a newtype's underlying type.
                        let mut candidates: Vec<String> = vec![first_ty.clone()];
                        if let Some(parent) = self.variant_parent.get(&first_ty) {
                            candidates.push(parent.clone());
                        }
                        for link in self.collect_alias_chain(&first_ty) {
                            if !candidates.contains(&link) {
                                candidates.push(link);
                            }
                        }
                        for cand in candidates {
                            let key = (Some(cand), name.to_string());
                            if let Some(info) = self.func_table.get(&key).cloned() {
                                // Compile the first arg (this becomes the
                                // receiver) and dispatch with the rest.
                                let _ = self.compile_expr(&flat[0], scope, f);
                                return self.emit_func_table_call(&info, &flat[1..], scope, f);
                            }
                        }
                    }
                }

                // 4. Type-def newtype / product constructor.
                if self.type_defs.contains_key(name) {
                    let body = self.type_defs.get(name).cloned().unwrap();
                    return match &body {
                        TypeExpr::Product { .. } => {
                            // Product type. Two surface shapes reach here:
                            //   * `Name(a * b * c)` — one arg, an
                            //     `Expr::ProductValue` whose fields are
                            //     the positional field values.
                            //   * `Name(a, b, c)` — N comma-separated args
                            //     in declaration (alphabetical) order.
                            // Both route through `build_product_value`,
                            // which allocates the struct, lays each field
                            // out at its byte offset, and returns the
                            // pointer typed as `Ty::NamedPtr(name)`.
                            // Anything else (mismatched arity, an empty
                            // call) falls through to the legacy
                            // side-effect-only path so we don't regress
                            // existing programs.
                            let layout = self.product_field_layout(name);
                            if args.len() == 1 {
                                if let Expr::ProductValue { fields, .. } = &args[0].clone() {
                                    if fields.len() == layout.len() {
                                        return self.build_product_value(name, fields, scope, f);
                                    }
                                }
                            }
                            if !layout.is_empty() && args.len() == layout.len() {
                                return self.build_product_value(name, args, scope, f);
                            }
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

    /// `infer_static_type_name` extended for constructor-argument routing:
    /// also resolves bare identifiers. In Canon an identifier in expression
    /// position *is* a type name — parameters and dispatch-arm payloads are
    /// referenced by the type they bind (there are no local variables) — so
    /// the name itself is the best static type available. Kept separate from
    /// `infer_static_type_name` so the method-call and async-classification
    /// call sites keep their conservative behavior.
    /// Static result type of a builtin-vocabulary method. Comparisons
    /// yield `Bool`; the numeric operations preserve their receiver's
    /// type (`Int` stays `Int`, `Float` stays `Float`); the string and
    /// index operations yield `String` / `Int`. Returns `None` for
    /// anything not in the builtin vocabulary, so a user shape of the
    /// same name (resolved earlier via `func_table`) always wins.
    fn builtin_result_type(&self, method: &str, receiver: &Expr) -> Option<String> {
        match method {
            "Eq" | "Ne" | "Lt" | "Le" | "Gt" | "Ge" | "And" | "Or" | "Not" => {
                Some("Bool".to_string())
            }
            "Length" | "ByteAt" => Some("Int".to_string()),
            "Joined" | "Substring" | "Slice" => Some("String".to_string()),
            // List transforms preserve list-ness (`map`/`filter`/`append`).
            "Mapped" | "Filtered" | "Appended" => Some("List".to_string()),
            "Sum" | "Difference" | "Product" | "Quotient" | "Remainder" | "Minimum" | "Maximum"
            | "Negated" => self.infer_ctor_arg_type_name(receiver),
            _ => None,
        }
    }

    fn infer_ctor_arg_type_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident(ident) => Some(ident.name.clone()),
            // Newtype unwrap (`x.String`) — a PascalCase field names the
            // component's type, which *is* the value's type.
            Expr::FieldAccess { field, .. }
                if field.name.chars().next().is_some_and(char::is_uppercase) =>
            {
                Some(field.name.clone())
            }
            // A method chain's static type comes from the callee's
            // registered result type — this is what lets a pipe hang off
            // a chain (`Map().Inserted("a", "1") -> Keys`). Builtin
            // methods aren't in `func_table`, so chains ending in them
            // still return `None` and the call falls through to the
            // pre-pipe routing paths.
            Expr::MethodCall {
                receiver, method, ..
            } => {
                let recv = self.infer_ctor_arg_type_name(receiver)?;
                let mut cands: Vec<String> = vec![recv.clone()];
                if let Some(p) = self.variant_parent.get(&recv) {
                    cands.push(p.clone());
                }
                for link in self.collect_alias_chain(&recv) {
                    if !cands.contains(&link) {
                        cands.push(link);
                    }
                }
                for c in cands {
                    if let Some(info) = self.func_table.get(&(Some(c), method.name.clone())) {
                        return match &info.result_ty {
                            Ty::NamedPtr(n) | Ty::NamedStr(n) | Ty::NamedPtrStr(n, _, _) => {
                                Some(n.clone())
                            }
                            Ty::Str => Some("String".to_string()),
                            Ty::I64 => Some("Int".to_string()),
                            Ty::F64 => Some("Float".to_string()),
                            Ty::I32 => Some("Bool".to_string()),
                            Ty::List => Some("List".to_string()),
                            _ => None,
                        };
                    }
                }
                // Builtin vocabulary isn't in `func_table`; infer its
                // result type so a constructor family keyed on that type
                // still resolves through a builtin-terminated chain
                // (`Eq(5) -> TestResult`, `Sum(1) -> Digits`).
                if let Some(t) = self.builtin_result_type(&method.name, receiver) {
                    return Some(t);
                }
                // Piped construction: `X -> Foo` builds a `Foo` (a
                // variant widens to its union), so `7 -> Value` inside a
                // product binds to the `Value` field by type.
                if let Some(parent) = self.variant_parent.get(&method.name) {
                    return Some(parent.clone());
                }
                if self.type_defs.contains_key(&method.name) {
                    return Some(method.name.clone());
                }
                None
            }
            _ => self.infer_static_type_name(expr),
        }
    }

    /// Quick static inference of an expression's Canon-level type *name*,
    /// used to look up methods/constructors before compiling. Returns
    /// `Some("String")` for string literals, `Some("Int")` for ints, etc.;
    /// `None` when the static shape isn't obvious without full type checking.
    fn infer_static_type_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::StringLit { .. } | Expr::JsonLit { .. } | Expr::HtmlLit { .. } => {
                Some("String".to_string())
            }
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

    /// HTTP mode: compile `Response(Body * Headers * Status)` (or the
    /// body-less `Response(Headers * Status)`) into the
    /// `wasi:http/types` construction sequence.
    ///
    /// Construction happens in two phases because the `handle` export
    /// is async-stackful: everything that *creates* handles runs here,
    /// inside the user function, but the body/trailer *writes* are
    /// sync canonical-ABI calls that block until the host consumes
    /// them — they can only run after `task.return` has delivered the
    /// response. So this function stashes the write-phase state
    /// (contents writer + body bytes + trailers writer) at fixed
    /// memory addresses, and `build_http_handle_wrapper` performs the
    /// writes after `task.return`.
    /// Does `e` construct the HTTP component named `name` (`Headers` /
    /// `Status`)? Matched by static type where inference succeeds, and
    /// by the chain's *syntactic base* otherwise — a builder chain like
    /// `Headers().set(…)` whose `.set` returns `Unit` breaks type
    /// inference, so we walk the receivers back to the `Headers()`
    /// constructor.
    fn is_http_component(&self, e: &Expr, name: &str) -> bool {
        if let Some(t) = self.infer_ctor_arg_type_name(e) {
            if self.widening_chain(&t).iter().any(|n| n == name) {
                return true;
            }
        }
        match e {
            Expr::Constructor { name: n, .. } => n.name == name,
            Expr::MethodCall {
                receiver, method, ..
            } => method.name == name || self.is_http_component(receiver, name),
            _ => false,
        }
    }

    fn build_http_response(&mut self, args: &[Expr], scope: &LocalScope, f: &mut Function) -> Ty {
        let mem = MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        };
        // Normalise `Response(a * b * c)` (one `ProductValue`) to the
        // component list.
        let exprs: Vec<Expr> = match args {
            [Expr::ProductValue { fields, .. }] => fields.clone(),
            _ => args.to_vec(),
        };
        let has_body = exprs.len() >= 3;
        // Positionless: pick each component by its type, not its slot.
        // `Headers()` and `Status(n)` name themselves; the body is
        // whatever remains. This keeps a formatter-sorted
        // `Response(Headers() * NotFound() * Status(404))` correct even
        // though the body (`NotFound()`) no longer sits first. Falls
        // back to declaration order when a component's type can't be
        // inferred statically.
        let headers_i = exprs
            .iter()
            .position(|e| self.is_http_component(e, "Headers"));
        let status_i = exprs
            .iter()
            .position(|e| self.is_http_component(e, "Status"));
        let (body_expr, headers_expr, status_expr) = match (headers_i, status_i) {
            (Some(hi), Some(si)) => {
                let body = if has_body {
                    (0..exprs.len())
                        .find(|&i| i != hi && i != si)
                        .map(|i| &exprs[i])
                } else {
                    None
                };
                (body, Some(&exprs[hi]), Some(&exprs[si]))
            }
            _ if has_body => (exprs.first(), exprs.get(1), exprs.get(2)),
            _ => (None, exprs.first(), exprs.get(1)),
        };

        // ── Phase 1: user expressions (parked on the operand stack —
        // each may be arbitrary user code, so nothing can live in
        // scratch locals until all three are compiled). ──────────────
        if let Some(e) = body_expr {
            let ty = self.compile_expr(e, scope, f);
            if !ty.is_str_like() {
                // Wrong shape — degrade to an empty body.
                self.drop_value(ty, f);
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::I32Const(0));
            }
        }
        match headers_expr {
            Some(e) => {
                let ty = self.compile_expr(e, scope, f);
                if !matches!(ty, Ty::I32 | Ty::Ptr | Ty::NamedPtr(_)) {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::Call(FN_HTTP_FIELDS_CTOR));
                }
            }
            None => {
                f.instruction(&Instruction::Call(FN_HTTP_FIELDS_CTOR));
            }
        }
        match status_expr {
            Some(e) => {
                let ty = self.compile_expr(e, scope, f);
                if matches!(ty, Ty::I64) {
                    f.instruction(&Instruction::I32WrapI64);
                } else {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::I32Const(200));
                }
            }
            None => {
                f.instruction(&Instruction::I32Const(200));
            }
        }
        // Peel into locals — no user code from here on.
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // status
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // headers
        if has_body {
            // [bptr, blen] → fixed body slots.
            f.instruction(&Instruction::LocalSet(scope.map_elem_ptr())); // blen
            f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // bptr
            f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_PTR as i32));
            f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
            f.instruction(&Instruction::I32Store(mem));
            f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_LEN as i32));
            f.instruction(&Instruction::LocalGet(scope.map_elem_ptr()));
            f.instruction(&Instruction::I32Store(mem));
        }

        // ── Phase 2: handle creation ─────────────────────────────────
        // Trailers future — reader (low 32) goes to response.new,
        // writer (high 32) to the fixed slot for the post-return write.
        f.instruction(&Instruction::Call(FN_HTTP_FUTURE_NEW));
        f.instruction(&Instruction::LocalTee(scope.tmp_i64()));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // trailers reader
        f.instruction(&Instruction::I32Const(MEM_HTTP_TRAILERS_WRITER as i32));
        f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
        f.instruction(&Instruction::I64Const(32));
        f.instruction(&Instruction::I64ShrU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::I32Store(mem));

        // Contents stream (only with a body): reader to response.new,
        // writer to the fixed slot. Without a body the slot holds 0
        // and the wrapper skips the write.
        if has_body {
            f.instruction(&Instruction::Call(FN_HTTP_STREAM_NEW));
            f.instruction(&Instruction::LocalTee(scope.tmp_i64()));
            f.instruction(&Instruction::I32WrapI64);
            f.instruction(&Instruction::LocalSet(scope.map_elem_ptr())); // contents reader
            f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_WRITER as i32));
            f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
            f.instruction(&Instruction::I64Const(32));
            f.instruction(&Instruction::I64ShrU);
            f.instruction(&Instruction::I32WrapI64);
            f.instruction(&Instruction::I32Store(mem));
        } else {
            f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_WRITER as i32));
            f.instruction(&Instruction::I32Const(0));
            f.instruction(&Instruction::I32Store(mem));
        }

        // response.new(headers, contents, trailers-reader, ret)
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        if has_body {
            f.instruction(&Instruction::I32Const(1)); // option<stream>: some
            f.instruction(&Instruction::LocalGet(scope.map_elem_ptr()));
        } else {
            f.instruction(&Instruction::I32Const(0)); // none
            f.instruction(&Instruction::I32Const(0));
        }
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(MEM_HTTP_RET as i32));
        f.instruction(&Instruction::Call(FN_HTTP_RESPONSE_NEW));

        // Unpack tuple<response, future>; drop the transmission future.
        f.instruction(&Instruction::I32Const(MEM_HTTP_RET as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // response
        f.instruction(&Instruction::I32Const(MEM_HTTP_RET as i32 + 4));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::Call(FN_HTTP_FUTURE_DROP_READABLE));

        // Apply the status (response.new defaults to 200); the bare
        // `result` discriminant is dropped — a rejected code leaves
        // the default, which is the sane degradation.
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::Call(FN_HTTP_SET_STATUS));
        f.instruction(&Instruction::Drop);

        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        Ty::NamedPtr("Response".to_string())
    }

    /// HTTP mode: the core export behind
    /// `[async-lift-stackful]wasi:http/handler@…#handle`. Core
    /// signature `(request: i32) -> ()` — the result is delivered via
    /// `[task-return]handle` mid-function, after which the task keeps
    /// running to perform the blocking body/trailer writes:
    ///
    ///   1. call the user's `(Request) -> Response` function,
    ///   2. drop the request handle (introspection is slice 2),
    ///   3. `task.return(ok(response))` — the host starts sending,
    ///   4. sync `stream.write` of the body bytes (blocks until the
    ///      host consumes), then `stream.drop-writable` (ends the
    ///      body),
    ///   5. sync `future.write` of `ok(none)` trailers, then
    ///      `future.drop-writable`.
    ///
    /// Step 4/5 state comes from the fixed memory slots
    /// `build_http_response` filled. No handles leak.
    fn build_http_handle_wrapper(&self, user_fn_idx: u32) -> Function {
        let mem = MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        };
        let mut f = Function::new([(1, ValType::I32)]); // local 1: response
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::Call(user_fn_idx));
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::Call(FN_HTTP_REQUEST_DROP));

        // task.return(ok(response)) — `result<own<response>, error-code>`
        // lowered flat as the joined slots of both arms; the ok arm
        // uses (disc = 0, handle) and pads the six error-code slots.
        f.instruction(&Instruction::I32Const(0)); // ok
        f.instruction(&Instruction::LocalGet(1)); // response handle
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::Call(FN_HTTP_TASK_RETURN));

        // ── Post-return: body ────────────────────────────────────────
        f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_WRITER as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_WRITER as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_PTR as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_LEN as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::Call(FN_HTTP_STREAM_WRITE));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_WRITER as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::Call(FN_HTTP_STREAM_DROP_WRITABLE));
        f.instruction(&Instruction::End);

        // ── Post-return: trailers (`ok(none)` — zero bytes) ──────────
        f.instruction(&Instruction::I32Const(MEM_HTTP_TRAILERS_WRITER as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::I32Const(MEM_HTTP_TRAILERS_ZERO as i32));
        f.instruction(&Instruction::Call(FN_HTTP_FUTURE_WRITE));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::I32Const(MEM_HTTP_TRAILERS_WRITER as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::Call(FN_HTTP_FUTURE_DROP_WRITABLE));

        f.instruction(&Instruction::End);
        f
    }

    /// HTTP mode: the module owns its allocator, so `cabi_realloc` is
    /// defined here (same bump global `$alloc` uses) instead of living
    /// in the component wrapper's memory-provider module.
    /// `old_ptr`/`old_size` are ignored — one-pass bump, never frees.
    fn build_cabi_realloc(&self) -> Function {
        let mut f = Function::new([(1, ValType::I32)]); // local 4: aligned
                                                        // aligned = (bump + align - 1) & -align
        f.instruction(&Instruction::GlobalGet(0));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::LocalTee(4));
        // bump = aligned + new_size
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::GlobalSet(0));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::End);
        f
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

        // ── Auto-boxed product payloads (the language spec, docs/src/spec/) ──
        //
        // A variant whose typedef is a multi-field product (`Link =
        // Label * Next` inside `Chain = Link + Stop`) stores ONE
        // pointer to a standalone product struct, not inline fields.
        // `build_product_value` already handles any field count and
        // arbitrarily nested constructors (including recursive
        // same-union values) via its operand-stack discipline, and the
        // indirection is exactly what makes recursive types finite.
        // The arm side reads the pointer back in `bind_arm_payload`'s
        // `NamedPtr` case, so field access on the bound name goes
        // through the ordinary `product_field_layout` offsets.
        if layout.len() >= 2 {
            let fields: Vec<Expr> = match args {
                [Expr::ProductValue { fields, .. }] => fields.clone(),
                _ => args.to_vec(),
            };
            self.build_product_value(variant_name, &fields, scope, f);
            // [product_ptr] — park it while the union struct allocates
            // (nothing below compiles user code, so tmp_i32 is safe).
            f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
            f.instruction(&Instruction::I32Const(total_size as i32));
            f.instruction(&Instruction::Call(self.fn_alloc));
            f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
            f.instruction(&Instruction::I32Const(tag as i32));
            f.instruction(&Instruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
            f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
            f.instruction(&Instruction::I32Store(MemArg {
                offset: payload_start as u64,
                align: 2,
                memory_index: 0,
            }));
            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
            return Ty::NamedPtr(union_name.to_string());
        }

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
            F64_0,
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
                                Ty::I64 => {
                                    f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                                    saved.push(SavedField::I64_0);
                                    i64_count += 1;
                                }
                                Ty::F64 => {
                                    f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
                                    saved.push(SavedField::F64_0);
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
                        Ty::I64 => {
                            f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                            saved.push(SavedField::I64_0);
                        }
                        Ty::F64 => {
                            f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
                            saved.push(SavedField::F64_0);
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
                    Ty::I64 => {
                        f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                        saved.push(SavedField::I64_0);
                    }
                    Ty::F64 => {
                        f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
                        saved.push(SavedField::F64_0);
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
                            SavedField::F64_0 => {
                                f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
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
                    SavedField::F64_0 => {
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
                        f.instruction(&Instruction::F64Store(MemArg {
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

    /// Build a value-level product (`Foo(a * b * c)` or `Foo(a, b, c)`).
    ///
    /// Allocates one heap block sized to the product's field layout,
    /// then for each field: pushes the struct base, compiles the field
    /// expression, and stores the result at the field's byte offset.
    /// Returns the struct pointer typed as `Ty::NamedPtr(product_name)`,
    /// which downstream `Expr::FieldAccess` reads back from in
    /// `compile_expr` (matching offset via `product_field_layout`).
    ///
    /// Field expressions are assumed to be positional (same order as
    /// the type-level field declaration, which the parser preserves
    /// and the alphabetical-ordering rule pins).
    /// Every type `name` widens to, most-specific first: itself, its
    /// newtype-alias targets (`Value` → `String`), and — if it names a
    /// union variant — its parent union and that union's aliases
    /// (`Empty` → `Map`). This is the set a value of type `name` can
    /// satisfy, used to bind product values to fields by type rather
    /// than by position.
    fn widening_chain(&self, name: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for link in self.collect_alias_chain(name) {
            if !out.contains(&link) {
                out.push(link);
            }
        }
        if let Some(parent) = self.variant_parent.get(name) {
            for link in self.collect_alias_chain(parent) {
                if !out.contains(&link) {
                    out.push(link);
                }
            }
        }
        out
    }

    /// How well a value (given its widening chain) fits a field of type
    /// `field_ty`: `2` when the field's exact newtype appears on the
    /// value's chain (`Value` value → `Value` field), `1` when they
    /// merely share a base type (`String` value → `Key` field, both
    /// erase to `String`), `0` when unrelated.
    fn field_match_score(&self, value_chain: &[String], field_ty: &str) -> u8 {
        if value_chain.iter().any(|n| n == field_ty) {
            return 2;
        }
        let field_chain = self.widening_chain(field_ty);
        if value_chain.iter().any(|n| field_chain.contains(n)) {
            return 1;
        }
        0
    }

    fn build_product_value(
        &mut self,
        product_name: &str,
        field_exprs: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let layout = self.product_field_layout(product_name);
        let total_size: u32 = layout
            .iter()
            .map(|(name, _, _)| self.field_byte_size(name))
            .sum::<u32>()
            .max(4); // `alloc` expects a non-zero size.

        // ── Bind values to fields by type, not by position ────────────
        // Fields are alphabetical and construction is positionless
        // (`Node(String * Empty() * Value)` and `Node(Empty() * Value *
        // String)` build the same struct). Each value is routed to the
        // field whose type it best matches: an exact newtype match
        // (`Value` → the `Value` field) wins over a shared-base match
        // (a bare `String` → the `Key` field), and any leftovers fall
        // back to declaration order. Same-typed fields (map's `Key` and
        // `Value`, both `String`) are why newtypes matter — tag a value
        // `Value(x)` and it lands in the `Value` slot regardless of
        // where it was written.
        let n_fields = layout.len().min(field_exprs.len());
        let value_chains: Vec<Option<Vec<String>>> = field_exprs
            .iter()
            .map(|e| {
                self.infer_ctor_arg_type_name(e)
                    .map(|nm| self.widening_chain(&nm))
            })
            .collect();
        let mut used = vec![false; field_exprs.len()];
        let mut slot_val: Vec<Option<usize>> = vec![None; n_fields];
        // Pass 1 (exact) then pass 2 (shared-base): a slot claims the
        // first unused value that scores at the current threshold.
        for threshold in [2u8, 1u8] {
            for (si, (field_name, _, _)) in layout.iter().take(n_fields).enumerate() {
                if slot_val[si].is_some() {
                    continue;
                }
                if let Some(vi) = (0..field_exprs.len()).find(|&vi| {
                    !used[vi]
                        && value_chains[vi]
                            .as_ref()
                            .is_some_and(|vc| self.field_match_score(vc, field_name) == threshold)
                }) {
                    slot_val[si] = Some(vi);
                    used[vi] = true;
                }
            }
        }
        // Pass 3 (positional): unresolved values fill remaining slots in
        // order — the pre-typed-construction behaviour, kept as a floor.
        for slot in slot_val.iter_mut().take(n_fields) {
            if slot.is_some() {
                continue;
            }
            if let Some(vi) = (0..field_exprs.len()).find(|&vi| !used[vi]) {
                *slot = Some(vi);
                used[vi] = true;
            }
        }

        // ── Allocate ──────────────────────────────────────────────────
        f.instruction(&Instruction::I32Const(total_size as i32));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));

        // ── Pre-push base copies on the operand stack ─────────────────
        // A nested constructor inside any field expression (`Some("hi")`,
        // an inner product, …) reassigns `scope.alloc_ptr()`, so the
        // local can't be trusted after the first `compile_expr`. Values
        // already on the operand stack, however, sit safely below a
        // nested expression's own stack activity. So: one copy per
        // stored field (consumed bottom-up by the stores below) plus
        // one at the very bottom that survives as the result.
        for _ in 0..=n_fields {
            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        }

        // ── Lay out each field ────────────────────────────────────────
        // The store helper accepts `[addr, value]` (scalar) or
        // `[addr, ptr, len]` (string) and consumes the address copy
        // pre-pushed above.
        for (i, (_field_name, field_repr, field_offset)) in layout.iter().take(n_fields).enumerate()
        {
            let vi = slot_val[i].unwrap_or(i);
            let _val_ty = self.compile_expr(&field_exprs[vi], scope, f);
            self.store_payload_at_offset(*field_offset, field_repr, scope, f);
        }

        // ── Result ────────────────────────────────────────────────────
        // The bottom-most base copy is still on the stack.
        Ty::NamedPtr(product_name.to_string())
    }

    /// Load a single field from a heap-allocated product struct.
    ///
    /// Stack contract: enters with `[ptr_to_struct]` on top, exits with
    /// the field value laid out per `field_repr` (one i32/i64 for
    /// scalars / named pointers, two i32s `[ptr, len]` for strings).
    /// Returns the field's wasm repr so the caller can thread it
    /// through subsequent method dispatch.
    ///
    /// Returns `None` if `field_name` is not a known field of
    /// `product_name` (the caller is responsible for the fallback).
    fn load_product_field(
        &self,
        product_name: &str,
        field_name: &str,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Option<Ty> {
        let layout = self.product_field_layout(product_name);
        let (_, field_repr, field_offset) =
            layout.iter().find(|(n, _, _)| n == field_name).cloned()?;
        match &field_repr {
            Ty::I64 => {
                f.instruction(&Instruction::I64Load(MemArg {
                    offset: field_offset as u64,
                    align: 3,
                    memory_index: 0,
                }));
                Some(field_repr)
            }
            Ty::F64 => {
                f.instruction(&Instruction::F64Load(MemArg {
                    offset: field_offset as u64,
                    align: 3,
                    memory_index: 0,
                }));
                Some(field_repr)
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: field_offset as u64,
                    align: 2,
                    memory_index: 0,
                }));
                Some(field_repr)
            }
            Ty::Str | Ty::NamedStr(_) => {
                // Stack: [base]. Stash base, then re-load it twice to
                // emit the (ptr, len) pair as two i32 loads.
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: field_offset as u64,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: (field_offset + 4) as u64,
                    align: 2,
                    memory_index: 0,
                }));
                Some(field_repr)
            }
            Ty::List | Ty::Unit => {
                f.instruction(&Instruction::Drop);
                None
            }
        }
    }

    /// Build a list value from positional element expressions.
    ///
    /// Each slot is fixed at 8 bytes regardless of element type. The
    /// layout per slot is:
    ///
    ///   * `Ty::I64`        → one i64 at offset 0.
    ///   * `Ty::I32`        → one i32 at offset 0, upper 4 bytes unused.
    ///   * `Ty::Str`/`NamedStr` → i32 ptr at offset 0, i32 len at offset 4.
    ///   * anything else    → dropped + zeroed (legacy fallback).
    ///
    /// The fixed 8-byte stride lets the same `(ptr, len)` representation
    /// describe lists of any of the above types; downstream methods
    /// dispatch on a `Ty::List` receiver and read back according to the
    /// expected element shape (see `compile_builtin_method`).
    fn build_list_literal(&mut self, args: &[Expr], scope: &LocalScope, f: &mut Function) -> Ty {
        // `List(a * b * c)` — the elements arrive as one product now that
        // comma argument lists are gone; flatten it to the element list.
        // A single non-product element (`List("x")`) stays one element.
        let flat: Vec<Expr>;
        let args: &[Expr] = match args {
            [Expr::ProductValue { fields, .. }] => {
                flat = fields.clone();
                &flat
            }
            _ => args,
        };
        let n = args.len() as u32;
        let byte_size = n * 8;
        f.instruction(&Instruction::I32Const(byte_size as i32));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));

        for (i, arg) in args.iter().enumerate() {
            let ty = self.compile_expr(arg, scope, f);
            let slot_offset = (i as u64) * 8;
            match ty {
                Ty::I64 => {
                    // Stack: [value]. Store at slot.
                    f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                    f.instruction(&Instruction::I64Store(MemArg {
                        offset: slot_offset,
                        align: 3,
                        memory_index: 0,
                    }));
                }
                Ty::F64 => {
                    f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
                    f.instruction(&Instruction::F64Store(MemArg {
                        offset: slot_offset,
                        align: 3,
                        memory_index: 0,
                    }));
                }
                Ty::I32 => {
                    // Promote i32 to i64 so all numeric lists share the
                    // same wire format. Upper 4 bytes carry the
                    // sign-extension; callers reading back as i32 simply
                    // load the low 4 bytes.
                    f.instruction(&Instruction::I64ExtendI32S);
                    f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                    f.instruction(&Instruction::I64Store(MemArg {
                        offset: slot_offset,
                        align: 3,
                        memory_index: 0,
                    }));
                }
                Ty::Str | Ty::NamedStr(_) => {
                    // Stack: [ptr, len]. Stash len, then ptr, then store
                    // them at offset+0 and offset+4 of the slot.
                    f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // len
                    f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // ptr
                                                                            // Store ptr at offset+0
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                    f.instruction(&Instruction::I32Store(MemArg {
                        offset: slot_offset,
                        align: 2,
                        memory_index: 0,
                    }));
                    // Store len at offset+4
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                    f.instruction(&Instruction::I32Store(MemArg {
                        offset: slot_offset + 4,
                        align: 2,
                        memory_index: 0,
                    }));
                }
                other => {
                    self.drop_value(other, f);
                    // Zero the slot so a later read doesn't see
                    // uninitialised heap bytes.
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::I64Const(0));
                    f.instruction(&Instruction::I64Store(MemArg {
                        offset: slot_offset,
                        align: 3,
                        memory_index: 0,
                    }));
                }
            }
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
        // Concurrency combinators: `a.parallel(b)` / `a.race(b)`. The
        // receiver and argument are *un-awaited* async calls (the
        // auto-await pass exempts these two methods); compile_parallel /
        // compile_race emit the non-blocking call for each side
        // themselves, so the receiver must NOT be compiled here.
        if matches!(method, "parallel" | "race" | "Parallel" | "Race") && args.len() == 1 {
            let combined = [receiver.clone(), args[0].clone()];
            return if method.eq_ignore_ascii_case("parallel") {
                self.compile_parallel(&combined, scope, f)
            } else {
                self.compile_race(&combined, scope, f)
            };
        }

        // The pipe form of prefix construction: `A -> B(C)` is the same
        // call as `B(A * C)` — the receiver fills the first slot of `B`'s
        // input product. When `B` names a type constructor, route it
        // through `compile_constructor` (the single construction path)
        // so piped and prefix spellings build identically: products,
        // union variants, newtypes, primitive conversions, constructor
        // families, shapes, and the HTTP `Response` all handled there.
        // Builtins (`Sum`, `Ge`, `Joined`, …) and pure operations are
        // not type names, so they fall through to the method paths
        // below. Runs before the receiver is compiled, so
        // `compile_constructor` owns every input — no double emit.
        // A name in the builtin vocabulary (`Length`, `Sum`, `Mapped`,
        // `Eq`, …) is never construction even when it also names a type
        // (`Length = Int`, `Mapped<U> = List<U>`): the method paths below
        // own it as a builtin on the receiver (list length / map) or a
        // stdlib shape. Excluding it keeps `list -> Length` a length, not
        // a `Length(list)` newtype wrap.
        let is_builtin_op = crate::ast::builtin_method_alias(method).is_some();
        // A name with a func-table body is a shape / constructor family
        // (`Route`, `Served`, `TestResult`, `Greeting`'s Int member, …).
        // Those resolve on the method path below, keyed on the receiver's
        // *compiled* type — routing them through `compile_constructor`
        // would rebuild the receiver and lose handle/repr threading. Only
        // *pure* construction (a product / newtype / variant / primitive
        // with no func body) needs the construction route.
        let has_func_body = self.func_table.keys().any(|(_, m)| m == method);
        let is_ctor_name = (!is_builtin_op
            && !has_func_body
            && method.chars().next().is_some_and(char::is_uppercase)
            && (self.type_defs.contains_key(method)
                || self.variant_parent.contains_key(method)
                || matches!(method, "Some" | "None" | "Ok" | "Err")))
            // HTTP `Response` construction is owned by codegen
            // (`build_http_response`) regardless of its checker binding,
            // so route the piped form there too.
            || (self.http_mode && method == "Response");
        if is_ctor_name {
            let mut ctor_inputs = vec![receiver.clone()];
            match args {
                [Expr::ProductValue { fields, .. }] => ctor_inputs.extend(fields.iter().cloned()),
                _ => ctor_inputs.extend(args.iter().cloned()),
            }
            let ctor_args = if ctor_inputs.len() == 1 {
                ctor_inputs
            } else {
                vec![Expr::ProductValue {
                    fields: ctor_inputs,
                    span: receiver.span(),
                }]
            };
            return self.compile_constructor(method, &ctor_args, scope, f);
        }

        // A single product argument stands for its flattened components:
        // `headers.set(Name * Value)`, `server.route(a * b * c * d)`, and
        // every other multi-input builtin/binding receive positional args
        // this way now that comma argument lists are gone. (The checker's
        // `effective_call_arity` already flattens for arity; codegen
        // matches here.) `substring`/`slice` keep the product intact —
        // `substring_bounds` reads the `From`/`To` components by type, so
        // it stays positionless.
        let flat_args: Vec<Expr>;
        let args: &[Expr] = match args {
            [Expr::ProductValue { fields, .. }] if !matches!(method, "substring" | "Substring") => {
                flat_args = fields.clone();
                &flat_args
            }
            _ => args,
        };

        let recv_ty = self.compile_expr(receiver, scope, f);

        // Check user func table first: look up by Canon type name. Scalars
        // (`Int`, `Float`, `Bool`, `String`) don't carry their name on the
        // `Ty` enum, so we map them back to a canonical Canon type name here
        // — this lets `extern Wasm` declarations with scalar receivers (e.g.
        // `min = (Int * …)`) resolve from a call site like `5.min(…)`.
        //
        // Capability receivers (`Random`, `Stdout`, `Clock`, …) leave nothing
        // on the stack and have type `Ty::Unit`. We recover their type name
        // from the AST identifier so calls like `Random.randomInt` resolve.
        let type_name = recv_ty
            .canon_name()
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
        // Try the receiver's own type name first, then every name in
        // its newtype alias chain — `Foo("x").ToJson()` with `Foo =
        // String` must find a `ToJson` declared on `String`.
        // A method resolves to a user/stdlib function under its written
        // name or — for the types-only vocabulary — under its camelCase
        // alias. `stream -> Mapped(f)` finds the `map` binding on
        // `Stream` (a camelCase FFI function) before the `List` builtin
        // `Mapped` claims it; `list -> Mapped(f)` misses both bindings
        // and falls through to the builtin below.
        let method_names: Vec<String> = match crate::ast::builtin_method_alias(method) {
            Some(canonical) => vec![method.to_string(), canonical.to_string()],
            None => vec![method.to_string()],
        };
        // A scalar newtype erases to its underlying primitive, so a piped
        // construction like `3000 -> Port` leaves `Ty::I64` on the stack
        // and `type_name` recovers only "Int" — losing "Port", which the
        // next step (`Port -> HttpServer`) dispatches on. Recover the
        // *static* type from the receiver's syntactic shape: `Foo(x)` or
        // `x -> Foo` constructs a `Foo` when `Foo` names a type. Tried
        // first so newtype-typed shapes still resolve.
        let static_recv_type: Option<String> = match receiver {
            Expr::Constructor { name, .. } if self.type_defs.contains_key(&name.name) => {
                Some(name.name.clone())
            }
            Expr::MethodCall {
                method: m,
                piped: true,
                ..
            } if self.type_defs.contains_key(&m.name) => Some(m.name.clone()),
            _ => None,
        };
        let mut candidate_types: Vec<String> = Vec::new();
        if let Some(st) = &static_recv_type {
            candidate_types.extend(self.collect_alias_chain(st));
        }
        if let Some(name) = &type_name {
            for a in self.collect_alias_chain(name) {
                if !candidate_types.contains(&a) {
                    candidate_types.push(a);
                }
            }
        }
        for alias in candidate_types {
            for m in &method_names {
                let key = (Some(alias.clone()), m.clone());
                if let Some(info) = self.func_table.get(&key).cloned() {
                    return self.emit_func_table_call(&info, args, scope, f);
                }
            }
        }
        if type_name.is_none() {
            for m in &method_names {
                if let Some(info) = self.func_table.get(&(None, m.clone())).cloned() {
                    return self.emit_func_table_call(&info, args, scope, f);
                }
            }
        }

        // Also try without type name (free functions used as methods)
        let free_key = (None, method.to_string());
        if type_name.is_some() {
            if let Some(info) = self.func_table.get(&free_key).cloned() {
                return self.emit_func_table_call(&info, args, scope, f);
            }
        }

        // No user/stdlib function matched — normalize the types-only
        // vocabulary (`Print`/`Sum`/`Joined`/…) to its canonical builtin
        // name so the `print`/`String`/builtin paths below recognize it.
        let method = crate::ast::builtin_method_alias(method).unwrap_or(method);

        // Conversion is construction (the language spec, docs/src/spec/):
        // `Int.String()` / `Byte.String()` are the method spellings of
        // the `String(Int)` / `String(Byte)` constructors. Placed after
        // the func-table lookups so a user-declared `String` method on
        // some other receiver type still wins.
        if method == "String" && args.is_empty() {
            match &recv_ty {
                Ty::I64 => {
                    return if self.expr_is_byte(receiver) {
                        self.emit_byte_to_str(scope, f)
                    } else {
                        f.instruction(&Instruction::Call(self.fn_int_to_str));
                        Ty::Str
                    };
                }
                // A String-alias receiver (`Path("/x").String()`) is
                // the identity conversion — the value already is one.
                ty if ty.is_str_like() => return Ty::Str,
                _ => {}
            }
        }

        // Primitive construction via pipe: `1 -> Int`, `2.5 -> Float`,
        // `b -> Bool`. The receiver is already on the stack; widen /
        // convert / pass through, mirroring `compile_constructor`'s
        // primitive arm. (`"5" -> Int` parses via a `(String) -> Int`
        // func-table member above, so only the non-string cases land
        // here.)
        if args.is_empty() {
            match (method, &recv_ty) {
                ("Int", Ty::I64) => return Ty::I64,
                ("Int", Ty::I32) => {
                    f.instruction(&Instruction::I64ExtendI32S);
                    return Ty::I64;
                }
                ("Int", Ty::F64) => {
                    f.instruction(&Instruction::I64TruncF64S);
                    return Ty::I64;
                }
                ("Float", Ty::F64) => return Ty::F64,
                ("Float", Ty::I64) => {
                    f.instruction(&Instruction::F64ConvertI64S);
                    return Ty::F64;
                }
                ("Bool", Ty::I32) => return Ty::I32,
                _ => {}
            }
        }

        // Newtype wrap via pipe: `"hi" -> Greeting` with `Greeting =
        // String` is the identity — the receiver already carries the
        // underlying representation, so relabel it to the newtype. Only
        // fires when the newtype's repr matches the receiver's, so a
        // *conversion* (`5 -> Json`, resolved above or a type error)
        // never silently becomes an identity wrap.
        if args.is_empty() {
            if let Some(TypeExpr::Named { .. }) = self.type_defs.get(method) {
                let wrapped = self.resolve_repr(method);
                let compatible = std::mem::discriminant(&wrapped)
                    == std::mem::discriminant(&recv_ty)
                    || (wrapped.is_str_like() && recv_ty.is_str_like());
                if compatible {
                    return wrapped;
                }
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
        // Narrow-width conversions (WIT-informed lowering). Canon's
        // `Int` is i64 everywhere; a `wasi:*` extern whose WIT declares
        // u8/u16/u32/s8/s16/s32 has a core i32 slot instead. The
        // receiver (when present) is already on the stack — component
        // param 0 with everything else still unpushed, so its wrap must
        // happen before the args compile.
        let recv_count = info.narrow_params.len().saturating_sub(args.len());
        let narrow_at = |i: usize| info.narrow_params.get(i).copied().unwrap_or(false);
        if recv_count == 1 && narrow_at(0) {
            f.instruction(&Instruction::I32WrapI64);
        }
        for (i, a) in args.iter().enumerate() {
            let _ = self.compile_expr(a, scope, f);
            if narrow_at(recv_count + i) {
                f.instruction(&Instruction::I32WrapI64);
            }
        }
        if info.is_async {
            return self.emit_async_call(info, scope, f);
        }
        let Some(shape) = info.indirect_return.clone() else {
            f.instruction(&Instruction::Call(info.func_idx));
            // Widen a narrow scalar result back to Canon's i64 `Int`,
            // zero- or sign-extending per the WIT signedness.
            match info.narrow_result_signed {
                Some(true) => {
                    f.instruction(&Instruction::I64ExtendI32S);
                }
                Some(false) => {
                    f.instruction(&Instruction::I64ExtendI32U);
                }
                None => {}
            }
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
                // (i32 ptr at +0, i32 len at +4) — push both as a string
                // pair. Use `info.result_ty` so the alias name is
                // preserved (set up by `assign_func_indices`).
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
            IndirectReturnShape::OptionString => {
                // Re-shape the canonical `option<string>` ret area
                // (disc byte at +0, ptr/len at +4/+8) into a fresh
                // Canon Option struct (i32 tag at +0, payload at
                // +4/+8). `$alloc` doesn't touch the `alloc_ptr`
                // *local*, which still points at the ret area.
                f.instruction(&Instruction::I32Const(12));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load8U(MemArg {
                    offset: 0,
                    align: 0,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                for off in [4u64, 8] {
                    f.instruction(&Instruction::LocalGet(scope.rbool()));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::I32Load(MemArg {
                        offset: off,
                        align: 2,
                        memory_index: 0,
                    }));
                    f.instruction(&Instruction::I32Store(MemArg {
                        offset: off,
                        align: 2,
                        memory_index: 0,
                    }));
                }
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                Ty::NamedPtr("Option".to_string())
            }
            IndirectReturnShape::ListString => {
                // (i32 list ptr at +0, i32 count at +4). The canonical
                // element layout matches Canon's `List<String>` exactly
                // — push the pair as-is.
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
                Ty::List
            }
            IndirectReturnShape::ScalarRecord {
                product, fields, ..
            } => {
                // Copy each canonical field into a fresh Canon product
                // struct, widening narrow ints to i64. The ret area is
                // still in the `alloc_ptr` local ($alloc the function
                // doesn't touch codegen locals).
                use wasm_encoder::PrimitiveValType as P;
                let layout = self.product_field_layout(&product);
                let total: u32 = layout
                    .iter()
                    .map(|(n, _, _)| self.field_byte_size(n))
                    .sum::<u32>()
                    .max(4);
                f.instruction(&Instruction::I32Const(total as i32));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                for field in &fields {
                    let Some((_, repr, canon_off)) =
                        layout.iter().find(|(n, _, _)| n == &field.canon_name)
                    else {
                        continue;
                    };
                    f.instruction(&Instruction::LocalGet(scope.rbool()));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    let off = field.offset as u64;
                    match field.prim {
                        P::U64 | P::S64 => {
                            f.instruction(&Instruction::I64Load(MemArg {
                                offset: off,
                                align: 3,
                                memory_index: 0,
                            }));
                        }
                        P::F64 => {
                            f.instruction(&Instruction::F64Load(MemArg {
                                offset: off,
                                align: 3,
                                memory_index: 0,
                            }));
                        }
                        P::U32 | P::S32 | P::U16 | P::S16 | P::U8 | P::S8 | P::Bool | P::Char => {
                            match field.prim {
                                P::U16 | P::S16 => {
                                    f.instruction(&Instruction::I32Load16U(MemArg {
                                        offset: off,
                                        align: 1,
                                        memory_index: 0,
                                    }));
                                }
                                P::U8 | P::S8 | P::Bool => {
                                    f.instruction(&Instruction::I32Load8U(MemArg {
                                        offset: off,
                                        align: 0,
                                        memory_index: 0,
                                    }));
                                }
                                _ => {
                                    f.instruction(&Instruction::I32Load(MemArg {
                                        offset: off,
                                        align: 2,
                                        memory_index: 0,
                                    }));
                                }
                            }
                            if matches!(field.prim, P::S8 | P::S16 | P::S32) {
                                f.instruction(&Instruction::I64ExtendI32S);
                            } else {
                                f.instruction(&Instruction::I64ExtendI32U);
                            }
                        }
                        _ => {
                            f.instruction(&Instruction::I64Const(0));
                        }
                    }
                    match repr {
                        Ty::F64 => {
                            f.instruction(&Instruction::F64Store(MemArg {
                                offset: *canon_off as u64,
                                align: 3,
                                memory_index: 0,
                            }));
                        }
                        _ => {
                            f.instruction(&Instruction::I64Store(MemArg {
                                offset: *canon_off as u64,
                                align: 3,
                                memory_index: 0,
                            }));
                        }
                    }
                }
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                Ty::NamedPtr(product)
            }
            IndirectReturnShape::ResultStringString { ok_name, err_name } => {
                // Flip the WIT discriminant (byte 0) into Canon's tag
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

    // ── Concurrency combinators ─────────────────────────────────────
    //
    // `parallel(a, b)` and `race(a, b)` are guest-side combinators: the
    // codegen emits a non-blocking async call for each arg (capturing
    // subtask handle + ret-area into named locals), then runs the
    // canonical-ABI multi-subtask wait sequence in the same function.
    // No host bridge is involved — the `canon:async/waitable` canon
    // intrinsics (`set-new`, `join`, `set-wait`, `set-drop`,
    // `subtask-drop`, `subtask-cancel`) handle everything.

    /// Compile a single `parallel`/`race` argument as a non-blocking
    /// async call. The arg must be a `MethodCall` or `Constructor` that
    /// resolves to an `extern Wasm.async` function in `func_table`.
    ///
    /// On exit:
    ///   - The arg's sub-args are evaluated.
    ///   - The arg's ret-area is allocated into `retarea_local`.
    ///   - The import is called; the packed status is consumed.
    ///   - The subtask handle (status >> 4) is stored in `subtask_local`.
    ///
    /// Returns the callee's declared `result_ty` so the caller knows how
    /// to decode the ret-area later.
    ///
    /// Today this is conservative: if the arg shape doesn't match a known
    /// async extern, the codegen traps via `unreachable`. The checker
    /// can't surface a friendlier error yet because the surface is brand
    /// new; clean up once user pain reports.
    fn emit_arg_as_nonblocking(
        &mut self,
        arg: &Expr,
        scope: &LocalScope,
        f: &mut Function,
        subtask_local: u32,
        retarea_local: u32,
    ) -> Ty {
        // Resolve the callee FuncInfo and identify the receiver / args.
        let resolved: Option<(FuncInfo, Option<Box<Expr>>, Vec<Expr>)> = match arg {
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => {
                let recv_ty_name = self.infer_static_type_name(receiver);
                let key = recv_ty_name.map(|n| (Some(n), method.name.clone()));
                let info = key
                    .and_then(|k| self.func_table.get(&k).cloned())
                    .or_else(|| self.func_table.get(&(None, method.name.clone())).cloned());
                info.map(|i| (i, Some(receiver.clone()), args.clone()))
            }
            Expr::Constructor { name, args, .. } => {
                // Try free-function key first.
                let mut info = self.func_table.get(&(None, name.name.clone())).cloned();
                // Then try Self-renamed constructor.
                if info.is_none() {
                    info = self
                        .func_table
                        .get(&(Some(name.name.clone()), "Self".to_string()))
                        .cloned();
                }
                // Then try capability-receiver: first arg's type as receiver.
                if info.is_none() {
                    if let Some(first) = args.first() {
                        if let Some(tname) = self.infer_static_type_name(first) {
                            info = self
                                .func_table
                                .get(&(Some(tname), name.name.clone()))
                                .cloned();
                        }
                    }
                }
                info.map(|i| (i, None, args.clone()))
            }
            _ => None,
        };

        let Some((info, receiver_opt, args_to_push)) = resolved else {
            // Couldn't resolve the call; trap. Callers should ensure the
            // arg points to a real async extern.
            f.instruction(&Instruction::Unreachable);
            return Ty::Unit;
        };

        if !info.is_async {
            // Only async calls make sense here — a sync call would
            // complete immediately and there'd be no subtask to wait on.
            f.instruction(&Instruction::Unreachable);
            return info.result_ty.clone();
        }

        // Push the receiver expression first (for MethodCall form). The
        // receiver becomes the first param of the import call.
        if let Some(rcv) = receiver_opt {
            let _ = self.compile_expr(&rcv, scope, f);
        }
        // Then the explicit args.
        for a in args_to_push {
            let _ = self.compile_expr(&a, scope, f);
        }

        // Allocate the ret-area and tee into `retarea_local` (leaving the
        // ptr on the stack as the last param to the import).
        let has_result = !matches!(info.result_ty, Ty::Unit);
        if has_result {
            let size = ret_area_size_for(&info.result_ty);
            f.instruction(&Instruction::I32Const(size as i32));
            f.instruction(&Instruction::Call(self.fn_alloc));
            f.instruction(&Instruction::LocalTee(retarea_local));
        } else {
            f.instruction(&Instruction::I32Const(0));
            f.instruction(&Instruction::LocalSet(retarea_local));
        }

        // Call the async-lowered import. Stack on return: i32 packed status.
        f.instruction(&Instruction::Call(info.func_idx));

        // Extract subtask handle = status >> 4. The low 4 bits encode the
        // CallState; the high 28 bits are the subtask waitable handle.
        f.instruction(&Instruction::I32Const(4));
        f.instruction(&Instruction::I32ShrU);
        f.instruction(&Instruction::LocalSet(subtask_local));

        info.result_ty.clone()
    }

    /// Emit `parallel(a, b)`: start both async calls non-blocking, join
    /// their subtasks to a fresh waitable-set, loop until both events
    /// fire, then build a `List<T>` with the two results in arg-order.
    ///
    /// Both args must call async externs returning the same payload type.
    /// The result type is `Ty::List`. Today only `Ty::Str` / `Ty::NamedStr`
    /// element shapes are decoded; other shapes trap.
    fn compile_parallel(&mut self, args: &[Expr], scope: &LocalScope, f: &mut Function) -> Ty {
        if args.len() != 2 {
            // Surface error: parallel expects exactly two args. The
            // checker doesn't yet validate arity for synthetic combinators.
            f.instruction(&Instruction::Unreachable);
            return Ty::List;
        }

        // ── Start both calls non-blocking ─────────────────────────
        let ty_a = self.emit_arg_as_nonblocking(
            &args[0],
            scope,
            f,
            scope.par_subtask_a(),
            scope.par_retarea_a(),
        );
        let ty_b = self.emit_arg_as_nonblocking(
            &args[1],
            scope,
            f,
            scope.par_subtask_b(),
            scope.par_retarea_b(),
        );
        // Both arms must agree on element type.
        let _ = ty_b;
        let elem_ty = ty_a;

        // ── Build waitable-set, join both ──────────────────────────
        f.instruction(&Instruction::Call(self.fn_waitable_set_new));
        f.instruction(&Instruction::LocalSet(scope.par_set()));

        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));

        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));

        // ── Event area + seen flags ─────────────────────────────
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.par_event_ptr()));

        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.par_seen_b()));

        // ── Wait loop until both seen ───────────────────────────
        //
        // Structure:
        //   block $break
        //     loop $continue
        //       wait; drop event_code
        //       handle = load i32 at par_event_ptr+0
        //       handle == subtask_a ? seen_a = 1
        //       handle == subtask_b ? seen_b = 1
        //       (seen_a & seen_b) ? br $break (depth=1)
        //       br $continue (depth=0)
        //     end
        //   end
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));

        // waitable-set.wait(set, event_area) → event_code; drop event_code
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_wait));
        f.instruction(&Instruction::Drop);

        // event_handle = load i32 at par_event_ptr+0 → tmp_i32
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));

        // if event_handle == subtask_a: seen_a = 1
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));
        f.instruction(&Instruction::End);

        // if event_handle == subtask_b: seen_b = 1
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(scope.par_seen_b()));
        f.instruction(&Instruction::End);

        // if (seen_a & seen_b): br $break (depth 1 — the block above the loop)
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::BrIf(1));

        // br $continue (depth 0 — the loop itself)
        f.instruction(&Instruction::Br(0));

        f.instruction(&Instruction::End); // end loop
        f.instruction(&Instruction::End); // end block

        // ── Cleanup: drop subtasks before the set ────────────────────
        // Subtasks are children of the set; the set's drop requires no
        // children (see wasmtime's `ResourceTableError::HasChildren`).
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_drop));

        // ── Build List<T> with the two results ──────────────────────
        // List layout per `build_list_literal`: N*8 bytes, each slot is
        // (ptr i32, len i32) for Str / (8 bytes for I64/F64) at offsets
        // i*8. Total size = 16 for 2 elements.
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));

        match &elem_ty {
            Ty::Str | Ty::NamedStr(_) => {
                // slot 0 ← (ptr,len) at par_retarea_a +0/+4
                self.copy_str_pair(f, scope.alloc_ptr(), 0, scope.par_retarea_a(), 0);
                // slot 1 ← (ptr,len) at par_retarea_b +0/+4
                self.copy_str_pair(f, scope.alloc_ptr(), 8, scope.par_retarea_b(), 0);
            }
            Ty::I64 | Ty::F64 => {
                // Each slot is one i64. Source ret-area holds the value at +0.
                for (slot_off, retarea) in
                    [(0u64, scope.par_retarea_a()), (8u64, scope.par_retarea_b())]
                {
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(retarea));
                    f.instruction(&Instruction::I64Load(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                    f.instruction(&Instruction::I64Store(MemArg {
                        offset: slot_off,
                        align: 3,
                        memory_index: 0,
                    }));
                }
            }
            _ => {
                // Other element shapes not yet supported. Trap so the gap
                // is visible (we'd silently corrupt the list otherwise).
                f.instruction(&Instruction::Unreachable);
            }
        }

        // Push (list_ptr, len=2) — the standard `Ty::List` representation.
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(2));
        Ty::List
    }

    /// Emit `race(a, b)`: start both async calls non-blocking, wait for
    /// the *first* event, cancel the loser, drop everything, and return
    /// the winner's result decoded from its ret-area.
    ///
    /// Today only `Ty::Str` / `Ty::NamedStr` element shapes are decoded;
    /// other shapes trap.
    fn compile_race(&mut self, args: &[Expr], scope: &LocalScope, f: &mut Function) -> Ty {
        if args.len() != 2 {
            f.instruction(&Instruction::Unreachable);
            return Ty::Str;
        }

        // Start both calls non-blocking.
        let ty_a = self.emit_arg_as_nonblocking(
            &args[0],
            scope,
            f,
            scope.par_subtask_a(),
            scope.par_retarea_a(),
        );
        let _ = self.emit_arg_as_nonblocking(
            &args[1],
            scope,
            f,
            scope.par_subtask_b(),
            scope.par_retarea_b(),
        );
        let elem_ty = ty_a;

        // Build waitable-set, join both.
        f.instruction(&Instruction::Call(self.fn_waitable_set_new));
        f.instruction(&Instruction::LocalSet(scope.par_set()));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));

        // Event area + flags. Re-using par_seen_a as "winner is a?".
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));

        // One wait, then identify the winner.
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_wait));
        f.instruction(&Instruction::Drop);

        // Read event handle into tmp_i32, set seen_a = (handle == subtask_a).
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalTee(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));

        // Cancel the loser. `subtask.cancel` takes a subtask handle and
        // returns a state code (which we drop). The runtime guarantees
        // teardown of any transitive subtasks.
        //
        // The cancel call returns an i32 status code, even when issued
        // with async semantics. We drop it; the caller only cares that
        // the loser is no longer producing observable side effects.
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::If(BlockType::Empty));
        // a won → cancel b
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::Call(self.fn_subtask_cancel));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::Else);
        // b won → cancel a
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::Call(self.fn_subtask_cancel));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::End);

        // Drop both subtasks before the set.
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_drop));

        // Decode the winner's ret-area onto the stack.
        match &elem_ty {
            Ty::Str | Ty::NamedStr(_) => {
                // if seen_a: push (par_retarea_a +0, +4) else (par_retarea_b +0, +4)
                // WASM `if` with result type doesn't natively allow pushing two
                // values — use a Select-style approach via a winner_retarea local.
                // Compute winner_retarea via Select.
                f.instruction(&Instruction::LocalGet(scope.par_retarea_a()));
                f.instruction(&Instruction::LocalGet(scope.par_retarea_b()));
                f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
                f.instruction(&Instruction::Select);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));

                // Push ptr, then len.
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                elem_ty
            }
            _ => {
                f.instruction(&Instruction::Unreachable);
                elem_ty
            }
        }
    }

    /// Copy a `(ptr i32, len i32)` pair from `src_local + src_off` to
    /// `dst_local + dst_off`. Small helper used by the list-building tail
    /// of `compile_parallel`.
    fn copy_str_pair(
        &self,
        f: &mut Function,
        dst_local: u32,
        dst_off: u64,
        src_local: u32,
        src_off: u64,
    ) {
        // ptr
        f.instruction(&Instruction::LocalGet(dst_local));
        f.instruction(&Instruction::LocalGet(src_local));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: src_off,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Store(MemArg {
            offset: dst_off,
            align: 2,
            memory_index: 0,
        }));
        // len
        f.instruction(&Instruction::LocalGet(dst_local));
        f.instruction(&Instruction::LocalGet(src_local));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: src_off + 4,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Store(MemArg {
            offset: dst_off + 4,
            align: 2,
            memory_index: 0,
        }));
    }

    /// Compile `list.map(lambda)` as an inlined element-wise loop.
    ///
    /// Entry stack: `[src_ptr, len]` (the `Ty::List` pair). Exit stack:
    /// `[dst_ptr, len]` of a freshly allocated result list. `elem_name`
    /// is the lambda parameter's type name (Canon lambda bodies refer
    /// to the parameter by its type name), `elem_repr` its resolved
    /// representation — only `Ty::I64` and string-shaped elements are
    /// supported by the caller's gate.
    ///
    /// Loop state (`src`, `dst`, `remaining`) is carried on the wasm
    /// operand stack through multi-value block/loop params, NOT in
    /// locals — the lambda body is arbitrary user code and may clobber
    /// every scratch local. The only locals live across the body are
    /// the element binding itself (`map_elem_i64` / `map_elem_ptr`),
    /// which is exactly what the body is supposed to read.
    fn compile_list_map(
        &mut self,
        elem_name: &str,
        elem_repr: &Ty,
        body: &Block,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let trio = self
            .user_type_map
            .get(&(
                vec![ValType::I32, ValType::I32, ValType::I32],
                vec![ValType::I32, ValType::I32, ValType::I32],
            ))
            .copied()
            .expect("list-map loop type reserved in compile()");
        let mem64 = MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        };
        let mem32 = MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        };
        let mem32_4 = MemArg {
            offset: 4,
            align: 2,
            memory_index: 0,
        };

        // ── Setup. Stack: [src, len] ─────────────────────────────────
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // len
        f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // src
                                                                     // dst_base = alloc(len*8 + 8) — the +8 keeps a zero-length list
                                                                     // from handing $alloc a zero size.
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // dst_base
                                                                  // Bottom-of-stack survivors: result (len, dst_base) …
        f.instruction(&Instruction::LocalGet(scope.tmp_i32())); // n
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b())); // dst_base
                                                                  // … and the loop-carried trio.
        f.instruction(&Instruction::LocalGet(scope.addr_scratch())); // src
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b())); // dst
        f.instruction(&Instruction::LocalGet(scope.tmp_i32())); // rem

        f.instruction(&Instruction::Block(BlockType::FunctionType(trio)));
        f.instruction(&Instruction::Loop(BlockType::FunctionType(trio)));
        // [src, dst, rem] — exit when rem == 0.
        f.instruction(&Instruction::LocalTee(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        // Peel the trio (no user code between here and the re-push, so
        // scratch locals are safe).
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // rem
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // dst
        f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // src
                                                                     // Bind the current element.
        match elem_repr {
            Ty::I64 => {
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::I64Load(mem64));
                f.instruction(&Instruction::LocalSet(scope.map_elem_i64()));
            }
            _ => {
                // String-shaped: (ptr, len) at +0/+4.
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::I32Load(mem32));
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr()));
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::I32Load(mem32_4));
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr() + 1));
            }
        }
        // Park the next iteration's state (and the current dst for the
        // post-body store) on the operand stack where the body can't
        // touch it: [new_src, dst_cur, new_dst, new_rem].
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);

        // ── The lambda body ──────────────────────────────────────────
        let elem_local = match elem_repr {
            Ty::I64 => scope.map_elem_i64(),
            _ => scope.map_elem_ptr(),
        };
        let mut inner = LocalScope {
            vars: scope.vars.clone(),
            param_count: scope.param_count,
        };
        for alias in self.collect_alias_chain(elem_name) {
            inner.vars.insert(alias, (elem_local, elem_repr.clone()));
        }
        let out_ty = self.compile_block_return(body, &inner, f);

        // ── Store the result, restore the trio ───────────────────────
        // Stash the body's result (the element locals are free again;
        // trio juggling below uses the i32 scratch, so i32-shaped
        // results go through the element pair instead).
        match &out_ty {
            Ty::I64 => {
                f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
            }
            Ty::F64 => {
                f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
            }
            Ty::Str | Ty::NamedStr(_) | Ty::List => {
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr() + 1));
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr()));
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr()));
            }
            Ty::Unit => {}
        }
        // [new_src, dst_cur, new_dst, new_rem] → locals.
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // new_rem
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // new_dst
        f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // dst_cur
                                                                     // Store the stashed result at dst_cur.
        match &out_ty {
            Ty::I64 => {
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                f.instruction(&Instruction::I64Store(mem64));
            }
            Ty::F64 => {
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
                f.instruction(&Instruction::F64Store(mem64));
            }
            Ty::Str | Ty::NamedStr(_) | Ty::List => {
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.map_elem_ptr()));
                f.instruction(&Instruction::I32Store(mem32));
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.map_elem_ptr() + 1));
                f.instruction(&Instruction::I32Store(mem32_4));
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.map_elem_ptr()));
                f.instruction(&Instruction::I32Store(mem32));
            }
            Ty::Unit => {}
        }
        // Rebuild the trio and continue: [new_src] + new_dst + new_rem.
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End); // loop
        f.instruction(&Instruction::End); // block

        // [n, dst_base, src_f, dst_f, rem_f] → [dst_base, n].
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // dst_base
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // n
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        Ty::List
    }

    fn compile_builtin_method(
        &mut self,
        recv_ty: Ty,
        method: &str,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        // Types-only vocabulary: `-> Print` / `-> Sum(2)` / `-> Joined(s)`
        // resolve to the same codegen as `print` / `add` / `concat`. Only
        // reached after the func_table lookup missed, so a user/stdlib
        // function of the same name always wins first.
        let method = crate::ast::builtin_method_alias(method).unwrap_or(method);
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
            // ── Bool composition ─────────────────────────────────────────────
            // Bools are i32 0/1. `and`/`or` are non-short-circuiting
            // (both sides evaluate) — acceptable because Canon
            // expressions are effect-free apart from capabilities, and
            // it matches the eager `.eq(..)` chains they compose with.
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
            // wasm has no f64 remainder instruction; compute
            // `a - trunc(a/b) * b` (sign follows the dividend, matching
            // Rust's `%` on floats). Both operands are needed twice and
            // wasm has no stack dup, so they round-trip through the
            // f64 scratch pair.
            ("mod" | "rem", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::LocalSet(scope.tmp_f64_b())); // b
                f.instruction(&Instruction::LocalSet(scope.tmp_f64())); // a
                f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
                f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
                f.instruction(&Instruction::LocalGet(scope.tmp_f64_b()));
                f.instruction(&Instruction::F64Div);
                f.instruction(&Instruction::F64Trunc);
                f.instruction(&Instruction::LocalGet(scope.tmp_f64_b()));
                f.instruction(&Instruction::F64Mul);
                f.instruction(&Instruction::F64Sub);
                Ty::F64
            }
            ("lt", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Lt);
                Ty::I32
            }
            ("le", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Le);
                Ty::I32
            }
            ("gt", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Gt);
                Ty::I32
            }
            ("ge", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Ge);
                Ty::I32
            }
            ("eq", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Eq);
                Ty::I32
            }
            ("ne", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Ne);
                Ty::I32
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
                //   str_scratch_ptr+1 = len2
                //   str_scratch_ptr   = ptr2
                //   tmp_i32_b         = len1 (kept immutable; used both as
                //                              n for copy 1 and as offset
                //                              into result for copy 2)
                //   rbool             = ptr1 (used as src in copy 1; the
                //                              copy loop modifies it)
                //
                // NOTE: deliberately uses `str_scratch_ptr` (not
                // `arm_payload_ptr`) so a `concat` call inside a
                // dispatch arm body doesn't corrupt the arm's bound
                // payload — see the gap fix in CLAUDE.md.
                f.instruction(&Instruction::LocalSet(scope.str_scratch_ptr() + 1));
                f.instruction(&Instruction::LocalSet(scope.str_scratch_ptr()));
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalSet(scope.rbool()));

                // total_len = len1 + len2, kept in tmp_i32 for the return.
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalGet(scope.str_scratch_ptr() + 1));
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
                f.instruction(&Instruction::LocalGet(scope.str_scratch_ptr()));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.str_scratch_ptr() + 1));
                f.instruction(&Instruction::LocalSet(scope.rlen()));
                self.emit_byte_copy_loop(scope, f);

                // Push (result_ptr, total_len) as the concat's return value.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                Ty::Str
            }
            // ── String length ───────────────────────────────────────
            //
            // Stack: [ptr, len] → [len_i64]. Drops the pointer; the
            // length is the i32 byte-count promoted to i64 (Canon `Int`).
            ("length", _) if recv_ty.is_str_like() => {
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // save len
                f.instruction(&Instruction::Drop); // drop ptr
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I64ExtendI32S);
                Ty::I64
            }
            // ── String byteAt ──────────────────────────────────────
            //
            // `s.byteAt(i)` returns the unsigned byte at index `i`
            // (0..=255) as an `Int`. Out-of-bounds access traps via the
            // raw `i32.load8_u` (wasmtime translates an OOB load into a
            // memory-out-of-bounds trap, which surfaces as a Rust panic
            // through wasmtime's runtime). For a string-as-bytes view of
            // a String — this is the primitive that makes Canon-side
            // string parsing possible.
            ("byteAt", _) if recv_ty.is_str_like() => {
                // Receiver on stack: [ptr, len]. Compile index arg next.
                let mut arg_pushed = false;
                if let Some(a) = args.first() {
                    let arg_ty = self.compile_expr(a, scope, f);
                    if matches!(arg_ty, Ty::I64) {
                        arg_pushed = true;
                    } else {
                        self.drop_value(arg_ty, f);
                    }
                }
                if !arg_pushed {
                    f.instruction(&Instruction::I64Const(1));
                }
                // Canon indexing is 1-based (like positional product
                // access `byte.1`): byteAt(1) is the first byte.
                f.instruction(&Instruction::I64Const(1));
                f.instruction(&Instruction::I64Sub);
                // Stack: [ptr, len, index_i64]. Want: load byte at ptr+index.
                f.instruction(&Instruction::I32WrapI64);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // index_i32
                f.instruction(&Instruction::Drop); // drop len
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Add); // ptr + index
                f.instruction(&Instruction::I32Load8U(MemArg {
                    offset: 0,
                    align: 0,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::I64ExtendI32U);
                Ty::I64
            }
            // ── String substring ────────────────────────────────────
            //
            // `s.substring(start, end)` returns the 1-based, inclusive
            // slice `[start, end]` as a fresh String — `substring(1, 4)`
            // is the first four bytes, pairing with 1-based `byteAt`.
            // Internally start is shifted down once and the old
            // half-open arithmetic does the rest (`len = end - (start-1)`).
            // Allocates a new buffer and copies the bytes — the result
            // is independent of the receiver's lifetime (heap is
            // bump-allocated, so neither outlives the other; copying
            // makes mutation safe if it ever lands).
            ("substring" | "slice", _)
                if recv_ty.is_str_like() && substring_bounds(args).is_some() =>
            {
                // The bounds arrive either as a `From * To` product (the
                // canonical, positionless form — alphabetical order puts
                // `From` first) or, during migration, as two positional
                // args. Either way: `start`, then `end` (both `Int`).
                let (start_e, end_e) = substring_bounds(args).unwrap();
                let ty0 = self.compile_expr(start_e, scope, f);
                if !matches!(ty0, Ty::I64) {
                    self.drop_value(ty0, f);
                    f.instruction(&Instruction::I64Const(1));
                }
                f.instruction(&Instruction::I64Const(1));
                f.instruction(&Instruction::I64Sub);
                let ty1 = self.compile_expr(end_e, scope, f);
                if !matches!(ty1, Ty::I64) {
                    self.drop_value(ty1, f);
                    f.instruction(&Instruction::I64Const(0));
                }
                // Stack: [ptr, len, start_i64, end_i64].
                f.instruction(&Instruction::I32WrapI64);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // end_i32
                f.instruction(&Instruction::I32WrapI64);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // start_i32
                f.instruction(&Instruction::Drop); // drop len
                                                   // src = ptr + start
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::LocalSet(scope.rbool())); // src
                                                                      // new_len = end - start (preserved in str_scratch_ptr for
                                                                      // the final return push; the copy loop will clobber rlen).
                                                                      // Uses `str_scratch_ptr` (not `arm_payload_ptr`) so a
                                                                      // `substring` call inside a dispatch arm body doesn't
                                                                      // corrupt the bound payload.
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Sub);
                f.instruction(&Instruction::LocalSet(scope.str_scratch_ptr()));
                // result_ptr = alloc(new_len)
                f.instruction(&Instruction::LocalGet(scope.str_scratch_ptr()));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
                // Copy loop locals: dst → rptr, src → rbool (already set),
                // n → rlen (decremented to 0 by the loop).
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalSet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.str_scratch_ptr()));
                f.instruction(&Instruction::LocalSet(scope.rlen()));
                self.emit_byte_copy_loop(scope, f);
                // Return (result_ptr, new_len).
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.str_scratch_ptr()));
                Ty::Str
            }
            // ── String eq ───────────────────────────────────────────────────────────────
            //
            // `s1.eq(s2)` returns `True` if both strings have the same
            // length and byte-for-byte content. Length-mismatch is the
            // fast-fail path; equal-length walks a byte-by-byte compare
            // loop. Pairs with `byteAt` to unblock parser-style code.
            ("eq", _) if recv_ty.is_str_like() && args.len() == 1 => {
                // Compile the other string. Stack ends as [ptr1, len1, ptr2, len2].
                let arg_ty = self.compile_expr(&args[0], scope, f);
                if !arg_ty.is_str_like() {
                    // Mismatched arg type — drop everything and return false.
                    self.drop_value(arg_ty, f);
                    self.drop_value(recv_ty, f);
                    f.instruction(&Instruction::I32Const(0));
                    return Ty::I32;
                }
                self.emit_str_eq(scope, f);
                Ty::I32
            }
            // ── String ordering ─────────────────────────────────────
            //
            // Byte-wise lexicographic comparison via `fn_str_cmp`
            // (-1/0/1), mirroring `Int`'s comparison surface. This is
            // the primitive behind user-side alphabetical ordering —
            // the same order the language enforces on declarations.
            ("lt" | "le" | "gt" | "ge" | "ne", _) if recv_ty.is_str_like() && args.len() == 1 => {
                let arg_ty = self.compile_expr(&args[0], scope, f);
                if !arg_ty.is_str_like() {
                    // Mismatched arg type — drop everything, return false.
                    self.drop_value(arg_ty, f);
                    self.drop_value(recv_ty, f);
                    f.instruction(&Instruction::I32Const(0));
                    return Ty::I32;
                }
                f.instruction(&Instruction::Call(self.fn_str_cmp));
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&match method {
                    "lt" => Instruction::I32LtS,
                    "le" => Instruction::I32LeS,
                    "gt" => Instruction::I32GtS,
                    "ge" => Instruction::I32GeS,
                    _ => Instruction::I32Ne,
                });
                Ty::I32
            }
            // ── List methods ───────────────────────────────────────────────────
            ("length", Ty::List) | ("length", Ty::NamedPtr(_)) => {
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
                // Real element-wise map when the argument is an inline
                // lambda with a supported element type. Canon lambdas
                // are non-capturing (the language has no local
                // variables), so the body is inlined straight into the
                // loop with the parameter's type name bound to the
                // current-element local. Anything else falls back to
                // the historical identity behaviour.
                if let Some(Expr::Lambda { params, body, .. }) = args.first() {
                    if params.len() == 1 {
                        if let TypeExpr::Named { name, .. } = &params[0].ty {
                            let name = name.clone();
                            let body = body.clone();
                            let elem = self.resolve_repr(&name);
                            if matches!(elem, Ty::I64 | Ty::Str | Ty::NamedStr(_)) {
                                return self.compile_list_map(&name, &elem, &body, scope, f);
                            }
                        }
                    }
                }
                // Identity fallback (unsupported element shapes).
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
            ("get", Ty::List) => {
                // list.get(i) -> Option — mirrors `first` but reads at
                // `list_ptr + i*8` after an unsigned bounds check
                // (negative indices wrap to huge u64s and fail it).
                //
                // Compile the index argument first — it is arbitrary
                // user code and may clobber every scratch local; the
                // receiver's (ptr, len) stays safe on the stack below
                // it.
                let idx_ty = self.compile_expr(&args[0], scope, f);
                if !matches!(idx_ty, Ty::I64) {
                    self.drop_value(idx_ty, f);
                    f.instruction(&Instruction::I64Const(1));
                }
                // 1-based: get(1) is the first element. `get(0)` shifts
                // to -1, wraps to a huge u64, and fails the unsigned
                // bounds check below — a clean `None`.
                f.instruction(&Instruction::I64Const(1));
                f.instruction(&Instruction::I64Sub);
                // Stack: [ptr, len, idx]. All user code is done; peel.
                f.instruction(&Instruction::LocalSet(scope.tmp_i64())); // idx
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // len
                f.instruction(&Instruction::LocalSet(scope.alloc_ptr())); // ptr
                                                                          // Allocate the Option struct.
                f.instruction(&Instruction::I32Const(12));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                // idx < len (unsigned, in i64 space)?
                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I64ExtendI32U);
                f.instruction(&Instruction::I64LtU);
                f.instruction(&Instruction::If(BlockType::Empty));
                // Some: tag=1, payload = i64 at ptr + idx*8.
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                f.instruction(&Instruction::I32WrapI64);
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::I32Mul);
                f.instruction(&Instruction::I32Add);
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
                // None: tag=0.
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
            // NOTE: `Map` / `Set` methods are NOT built in — they are
            // pure Canon (`canon/std/Map`, `canon/std/Set`) and resolve
            // through `func_table` in `compile_method_call` before the
            // builtin fallback ever fires.
            // ── List growth ──────────────────────────────────────────
            ("append", Ty::List) => {
                // Compile the element, then pack it into the 8-byte
                // slot the same way `build_list_literal` stores it:
                // i64 verbatim, strings as `ptr | len << 32`.
                let elem_ty = self.compile_expr(&args[0], scope, f);
                match elem_ty {
                    Ty::I64 => {}
                    ref t if t.is_str_like() => {
                        f.instruction(&Instruction::I64ExtendI32U); // len
                        f.instruction(&Instruction::I64Const(32));
                        f.instruction(&Instruction::I64Shl);
                        f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                        f.instruction(&Instruction::I64ExtendI32U); // ptr
                        f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                        f.instruction(&Instruction::I64Or);
                    }
                    Ty::F64 => {
                        f.instruction(&Instruction::I64ReinterpretF64);
                    }
                    Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                        f.instruction(&Instruction::I64ExtendI32U);
                    }
                    other => {
                        self.drop_value(other, f);
                        f.instruction(&Instruction::I64Const(0));
                    }
                }
                f.instruction(&Instruction::Call(self.fn_list_append));
                Ty::List
            }
            ("concat", Ty::List) => {
                let ty = self.compile_expr(&args[0], scope, f);
                if !matches!(ty, Ty::List) {
                    // Non-list arg — concat with the empty list.
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::I32Const(0));
                }
                f.instruction(&Instruction::Call(self.fn_list_concat));
                Ty::List
            }
            // `list.Json()` — conversion-is-construction spelling
            // (the language spec, docs/src/spec/) of "encode this list of
            // pre-rendered JSON values as a JSON array".
            ("Json", Ty::List) => {
                // Stack: [list_ptr, list_len]. Call the helper which
                // returns `(out_ptr, out_len)` of a freshly-allocated
                // string `[elem0,elem1,…,elemN]`. Each slot in the list
                // is read as `(i32 ptr, i32 len)` at offsets 0/4 — the
                // storage layout of `build_list_literal` for string
                // elements. Lists of `Int` / `Float` slots are
                // misinterpreted (their first 4 bytes would be read as
                // a ptr); we document that and rely on user code to
                // only call this on `List<String>`-shaped lists.
                f.instruction(&Instruction::Call(self.fn_list_to_json_array));
                Ty::Str
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
            // ── HTTP mode: request introspection ─────────────────────────────
            // `request.path()` — `[method]request.get-path-with-query`
            // returns `option<string>` through an indirect ret-area
            // (disc byte at +0, ptr/len at +4/+8). Re-shaped into a
            // Canon `Option` struct (i32 tag at +0, payload at +4/+8)
            // so the ordinary `(None, Some<String>)` dispatch works.
            ("path", Ty::NamedPtr(ref n)) if n == "Request" && self.http_mode => {
                // Stack: [request]. Methods take a borrow — passing our
                // own handle index is the standard convention.
                f.instruction(&Instruction::I32Const(MEM_HTTP_RET as i32));
                f.instruction(&Instruction::Call(FN_HTTP_GET_PATH));
                f.instruction(&Instruction::I32Const(12));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(MEM_HTTP_RET as i32));
                f.instruction(&Instruction::I32Load8U(MemArg {
                    offset: 0,
                    align: 0,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                for off in [4u64, 8] {
                    f.instruction(&Instruction::LocalGet(scope.rbool()));
                    f.instruction(&Instruction::I32Const(MEM_HTTP_RET as i32));
                    f.instruction(&Instruction::I32Load(MemArg {
                        offset: off,
                        align: 2,
                        memory_index: 0,
                    }));
                    f.instruction(&Instruction::I32Store(MemArg {
                        offset: off,
                        align: 2,
                        memory_index: 0,
                    }));
                }
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                Ty::NamedPtr("Option".to_string())
            }
            // `request.method()` — `[method]request.get-method` returns
            // the WIT `method` variant through a 12-byte ret area (disc
            // byte at +0; the `other(string)` payload at +4/+8). Canon
            // surfaces it as a plain `String` ("GET", "POST", …) so
            // routing is the same literal dispatch used for paths and
            // web-app messages — no 10-arm union dispatch at every call
            // site. Static cases map to interned strings; `other`
            // passes its payload through verbatim.
            ("method", Ty::NamedPtr(ref n)) if n == "Request" && self.http_mode => {
                // Stack: [request].
                f.instruction(&Instruction::I32Const(12));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalTee(scope.rbool()));
                f.instruction(&Instruction::Call(FN_HTTP_GET_METHOD));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Load8U(MemArg {
                    offset: 0,
                    align: 0,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                // Defaults to the `other` payload (valid when disc = 9,
                // overwritten below for every static discriminant).
                for (off, local) in [(4u64, scope.map_elem_ptr()), (8, scope.addr_scratch())] {
                    f.instruction(&Instruction::LocalGet(scope.rbool()));
                    f.instruction(&Instruction::I32Load(MemArg {
                        offset: off,
                        align: 2,
                        memory_index: 0,
                    }));
                    f.instruction(&Instruction::LocalSet(local));
                }
                // WIT declaration order (wit-vendor/wasi/http.wit).
                const METHOD_NAMES: [&str; 9] = [
                    "GET", "HEAD", "POST", "PUT", "DELETE", "CONNECT", "OPTIONS", "TRACE", "PATCH",
                ];
                for (disc, name) in METHOD_NAMES.iter().enumerate() {
                    let (ptr, len) = self.strings.intern(name);
                    f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                    f.instruction(&Instruction::I32Const(disc as i32));
                    f.instruction(&Instruction::I32Eq);
                    f.instruction(&Instruction::If(BlockType::Empty));
                    f.instruction(&Instruction::I32Const(ptr as i32));
                    f.instruction(&Instruction::LocalSet(scope.map_elem_ptr()));
                    f.instruction(&Instruction::I32Const(len as i32));
                    f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
                    f.instruction(&Instruction::End);
                }
                f.instruction(&Instruction::LocalGet(scope.map_elem_ptr()));
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                Ty::Str
            }
            // `headers.set(name, value)` — `[method]fields.append`. The
            // stdlib binds `set` to `append`: on a freshly-constructed
            // `fields` every `set` is the first write for its name, so
            // append gives set semantics with the simpler single-value
            // WIT shape. The `result<_, header-error>` lands in a fresh
            // 20-byte ret area (disc at +0, `other(option<string>)`
            // payload from +4) and is deliberately ignored — a rejected
            // name/value degrades to "header absent", the same posture
            // as `set-status-code`.
            ("set", Ty::NamedPtr(ref n)) if n == "Headers" && self.http_mode => {
                // Stack: [hdrs]. The two args are arbitrary user code —
                // park both strings on the operand stack before touching
                // any scratch local.
                for a in args.iter().take(2) {
                    let ty = self.compile_expr(a, scope, f);
                    if !ty.is_str_like() {
                        self.drop_value(ty, f);
                        f.instruction(&Instruction::I32Const(0));
                        f.instruction(&Instruction::I32Const(0));
                    }
                }
                for _ in args.len()..2 {
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::I32Const(0));
                }
                // Peel [hdrs, nptr, nlen, vptr, vlen] into locals — no
                // user code runs from here on.
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // vlen
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // vptr
                f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // nlen
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr())); // nptr
                f.instruction(&Instruction::LocalSet(scope.rbool())); // hdrs
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.map_elem_ptr()));
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(20));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::Call(FN_HTTP_FIELDS_APPEND));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                Ty::NamedPtr("Headers".to_string())
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

    /// Byte-wise string equality. Expects `[ptr1, len1, ptr2, len2]`
    /// (four i32s) on the operand stack; leaves a single i32 (0/1).
    /// Length mismatch is the fast-fail path; equal lengths walk a
    /// byte-by-byte compare loop. Clobbers `rptr`, `rlen`, `rbool`,
    /// `tmp_i32`, and `tmp_i32_b`. Shared by the `String.eq` builtin
    /// and string literal-dispatch compare chains.
    fn emit_str_eq(&self, scope: &LocalScope, f: &mut Function) {
        // Save into locals.
        f.instruction(&Instruction::LocalSet(scope.rlen())); // len2
        f.instruction(&Instruction::LocalSet(scope.rbool())); // ptr2
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // len1
        f.instruction(&Instruction::LocalSet(scope.rptr())); // ptr1
                                                             // If len1 != len2, push 0 and skip. Otherwise compare bytes.
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::Else);
        // Equal-length compare. Use tmp_i32_b as the running
        // result (1 = still-equal). Walk bytes; on mismatch,
        // set result=0 and break out.
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        // if len == 0: break
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        // if load8(p1) != load8(p2): result=0, break
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::I32Load8U(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Load8U(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        f.instruction(&Instruction::Br(2)); // break outer block
        f.instruction(&Instruction::End);
        // p1++, p2++, len--
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        f.instruction(&Instruction::Br(0)); // continue
        f.instruction(&Instruction::End); // end loop
        f.instruction(&Instruction::End); // end block
                                          // Push result.
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::End); // end outer if
    }

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

        // Literal-pattern dispatch on a String / Int scrutinee: an
        // equality-compare chain instead of a discriminant switch.
        if arms.iter().any(|a| a.literal.is_some()) {
            return self.emit_literal_dispatch(scrut_ty, arms, &arm_result_ty, scope, f);
        }

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
    /// Compile a literal-pattern dispatch: the scrutinee is stashed in
    /// the dedicated `lit_scrut_*` locals, each literal arm becomes one
    /// link of an equality if/else chain (string compare via
    /// `emit_str_eq`, int compare via `i64.eq`), and the mandatory
    /// catch-all arm sits in the innermost `else`. Inside every arm
    /// body the scrutinee is bound under the catch-all's pattern name
    /// and the scrutinee's own type name. The bare primitive name
    /// (`String`) is bound only when the scrutinee *is* a bare string —
    /// a newtype-wrapped scrutinee (`Prefix(msg.substring(1, 4))`)
    /// binds `Prefix`, leaving the enclosing function's `String` param
    /// visible in arm bodies; distinguishing the two is exactly why
    /// the user wrapped it.
    fn emit_literal_dispatch(
        &mut self,
        scrut_ty: Ty,
        arms: &[MatchArm],
        result_ty: &Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let catch_all = arms.iter().find(|a| a.literal.is_none());
        let lit_arms: Vec<&MatchArm> = arms.iter().filter(|a| a.literal.is_some()).collect();

        let mut bound_names: Vec<String> = Vec::new();
        if let Some(arm) = catch_all {
            if let Some(n) = arm_type_name(arm) {
                bound_names.push(n.to_string());
            }
        }
        if let Some(n) = scrut_ty.canon_name() {
            bound_names.push(n.to_string());
        }

        if scrut_ty.is_str_like() {
            if scrut_ty.canon_name().is_none() {
                bound_names.push("String".to_string());
            }
            // Stash the scrutinee (ptr, len) where neither the compare
            // scratch nor arm bodies' builtins will clobber it.
            f.instruction(&Instruction::LocalSet(scope.lit_scrut_ptr() + 1));
            f.instruction(&Instruction::LocalSet(scope.lit_scrut_ptr()));
            let mut arm_scope = scope.clone();
            for n in &bound_names {
                arm_scope
                    .vars
                    .insert(n.clone(), (scope.lit_scrut_ptr(), scrut_ty.clone()));
            }
            for arm in &lit_arms {
                match &arm.literal {
                    Some(ArmLiteral::Str(value)) => {
                        let (lptr, llen) = self.strings.intern(value);
                        f.instruction(&Instruction::LocalGet(scope.lit_scrut_ptr()));
                        f.instruction(&Instruction::LocalGet(scope.lit_scrut_ptr() + 1));
                        f.instruction(&Instruction::I32Const(lptr as i32));
                        f.instruction(&Instruction::I32Const(llen as i32));
                        self.emit_str_eq(scope, f);
                    }
                    // Kind mismatch is a checker error; emit a
                    // never-taken link so the chain stays well-formed.
                    _ => {
                        f.instruction(&Instruction::I32Const(0));
                    }
                }
                f.instruction(&Instruction::If(BlockType::Empty));
                self.compile_arm_body_prebound(arm, result_ty, &arm_scope, f);
                f.instruction(&Instruction::Else);
            }
            if let Some(arm) = catch_all {
                self.compile_arm_body_prebound(arm, result_ty, &arm_scope, f);
            }
            for _ in 0..lit_arms.len() {
                f.instruction(&Instruction::End);
            }
            return self.load_result(result_ty, scope, f);
        }

        if scrut_ty == Ty::I64 {
            bound_names.push("Int".to_string());
            f.instruction(&Instruction::LocalSet(scope.lit_scrut_i64()));
            let mut arm_scope = scope.clone();
            for n in &bound_names {
                arm_scope
                    .vars
                    .insert(n.clone(), (scope.lit_scrut_i64(), scrut_ty.clone()));
            }
            for arm in &lit_arms {
                match &arm.literal {
                    Some(ArmLiteral::Int(v)) => {
                        f.instruction(&Instruction::LocalGet(scope.lit_scrut_i64()));
                        f.instruction(&Instruction::I64Const(*v));
                        f.instruction(&Instruction::I64Eq);
                    }
                    _ => {
                        f.instruction(&Instruction::I32Const(0));
                    }
                }
                f.instruction(&Instruction::If(BlockType::Empty));
                self.compile_arm_body_prebound(arm, result_ty, &arm_scope, f);
                f.instruction(&Instruction::Else);
            }
            if let Some(arm) = catch_all {
                self.compile_arm_body_prebound(arm, result_ty, &arm_scope, f);
            }
            for _ in 0..lit_arms.len() {
                f.instruction(&Instruction::End);
            }
            return self.load_result(result_ty, scope, f);
        }

        // Unsupported scrutinee shape — the checker has already
        // reported it; keep the stack balanced.
        self.drop_value(scrut_ty, f);
        Ty::Unit
    }

    fn compile_arm_body(
        &mut self,
        arm: &MatchArm,
        result_ty: &Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) {
        let arm_scope = self.bind_arm_payload(&arm.param_ty, scope, f);
        self.compile_arm_body_prebound(arm, result_ty, &arm_scope, f);
    }

    /// Body of `compile_arm_body` after payload binding: compile the
    /// arm's block in an already-prepared scope and save the result to
    /// the shared scratch locals. Literal dispatch calls this directly —
    /// its scrutinee binding replaces the union payload extraction.
    fn compile_arm_body_prebound(
        &mut self,
        arm: &MatchArm,
        result_ty: &Ty,
        arm_scope: &LocalScope,
        f: &mut Function,
    ) {
        let scope = arm_scope;
        let body = arm.body.clone();
        let ty = self.compile_block_return(&body, scope, f);
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
            Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) | Ty::Ptr => match ty {
                Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) | Ty::Ptr => {
                    f.instruction(&Instruction::LocalSet(scope.rbool()));
                }
                _ => {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::LocalSet(scope.rbool()));
                }
            },
            Ty::F64 => match ty {
                Ty::F64 => {
                    f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
                }
                _ => {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::F64Const(0.0.into()));
                    f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
                }
            },
            // A List result is a (ptr, count) pair — same two-i32 shape
            // as a string, parked in the same rptr/rlen scratch pair.
            Ty::List => match ty {
                Ty::List => {
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
            Ty::I64 => {
                // Load i64 at +4 into tmp_i64 and bind the arm's name
                // to that local. Variant payloads of `Int` user-newtype
                // (or the primitive directly) use the same 8-byte slot
                // at offset 4 of the union struct — see
                // `build_union_value` and `store_value_at_offset`.
                f.instruction(&Instruction::LocalGet(base_scope.alloc_ptr()));
                f.instruction(&Instruction::I64Load(MemArg {
                    offset: 4,
                    align: 3,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(base_scope.tmp_i64()));
                scope
                    .vars
                    .insert(bound_name, (base_scope.tmp_i64(), payload_ty));
            }
            Ty::F64 => {
                // Same slot as I64, but through the f64-typed scratch —
                // wasm locals are monomorphic, so a `Float` payload
                // can't be bound to `tmp_i64`.
                f.instruction(&Instruction::LocalGet(base_scope.alloc_ptr()));
                f.instruction(&Instruction::F64Load(MemArg {
                    offset: 4,
                    align: 3,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(base_scope.tmp_f64()));
                scope
                    .vars
                    .insert(bound_name, (base_scope.tmp_f64(), payload_ty));
            }
            Ty::I32 => {
                // Bool / discriminant-style payload at +4.
                f.instruction(&Instruction::LocalGet(base_scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(base_scope.rbool()));
                scope
                    .vars
                    .insert(bound_name, (base_scope.rbool(), payload_ty));
            }
            Ty::Ptr | Ty::NamedPtr(_) => {
                // Boxed product payload (auto-boxed by
                // `build_union_value` for multi-field product variants,
                // or a single pointer payload): the union stores one
                // pointer at +4. Bind it in the string pair's first
                // slot — dedicated, so arm-body builtins that use the
                // ordinary scratch locals can't clobber it — and field
                // access on the bound name (`Link.Label`) reads through
                // `product_field_layout` as usual.
                f.instruction(&Instruction::LocalGet(base_scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(base_scope.arm_payload_ptr()));
                scope
                    .vars
                    .insert(bound_name, (base_scope.arm_payload_ptr(), payload_ty));
            }
            _ => {
                // List payloads not yet bound.
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
        // Zero-data variants (`Stop`, `Empty` — a variant with no
        // typedef of its own) carry nothing to bind. Without this
        // guard their repr resolves to `NamedPtr(parent)` through the
        // `variant_parent` arm of `resolve_repr` and the pointer case
        // above would bind garbage read from offset 4.
        if !self.type_defs.contains_key(name) && self.variant_parent.contains_key(name) {
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
            Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                result_ty.clone()
            }
            Ty::F64 => {
                f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
                Ty::F64
            }
            Ty::List => {
                f.instruction(&Instruction::LocalGet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.rlen()));
                Ty::List
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
            Ty::I64 => {
                f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
            }
            Ty::F64 => {
                f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
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
                f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
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

    /// Store a single payload value into the struct at `address + offset`,
    /// where `address` is taken from the operand stack (NOT from
    /// `scope.alloc_ptr()` — that local is clobbered whenever the value
    /// expression contains a nested constructor).
    ///
    /// Stack contract on entry depends on `payload_ty`:
    ///   * Scalars (`Ty::I64`/`F64`/`I32`/`Ptr`/`NamedPtr`/`NamedPtrStr`):
    ///     `[address, value]` — one i32/i64 `store` consumes both.
    ///   * Strings (`Ty::Str`/`NamedStr`): `[address, ptr, len]` — two
    ///     i32 stores: `ptr` at `offset` and `len` at `offset + 4`,
    ///     both against the on-stack address.
    ///   * `Ty::Unit`: just drops the address. There's no payload.
    fn store_payload_at_offset(
        &self,
        offset: u32,
        payload_ty: &Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) {
        match payload_ty {
            Ty::I64 => {
                f.instruction(&Instruction::I64Store(MemArg {
                    offset: offset as u64,
                    align: 3,
                    memory_index: 0,
                }));
            }
            Ty::F64 => {
                f.instruction(&Instruction::F64Store(MemArg {
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
                // Stack: [addr, ptr, len]. Stash ptr+len in `tmp_i32`/
                // `tmp_i32_b` (`rptr`/`rlen` may still hold the value
                // the caller pushed via `load_from_scratch`), stash the
                // on-stack addr in `addr_scratch`, then emit the two
                // stores against it. No re-load of `alloc_ptr`: the
                // on-stack address is the only one guaranteed to point
                // at the struct being built when the payload expression
                // contained nested allocations.
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // len
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // ptr
                f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
                // Store ptr at +offset
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: offset as u64,
                    align: 2,
                    memory_index: 0,
                }));
                // Store len at +offset+4
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
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
                f.instruction(&Instruction::Call(self.fn_print_float));
            }
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
        // `subtask.cancel` has signature `(i32) -> (i32)` — takes a
        // subtask handle, returns the new CallState. Used by `race`'s
        // loser-cancel path.
        let ty_subtask_cancel = self.get_or_add_wasm_type(&[ValType::I32], &[ValType::I32]);
        // Reserve the wasm type for the list-to-json-array helper:
        // `(i32, i32) -> (i32, i32)`. Must be registered *before* the
        // type section is emitted below; the function section uses the
        // returned absolute index.
        let list_to_json_array_ty =
            self.get_or_add_wasm_type(&[ValType::I32, ValType::I32], &[ValType::I32, ValType::I32]);
        // Reserve the wasm type for the float printer: `(f64) -> ()`.
        let print_float_ty = self.get_or_add_wasm_type(&[ValType::F64], &[]);
        // Int→String renderer: `(i64) -> (i32, i32)`; string compare:
        // `(ptr1, len1, ptr2, len2) -> i32`.
        let int_to_str_ty =
            self.get_or_add_wasm_type(&[ValType::I64], &[ValType::I32, ValType::I32]);
        let str_cmp_ty = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32]);
        // Map + list-growth helper shapes.
        let list_append_ty = self.get_or_add_wasm_type(
            &[ValType::I32, ValType::I32, ValType::I64],
            &[ValType::I32; 2],
        );
        let list_concat_ty = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32; 2]);
        // Reserve the loop block type used by `compile_list_map`:
        // `(src, dst, remaining) -> (src, dst, remaining)`, all i32.
        // Block types must exist in the type section, which is emitted
        // before any user function body is compiled.
        let _list_map_loop_ty = self.get_or_add_wasm_type(
            &[ValType::I32, ValType::I32, ValType::I32],
            &[ValType::I32, ValType::I32, ValType::I32],
        );

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
        //   - canon:async/waitable.*: 6 canonical async/task helpers
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
            "canon:async/waitable",
            "set-new",
            EntityType::Function(TY_HANDLE_RETURN), // () -> i32
        );
        imports.import(
            "canon:async/waitable",
            "join",
            EntityType::Function(TY_PRINT_STR), // (i32, i32) -> ()
        );
        imports.import(
            "canon:async/waitable",
            "set-wait",
            EntityType::Function(ty_waitable_set_wait), // (i32, i32) -> i32
        );
        imports.import(
            "canon:async/waitable",
            "set-drop",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        imports.import(
            "canon:async/waitable",
            "subtask-drop",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        imports.import(
            "canon:async/waitable",
            "task-return",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> () — result<_,_> tag
        );
        imports.import(
            "canon:async/waitable",
            "subtask-cancel",
            EntityType::Function(ty_subtask_cancel), // (i32) -> (i32)
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
        funcs.function(list_to_json_array_ty); // list → json array helper
        funcs.function(print_float_ty); // float printer (f64) -> ()
        funcs.function(int_to_str_ty); // Int→String renderer (i64) -> (i32, i32)
        funcs.function(str_cmp_ty); // string compare -> -1/0/1
        funcs.function(list_append_ty); // list append
        funcs.function(list_concat_ty); // list concat
                                        // User-compiled functions only — extern imports are already declared
                                        // in the import section and must NOT get a defined-function slot.
                                        // `compiled_user_funcs` is the single source of truth shared
                                        // with the code section below: one entry per compiled body, in
                                        // func-index order, immune to `func_table` key collisions
                                        // (constructor families register several bodies per name).
        for (_, type_idx, _) in &self.compiled_user_funcs {
            funcs.function(*type_idx);
        }
        m.section(&funcs);

        // ── Memory section ───────────────────────────────────────────────────────────────
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

        // ── Code section ─────────────────────────────────────────────────────────────
        let mut codes = CodeSection::new();
        codes.function(&self.build_print_str());
        codes.function(&self.build_print_int());
        codes.function(&self.build_print_bool());
        codes.function(&self.build_alloc());
        codes.function(&self.build_start());
        codes.function(&self.build_list_to_json_array());
        codes.function(&self.build_print_float());
        codes.function(&self.build_int_to_str());
        codes.function(&self.build_str_cmp());
        codes.function(&self.build_list_append());
        codes.function(&self.build_list_concat());
        // User functions — one body per `compiled_user_funcs` entry, in
        // func-index order (matches the function section above exactly).
        let ordered_funcs: Vec<FunctionDef> = self
            .compiled_user_funcs
            .iter()
            .map(|(_, _, func)| func.clone())
            .collect();
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

    /// HTTP encoder mode counterpart of `compile()`.
    ///
    /// Emits a *self-contained* core module for
    /// `wit_component::ComponentEncoder`: own memory + bump global +
    /// exported `cabi_realloc`, imports named per `wit-component`'s
    /// mangling conventions (stdout intrinsics hang off
    /// `write-via-stream`, http intrinsics off `[static]response.new`),
    /// and the entry exported as `wasi:http/handler@…#handle`. The
    /// hand-rolled component path (`compile()` + `component::wrap`)
    /// stays in place for the CLI world.
    fn compile_http(&mut self) -> Vec<u8> {
        use crate::ast::{entry_world_of, EntryWorld};

        // Pre-passes — identical to `compile()`.
        self.build_type_defs();
        self.build_variant_info();
        self.collect_all_strings();
        self.assign_func_indices();
        // Dynamic type registrations shared with `compile()` plus the
        // two http-specific shapes.
        let ty_i32x2_to_i32 =
            self.get_or_add_wasm_type(&[ValType::I32, ValType::I32], &[ValType::I32]);
        let list_to_json_array_ty =
            self.get_or_add_wasm_type(&[ValType::I32, ValType::I32], &[ValType::I32, ValType::I32]);
        let print_float_ty = self.get_or_add_wasm_type(&[ValType::F64], &[]);
        let int_to_str_ty =
            self.get_or_add_wasm_type(&[ValType::I64], &[ValType::I32, ValType::I32]);
        let str_cmp_ty = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32]);
        let list_append_ty = self.get_or_add_wasm_type(
            &[ValType::I32, ValType::I32, ValType::I64],
            &[ValType::I32; 2],
        );
        let list_concat_ty = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32; 2]);
        let _list_map_loop_ty = self.get_or_add_wasm_type(
            &[ValType::I32, ValType::I32, ValType::I32],
            &[ValType::I32, ValType::I32, ValType::I32],
        );
        let ty_response_new = self.get_or_add_wasm_type(&[ValType::I32; 5], &[]);
        let ty_fields_append = self.get_or_add_wasm_type(&[ValType::I32; 6], &[]);
        let ty_task_return_handle = self.get_or_add_wasm_type(
            &[
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I64,
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I32,
            ],
            &[],
        );
        let ty_cabi_realloc = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32]);

        // The entry function: the free `(Request) -> Response` the
        // checker validated. Its compiled index feeds the wrapper.
        let entry_name = self
            .ast
            .items
            .iter()
            .find_map(|item| match item {
                Item::Function(func)
                    if func.receiver.is_none()
                        && entry_world_of(&func.return_ty) == Some(EntryWorld::Http) =>
                {
                    Some(func.name.name.clone())
                }
                _ => None,
            })
            .expect("checker guarantees an HTTP entry exists");
        let user_fn_idx = self
            .func_table
            .get(&(None, entry_name.clone()))
            .map(|info| info.func_idx)
            .unwrap_or_else(|| panic!("HTTP entry `{entry_name}` missing from func table"));

        let mut m = Module::new();

        // ── Type section — same fixed TY_* prefix as `compile()` ─────
        let mut types = TypeSection::new();
        types.ty().function([ValType::I32, ValType::I32], []); // 0
        types.ty().function([ValType::I64], []); // 1
        types.ty().function([ValType::I32], []); // 2
        types.ty().function([], []); // 3
        types.ty().function([ValType::I32], [ValType::I32]); // 4
        types.ty().function([ValType::I32], [ValType::I32]); // 5
        types.ty().function([], [ValType::I64]); // 6
        types
            .ty()
            .function([ValType::I32, ValType::I32, ValType::I32], [ValType::I32]); // 7
        types.ty().function([], [ValType::I32]); // 8
        let user_sigs: Vec<_> = self.user_type_sigs.clone();
        for (params, results) in &user_sigs {
            types
                .ty()
                .function(params.iter().cloned(), results.iter().cloned());
        }
        m.section(&types);

        // ── Import section (indices fixed — see FN_HTTP_*) ───────────
        const STDOUT_MODULE: &str = "wasi:cli/stdout@0.3.0-rc-2026-03-15";
        let mut imports = ImportSection::new();
        imports.import(
            STDOUT_MODULE,
            "write-via-stream",
            EntityType::Function(TY_STDOUT_WRITE_VIA_STREAM),
        );
        imports.import(
            STDOUT_MODULE,
            "[stream-new-0]write-via-stream",
            EntityType::Function(TY_STDOUT_STREAM_NEW),
        );
        imports.import(
            STDOUT_MODULE,
            "[stream-write-0]write-via-stream",
            EntityType::Function(TY_STDOUT_STREAM_WRITE),
        );
        imports.import(
            STDOUT_MODULE,
            "[stream-drop-writable-0]write-via-stream",
            EntityType::Function(TY_PRINT_BOOL),
        );
        imports.import(
            STDOUT_MODULE,
            "[future-drop-readable-1]write-via-stream",
            EntityType::Function(TY_PRINT_BOOL),
        );
        let http = component::WASI_HTTP_TYPES_MODULE;
        imports.import(
            http,
            "[constructor]fields",
            EntityType::Function(TY_HANDLE_RETURN),
        );
        imports.import(
            http,
            "[static]response.new",
            EntityType::Function(ty_response_new),
        );
        imports.import(
            http,
            "[future-new-1][static]response.new",
            EntityType::Function(TY_STDOUT_STREAM_NEW),
        );
        imports.import(
            http,
            "[future-write-1][static]response.new",
            EntityType::Function(ty_i32x2_to_i32),
        );
        imports.import(
            http,
            "[future-drop-readable-2][static]response.new",
            EntityType::Function(TY_PRINT_BOOL),
        );
        imports.import(
            http,
            "[future-drop-writable-1][static]response.new",
            EntityType::Function(TY_PRINT_BOOL),
        );
        imports.import(
            http,
            "[resource-drop]request",
            EntityType::Function(TY_PRINT_BOOL),
        );
        imports.import(
            http,
            "[method]response.set-status-code",
            EntityType::Function(ty_i32x2_to_i32),
        );
        imports.import(
            http,
            "[stream-new-0][static]response.new",
            EntityType::Function(TY_STDOUT_STREAM_NEW),
        );
        imports.import(
            http,
            "[stream-write-0][static]response.new",
            EntityType::Function(TY_STDOUT_STREAM_WRITE),
        );
        imports.import(
            http,
            "[stream-drop-writable-0][static]response.new",
            EntityType::Function(TY_PRINT_BOOL),
        );
        // task.return for the async-stackful `handle` lift. The
        // `result<own<response>, error-code>` result lowers flat to
        // the *joined* slots of both arms:
        // (disc, own-handle/err-disc, then error-code's joined payload
        // slots i32,i64,i32,i32,i32,i32). The ok arm only uses the
        // first two; the rest are padding zeros.
        imports.import(
            "[export]wasi:http/handler@0.3.0-rc-2026-03-15",
            "[task-return]handle",
            EntityType::Function(ty_task_return_handle),
        );
        imports.import(
            http,
            "[method]request.get-path-with-query",
            EntityType::Function(TY_PRINT_STR),
        );
        imports.import(
            http,
            "[method]fields.append",
            EntityType::Function(ty_fields_append),
        );
        imports.import(
            http,
            "[method]request.get-method",
            EntityType::Function(TY_PRINT_STR),
        );
        m.section(&imports);

        // ── Function section ─────────────────────────────────────────
        let mut funcs = FunctionSection::new();
        funcs.function(TY_PRINT_STR);
        funcs.function(TY_PRINT_INT);
        funcs.function(TY_PRINT_BOOL);
        funcs.function(TY_ALLOC);
        funcs.function(TY_PRINT_BOOL); // fn_start slot = handle wrapper, (i32) -> ()
        funcs.function(list_to_json_array_ty);
        funcs.function(print_float_ty);
        funcs.function(int_to_str_ty);
        funcs.function(str_cmp_ty);
        funcs.function(list_append_ty);
        funcs.function(list_concat_ty);
        for (_, type_idx, _) in &self.compiled_user_funcs {
            funcs.function(*type_idx);
        }
        funcs.function(ty_cabi_realloc); // cabi_realloc, appended last
        m.section(&funcs);

        // ── Memory / globals: self-contained ─────────────────────────
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: 2,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        m.section(&memories);
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

        // ── Exports ──────────────────────────────────────────────────
        let cabi_realloc_idx = self.fn_user_start + self.compiled_user_funcs.len() as u32;
        let mut exports = ExportSection::new();
        exports.export("memory", ExportKind::Memory, 0);
        exports.export("cabi_realloc", ExportKind::Func, cabi_realloc_idx);
        exports.export(
            &format!(
                "[async-lift-stackful]{}#handle",
                component::WASI_HTTP_HANDLER
            ),
            ExportKind::Func,
            self.fn_start,
        );
        m.section(&exports);

        // ── Code section — order must match the function section ─────
        let mut codes = CodeSection::new();
        codes.function(&self.build_print_str());
        codes.function(&self.build_print_int());
        codes.function(&self.build_print_bool());
        codes.function(&self.build_alloc());
        codes.function(&self.build_http_handle_wrapper(user_fn_idx));
        codes.function(&self.build_list_to_json_array());
        codes.function(&self.build_print_float());
        codes.function(&self.build_int_to_str());
        codes.function(&self.build_str_cmp());
        codes.function(&self.build_list_append());
        codes.function(&self.build_list_concat());
        let ordered_funcs: Vec<FunctionDef> = self
            .compiled_user_funcs
            .iter()
            .map(|(_, _, func)| func.clone())
            .collect();
        for func in ordered_funcs {
            let compiled = self.build_user_function(&func);
            codes.function(&compiled);
        }
        codes.function(&self.build_cabi_realloc());
        m.section(&codes);

        // ── Data ─────────────────────────────────────────────────────
        let mut data = DataSection::new();
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

    /// Web encoder mode (the web target, docs/src/reference/web-target.md): emits a self-contained core
    /// module (own memory, own bump global) exporting the Elm-triple
    /// ABI the bundled JS host (`canon-web.js`) drives:
    ///
    ///   init()                       -> i64        opaque model
    ///   update(model, msg_ptr, len)  -> i64        msg is UTF-8 in guest memory
    ///   view(model)                  -> (i32, i32) UTF-8 HTML (ptr, len)
    ///   alloc(size)                  -> i32        lets JS place the msg bytes
    ///   memory
    ///
    /// The model is whatever the user's `init` returns, normalized to
    /// one opaque i64 the host threads back into `update`/`view` (see
    /// `WebModelShape`). The only imports are the five stdout print
    /// intrinsics, which the JS host maps onto `console.log` — so
    /// `.print()` debugging works in the browser console.
    fn compile_web(&mut self) -> Vec<u8> {
        // Pre-passes — identical to `compile()`.
        self.build_type_defs();
        self.build_variant_info();
        self.collect_all_strings();
        self.assign_func_indices();
        let web = crate::ast::find_web_entry(&self.ast.items)
            .expect("checker guarantees a web entry exists");
        let model = web.model;

        // Dynamic type registrations shared with `compile()` plus the
        // three wrapper shapes.
        let list_to_json_array_ty =
            self.get_or_add_wasm_type(&[ValType::I32, ValType::I32], &[ValType::I32, ValType::I32]);
        let print_float_ty = self.get_or_add_wasm_type(&[ValType::F64], &[]);
        let int_to_str_ty =
            self.get_or_add_wasm_type(&[ValType::I64], &[ValType::I32, ValType::I32]);
        let str_cmp_ty = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32]);
        let list_append_ty = self.get_or_add_wasm_type(
            &[ValType::I32, ValType::I32, ValType::I64],
            &[ValType::I32; 2],
        );
        let list_concat_ty = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32; 2]);
        let _list_map_loop_ty = self.get_or_add_wasm_type(
            &[ValType::I32, ValType::I32, ValType::I32],
            &[ValType::I32, ValType::I32, ValType::I32],
        );
        let ty_init_wrapper = self.get_or_add_wasm_type(&[], &[ValType::I64]);
        let ty_update_wrapper =
            self.get_or_add_wasm_type(&[ValType::I64, ValType::I32, ValType::I32], &[ValType::I64]);
        let ty_view_wrapper =
            self.get_or_add_wasm_type(&[ValType::I64], &[ValType::I32, ValType::I32]);

        // The entry triple's compiled indices and the model's flat
        // core shape (from `init`'s result signature).
        let init_info = self
            .func_table
            .get(&web.init)
            .cloned()
            .expect("web entry `init` missing from func table");
        let update_info = self
            .func_table
            .get(&web.update)
            .cloned()
            .expect("web entry `update` missing from func table");
        let view_info = self
            .func_table
            .get(&web.view)
            .cloned()
            .expect("web entry `view` missing from func table");
        let sig_of = |type_idx: u32| -> &(Vec<ValType>, Vec<ValType>) {
            &self.user_type_sigs[(type_idx - TY_USER_START) as usize]
        };
        let init_results = sig_of(init_info.type_idx).1.clone();
        let model_shape = match init_results.as_slice() {
            [ValType::I64] => WebModelShape::I64,
            [ValType::F64] => WebModelShape::F64,
            [ValType::I32] => WebModelShape::Ptr,
            [ValType::I32, ValType::I32] => WebModelShape::Str,
            other => {
                eprintln!(
                    "error: unsupported web model shape {other:?}: the model must be a \
                     product, union, Int, Float, or String-aliased type"
                );
                std::process::exit(1);
            }
        };
        let model_flat: &[ValType] = match model_shape {
            WebModelShape::I64 => &[ValType::I64],
            WebModelShape::F64 => &[ValType::F64],
            WebModelShape::Ptr => &[ValType::I32],
            WebModelShape::Str => &[ValType::I32, ValType::I32],
        };
        let (update_params, update_results) = sig_of(update_info.type_idx).clone();
        let expected_update: Vec<ValType> = model_flat
            .iter()
            .chain(&[ValType::I32, ValType::I32])
            .cloned()
            .collect();
        if update_params != expected_update || update_results != init_results {
            eprintln!(
                "error: web entry shape mismatch: `update` must be \
                 `({model} * String) -> {model}` with the same model type `init` returns"
            );
            std::process::exit(1);
        }
        let (view_params, view_results) = sig_of(view_info.type_idx).clone();
        if view_params != model_flat || view_results != [ValType::I32, ValType::I32] {
            eprintln!(
                "error: web entry shape mismatch: `view` must be `({model}) -> Html` \
                 with the same model type `init` returns"
            );
            std::process::exit(1);
        }

        let mut m = Module::new();

        // ── Type section — same fixed TY_* prefix as `compile()` ─────
        let mut types = TypeSection::new();
        types.ty().function([ValType::I32, ValType::I32], []); // 0
        types.ty().function([ValType::I64], []); // 1
        types.ty().function([ValType::I32], []); // 2
        types.ty().function([], []); // 3
        types.ty().function([ValType::I32], [ValType::I32]); // 4
        types.ty().function([ValType::I32], [ValType::I32]); // 5
        types.ty().function([], [ValType::I64]); // 6
        types
            .ty()
            .function([ValType::I32, ValType::I32, ValType::I32], [ValType::I32]); // 7
        types.ty().function([], [ValType::I32]); // 8
        let user_sigs: Vec<_> = self.user_type_sigs.clone();
        for (params, results) in &user_sigs {
            types
                .ty()
                .function(params.iter().cloned(), results.iter().cloned());
        }
        m.section(&types);

        // ── Import section: the five stdout print intrinsics only ────
        const STDOUT_MODULE: &str = "wasi:cli/stdout@0.3.0-rc-2026-03-15";
        let mut imports = ImportSection::new();
        imports.import(
            STDOUT_MODULE,
            "write-via-stream",
            EntityType::Function(TY_STDOUT_WRITE_VIA_STREAM),
        );
        imports.import(
            STDOUT_MODULE,
            "[stream-new-0]write-via-stream",
            EntityType::Function(TY_STDOUT_STREAM_NEW),
        );
        imports.import(
            STDOUT_MODULE,
            "[stream-write-0]write-via-stream",
            EntityType::Function(TY_STDOUT_STREAM_WRITE),
        );
        imports.import(
            STDOUT_MODULE,
            "[stream-drop-writable-0]write-via-stream",
            EntityType::Function(TY_PRINT_BOOL),
        );
        imports.import(
            STDOUT_MODULE,
            "[future-drop-readable-1]write-via-stream",
            EntityType::Function(TY_PRINT_BOOL),
        );
        m.section(&imports);

        // ── Function section — order matches the WEB_BASE_DEFINED map ─
        let mut funcs = FunctionSection::new();
        funcs.function(TY_PRINT_STR);
        funcs.function(TY_PRINT_INT);
        funcs.function(TY_PRINT_BOOL);
        funcs.function(TY_ALLOC);
        funcs.function(ty_init_wrapper);
        funcs.function(ty_update_wrapper);
        funcs.function(ty_view_wrapper);
        funcs.function(list_to_json_array_ty);
        funcs.function(print_float_ty);
        funcs.function(int_to_str_ty);
        funcs.function(str_cmp_ty);
        funcs.function(list_append_ty);
        funcs.function(list_concat_ty);
        for (_, type_idx, _) in &self.compiled_user_funcs {
            funcs.function(*type_idx);
        }
        m.section(&funcs);

        // ── Memory / globals: self-contained ─────────────────────────
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: 2,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        m.section(&memories);
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

        // ── Exports — the JS-host ABI ────────────────────────────────
        let mut exports = ExportSection::new();
        exports.export("memory", ExportKind::Memory, 0);
        exports.export("alloc", ExportKind::Func, self.fn_alloc);
        exports.export("init", ExportKind::Func, self.fn_start);
        exports.export("update", ExportKind::Func, self.fn_start + 1);
        exports.export("view", ExportKind::Func, self.fn_start + 2);
        m.section(&exports);

        // ── Code section — order must match the function section ─────
        let mut codes = CodeSection::new();
        codes.function(&self.build_print_str());
        codes.function(&self.build_print_int());
        codes.function(&self.build_print_bool());
        codes.function(&self.build_alloc());
        codes.function(&self.build_web_init_wrapper(init_info.func_idx, model_shape));
        codes.function(&self.build_web_update_wrapper(update_info.func_idx, model_shape));
        codes.function(&self.build_web_view_wrapper(view_info.func_idx, model_shape));
        codes.function(&self.build_list_to_json_array());
        codes.function(&self.build_print_float());
        codes.function(&self.build_int_to_str());
        codes.function(&self.build_str_cmp());
        codes.function(&self.build_list_append());
        codes.function(&self.build_list_concat());
        let ordered_funcs: Vec<FunctionDef> = self
            .compiled_user_funcs
            .iter()
            .map(|(_, _, func)| func.clone())
            .collect();
        for func in ordered_funcs {
            let compiled = self.build_user_function(&func);
            codes.function(&compiled);
        }
        m.section(&codes);

        // ── Data ─────────────────────────────────────────────────────
        let mut data = DataSection::new();
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

    /// Normalize the model value(s) on the stack into the opaque i64
    /// handed to the JS host. `base` is the index of three scratch i32
    /// locals the caller declared (used only for the `Str` boxing).
    fn emit_web_model_wrap(&self, f: &mut Function, shape: WebModelShape, base: u32) {
        let mem = MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        };
        match shape {
            WebModelShape::I64 => {}
            WebModelShape::Ptr => {
                f.instruction(&Instruction::I64ExtendI32U);
            }
            WebModelShape::F64 => {
                f.instruction(&Instruction::I64ReinterpretF64);
            }
            WebModelShape::Str => {
                // [ptr, len] → box into a fresh 8-byte cell.
                f.instruction(&Instruction::LocalSet(base + 1)); // len
                f.instruction(&Instruction::LocalSet(base)); // ptr
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalTee(base + 2));
                f.instruction(&Instruction::LocalGet(base));
                f.instruction(&Instruction::I32Store(mem));
                f.instruction(&Instruction::LocalGet(base + 2));
                f.instruction(&Instruction::LocalGet(base + 1));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(base + 2));
                f.instruction(&Instruction::I64ExtendI32U);
            }
        }
    }

    /// Push the model back in its user-function shape from the opaque
    /// i64 in local `model_local`. `base` as in `emit_web_model_wrap`.
    fn emit_web_model_unwrap(
        &self,
        f: &mut Function,
        shape: WebModelShape,
        model_local: u32,
        base: u32,
    ) {
        match shape {
            WebModelShape::I64 => {
                f.instruction(&Instruction::LocalGet(model_local));
            }
            WebModelShape::Ptr => {
                f.instruction(&Instruction::LocalGet(model_local));
                f.instruction(&Instruction::I32WrapI64);
            }
            WebModelShape::F64 => {
                f.instruction(&Instruction::LocalGet(model_local));
                f.instruction(&Instruction::F64ReinterpretI64);
            }
            WebModelShape::Str => {
                f.instruction(&Instruction::LocalGet(model_local));
                f.instruction(&Instruction::I32WrapI64);
                f.instruction(&Instruction::LocalSet(base + 2));
                f.instruction(&Instruction::LocalGet(base + 2));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(base + 2));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
            }
        }
    }

    /// `init() -> i64` — call the user's `init`, normalize the model.
    fn build_web_init_wrapper(&self, init_idx: u32, shape: WebModelShape) -> Function {
        let mut f = Function::new([(3, ValType::I32)]); // locals 0..2 (no params)
        f.instruction(&Instruction::Call(init_idx));
        self.emit_web_model_wrap(&mut f, shape, 0);
        f.instruction(&Instruction::End);
        f
    }

    /// `update(model: i64, msg_ptr: i32, msg_len: i32) -> i64`.
    fn build_web_update_wrapper(&self, update_idx: u32, shape: WebModelShape) -> Function {
        let mut f = Function::new([(3, ValType::I32)]); // locals 3..5 after params
        self.emit_web_model_unwrap(&mut f, shape, 0, 3);
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::Call(update_idx));
        self.emit_web_model_wrap(&mut f, shape, 3);
        f.instruction(&Instruction::End);
        f
    }

    /// `view(model: i64) -> (i32, i32)` — UTF-8 HTML (ptr, len).
    fn build_web_view_wrapper(&self, view_idx: u32, shape: WebModelShape) -> Function {
        let mut f = Function::new([(3, ValType::I32)]); // locals 1..3 after the param
        self.emit_web_model_unwrap(&mut f, shape, 0, 1);
        f.instruction(&Instruction::Call(view_idx));
        f.instruction(&Instruction::End);
        f
    }
}

/// Builds the self-contained HTTP core module for
/// `component::wrap_http_service`.
pub(super) fn generate_http_core_module(module: &OModule) -> Vec<u8> {
    let mut gen = WasmGen::new_http(module);
    gen.compile_http()
}

/// Builds the self-contained web-app core module (the web target, docs/src/reference/web-target.md).
/// Unlike the CLI/HTTP worlds this is a plain core module, not a
/// component — browsers instantiate core wasm directly and the
/// bundled JS host (`canon-web.js`) is the "component wrapper".
pub fn generate_web_core_module(module: &OModule) -> Vec<u8> {
    let mut gen = WasmGen::new_web(module);
    gen.compile_web()
}

// ── Arm-type helpers ──────────────────────────────────────────────────────────

/// Size in bytes of the ret-area an async-lowered call writes its result
/// into. The layout matches what the canonical ABI's async lower
/// expects: a single packed value at offset 0, aligned to its natural
/// boundary. Returns a multiple of 4 so the bump allocator's 4-byte
/// alignment is sufficient.
/// Lower a `JsonLit { parts }` into the equivalent left-associative
/// `String.concat` chain over `StringLit` (Static parts) and `.ToJson()`
/// method calls (Interp parts). The result is a normal `Expr` the
/// codegen can compile via its existing machinery — no JsonLit-specific
/// instructions to lower below this point.
///
/// Example: `{"k": foo}` (parts = [Static(`{"k":`), Interp(foo), Static(`}`)])
///
///   → `"{\"k\":".concat(foo.ToJson()).concat("}")`
fn json_lit_to_concat_chain(parts: &[crate::ast::JsonLitPart], span: crate::error::Span) -> Expr {
    use crate::ast::{Ident, JsonLitPart};
    let part_exprs: Vec<Expr> = parts
        .iter()
        .map(|p| match p {
            JsonLitPart::Static(s) => Expr::StringLit {
                value: s.clone(),
                span,
            },
            JsonLitPart::Interp(e) => Expr::MethodCall {
                receiver: e.clone(),
                method: Ident {
                    name: "ToJson".to_string(),
                    span,
                },
                args: vec![],
                piped: false,
                span,
            },
        })
        .collect();

    let mut iter = part_exprs.into_iter();
    // Parser invariant: parts is never empty (always starts with the
    // opening `{` or `[` as a Static).
    let mut acc = iter.next().expect("JsonLit parts must be non-empty");
    for next in iter {
        acc = Expr::MethodCall {
            receiver: Box::new(acc),
            method: Ident {
                name: "concat".to_string(),
                span,
            },
            args: vec![next],
            piped: false,
            span,
        };
    }
    acc
}

/// Lower an `HtmlLit { parts }` into the equivalent left-associative
/// `String.concat` chain over `StringLit` (Static parts) and
/// `.ToHtml()` method calls (Interp parts) — the exact HTML analogue of
/// `json_lit_to_concat_chain` above. `ToHtml` dispatches on the
/// interpolated value's type: `String` and `Int` escape through the
/// stdlib's `text()`, `Html` passes through unchanged.
///
/// Example: `<li>{name}</li>` (parts = [Static(`<li>`), Interp(name),
/// Static(`</li>`)])
///
///   → `"<li>".concat(name.ToHtml()).concat("</li>")`
fn html_lit_to_concat_chain(parts: &[crate::ast::HtmlLitPart], span: crate::error::Span) -> Expr {
    use crate::ast::{HtmlLitPart, Ident};
    let part_exprs: Vec<Expr> = parts
        .iter()
        .map(|p| match p {
            HtmlLitPart::Static(s) => Expr::StringLit {
                value: s.clone(),
                span,
            },
            HtmlLitPart::Interp(e) => Expr::MethodCall {
                receiver: e.clone(),
                method: Ident {
                    name: "ToHtml".to_string(),
                    span,
                },
                args: vec![],
                piped: false,
                span,
            },
        })
        .collect();

    let mut iter = part_exprs.into_iter();
    // Parser invariant: parts is never empty (the literal's opening
    // tag is always a Static).
    let mut acc = iter.next().expect("HtmlLit parts must be non-empty");
    for next in iter {
        acc = Expr::MethodCall {
            receiver: Box::new(acc),
            method: Ident {
                name: "concat".to_string(),
                span,
            },
            args: vec![next],
            piped: false,
            span,
        };
    }
    acc
}

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
        // Idempotent unwrap for primitive payloads. `ParsePos.Int`
        // (where `ParsePos = Int`) is a no-op at the wasm level —
        // the value on the stack is already an i64 — but the
        // surface-level type changes from the newtype to the base.
        // Matches the way `Ty::Str` handles `.String`.
        (Ty::I64, "Int") => Some(Ty::I64),
        (Ty::F64, "Float") => Some(Ty::F64),
        (Ty::I32, "Bool") => Some(Ty::I32),
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

/// Returns whether the program has a free function returning `Response`
/// (or `Result<Response, _>`), per the entry-point rule
/// (docs/src/spec/functions.md). When true, codegen routes through
/// `component::wrap_http_service` instead of the CLI path.
fn has_http_entry(module: &OModule) -> bool {
    use crate::ast::{entry_world_of, EntryWorld};
    module.items.iter().any(|item| match item {
        Item::Function(func) => {
            func.receiver.is_none() && entry_world_of(&func.return_ty) == Some(EntryWorld::Http)
        }
        _ => false,
    })
}

/// Builds the final Component Model component (`.wasm` bytes).
///
/// The output is a WASI Preview 3 component that exports
/// `wasi:cli/run@0.3.0-rc-2026-03-15`, imports `wasi:cli/stdout@0.3.0-rc-2026-03-15`,
/// and additionally imports every interface referenced by an `extern Wasm`
/// declaration in the user program.
/// It is validated with `wasmparser` before being returned.
pub fn generate(module: &OModule) -> Vec<u8> {
    // Branch on the entry-point's world (see the entry-point rule,
    // docs/src/spec/functions.md). CLI entries flow through the existing
    // hand-rolled `wasm-encoder` pipeline; HTTP entries route to a
    // separate codegen path that delegates type-section emission to
    // `wit-component` (the resource + variant surface in
    // `wasi:http/types` is too large to maintain by hand).
    //
    // The checker has already validated which entry shape applies
    // (slice 1a); this dispatch is the authoritative entry-world router.
    if has_http_entry(module) {
        let bytes = component::wrap_http_service(module);
        validate(&bytes);
        return bytes;
    }

    // Web-app entries (the `init`/`update`/`view` triple) emit a raw
    // core module — the JS host is the wrapper. See the web target, docs/src/reference/web-target.md.
    if crate::ast::find_web_entry(&module.items).is_some() {
        let bytes = generate_web_core_module(module);
        validate(&bytes);
        return bytes;
    }

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
