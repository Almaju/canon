//! `extern Wasm` import collection and canonical-ABI lowering.
//!
//! Walks the module for extern declarations, parses their paths, derives
//! core WASM signatures (WIT-informed where the vendored WIT is known),
//! and classifies indirect-return shapes. The resolved [`ExternImport`]
//! list feeds both codegen and the component wrapper.
use super::*;

/// Splits an `extern Wasm` path of the form
/// `"namespace:package/interface@version#fn-name"` into
/// `(component_namespace, core_namespace, fn_name)`.
///
///   - `component_namespace` keeps the `@version` suffix; it is the name
///     wasmtime matches against the linker.
///   - `core_namespace` strips the version; it is the import-module name we
///     use inside the core wasm module (purely an internal contract).
///   - `fn_name` is everything after `#`.
pub(super) fn parse_extern_path(path: &str) -> Option<(String, String, String)> {
    let (iface, fn_name) = path.split_once('#')?;
    let core_ns = match iface.split_once('@') {
        Some((before_version, _version)) => before_version.to_string(),
        None => iface.to_string(),
    };
    Some((iface.to_string(), core_ns, fn_name.to_string()))
}

pub(super) fn collect_extern_imports(ast: &OModule) -> Vec<ExternImport> {
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
                // A string-anchored binding returns its mint
                // (`SystemClockNow = Instant`); the record's field
                // names and layout key on the product the mint
                // aliases, so resolve the chain to the product itself.
                named_type_name(&func.return_ty)
                    .map(|n| resolve_alias_terminal_name(&n, &type_defs)),
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
        let mut indirect_return =
            record_shape.or_else(|| classify_return(&func.return_ty, &results, &type_defs));
        // WIT-informed refinement for scalar option/list payloads: Canon's
        // `Int` erases width and signedness, so the classification above
        // defaults to `s64`. For `wasi:*` imports the vendored WIT knows
        // the true element type (`get-random-bytes` returns `list<u8>`,
        // not `list<s64>`) — both the component-level import type and the
        // decode stride must match it.
        if component_ns.starts_with("wasi:") {
            match &mut indirect_return {
                Some(IndirectReturnShape::OptionScalar { prim }) => {
                    if let Some(p) = component::vendored_extern_option_payload(&ext.path) {
                        *prim = p;
                    }
                }
                Some(IndirectReturnShape::ListScalar { prim }) => {
                    if let Some(p) = component::vendored_extern_list_elem(&ext.path) {
                        *prim = p;
                    }
                }
                _ => {}
            }
        }
        // Bare `result` (no ok/err payloads): a *direct* return — the
        // canonical ABI flattens it to a single i32 discriminant, so the
        // core signature needs no ret-area transformation. Only the
        // component-level type (`result<_, _>`) and the call-site decode
        // (flip the discriminant into a Canon `Result` struct) differ
        // from a plain scalar.
        let bare_result =
            indirect_return.is_none() && is_bare_result_return(&func.return_ty, &type_defs);
        if bare_result {
            results = vec![ValType::I32];
        }
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
            bare_result,
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
pub(super) fn is_self_ctor(func: &FunctionDef) -> bool {
    func.name.name == "Self" && func.receiver.is_some()
}

/// Computes the WASM parameter types for a function. The receiver counts as
/// a runtime parameter *except* for `Self`-renamed constructors, where it's
/// purely a type-level marker.
pub(super) fn func_wasm_params_for(
    func: &FunctionDef,
    type_defs: &HashMap<String, TypeExpr>,
) -> Vec<ValType> {
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

pub(super) fn func_wasm_results_for(
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
pub(super) fn classify_return(
    return_ty: &TypeExpr,
    flat_results: &[ValType],
    type_defs: &HashMap<String, TypeExpr>,
) -> Option<IndirectReturnShape> {
    // A string-anchored binding mints a result newtype per function
    // (`GetInitialCwd = Option<String>`), so the structural shape sits
    // behind an alias — resolve it before pattern-matching.
    let return_ty = resolve_alias_structural(return_ty, type_defs);
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
        if name == "Option" && generics.len() == 1 {
            if resolves_to_string(&generics[0], type_defs) {
                return Some(IndirectReturnShape::OptionString);
            }
            if let Some(prim) = resolves_to_scalar_prim(&generics[0], type_defs) {
                return Some(IndirectReturnShape::OptionScalar { prim });
            }
        }
        if name == "List" && generics.len() == 1 {
            if resolves_to_string(&generics[0], type_defs) {
                return Some(IndirectReturnShape::ListString);
            }
            if let Some(prim) = resolves_to_scalar_prim(&generics[0], type_defs) {
                return Some(IndirectReturnShape::ListScalar { prim });
            }
        }
    }
    if matches!(flat_results, [ValType::I32, ValType::I32]) {
        return Some(IndirectReturnShape::String);
    }
    None
}

/// True when an extern's return type is `Result<A, B>` with both arms
/// resolving (through user alias chains) to `Unit` — the shape a
/// bindgen mint takes for a WIT bare `result;` return (`Sync = Unit`
/// plus a constructor returning `Result<Sync, Unit>`).
pub(super) fn is_bare_result_return(
    return_ty: &TypeExpr,
    type_defs: &HashMap<String, TypeExpr>,
) -> bool {
    let TypeExpr::Named { name, generics, .. } = return_ty else {
        return false;
    };
    name == "Result"
        && generics.len() == 2
        && generics.iter().all(|g| resolves_to_unit(g, type_defs))
}

fn resolves_to_unit(ty: &TypeExpr, type_defs: &HashMap<String, TypeExpr>) -> bool {
    let mut cur = ty;
    for _ in 0..20 {
        let TypeExpr::Named { name, generics, .. } = cur else {
            return false;
        };
        if !generics.is_empty() {
            return false;
        }
        if name == "Unit" {
            return true;
        }
        match type_defs.get(name) {
            Some(body) => cur = body,
            None => return false,
        }
    }
    false
}

/// Resolves an extern function's parameter types (receiver-first) into a list
/// of `ParamKind`s. Returns `None` if any parameter uses a type we can't yet
/// represent at the component-model boundary (lists, records, Result,
/// futures, …).
pub(super) fn build_extern_component_params(
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

pub(super) fn push_param_kind(
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
    /// `option<T>` return for a scalar payload `T` (WIT integer widths,
    /// floats, bool). Return area: the canonical option layout — disc
    /// byte at +0, payload at `align_to(1, align(T))` (= `align(T)`).
    /// Decoded into a fresh Canon `Option` struct (i32 tag at +0,
    /// 8-byte payload slot at +4), widening narrow ints per the WIT
    /// signedness and promoting `f32` to `f64`, so ordinary
    /// `(None, Some<Int>)` dispatch and `?` work unchanged.
    OptionScalar {
        prim: wasm_encoder::PrimitiveValType,
    },
    /// `list<string>` return. Return area: 8 bytes — (i32 list ptr,
    /// i32 element count). The canonical-ABI element layout (8-byte
    /// stride, i32 ptr + i32 len per element) is byte-identical to
    /// Canon's `List<String>` representation, so the pair is pushed
    /// directly as `Ty::List`.
    ListString,
    /// `list<T>` return for a scalar element `T`. Return area: 8 bytes
    /// — (i32 element ptr, i32 count). 8-byte elements (`u64`/`s64`/
    /// `f64`) share Canon's 8-byte-slot list layout and are pushed
    /// as-is; narrower elements are re-packed into a fresh Canon list,
    /// widening each element per the WIT signedness (`f32` promotes to
    /// `f64`).
    ListScalar {
        prim: wasm_encoder::PrimitiveValType,
    },
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
    pub(super) fn return_area_size(&self) -> u32 {
        match self {
            IndirectReturnShape::String => 8,
            IndirectReturnShape::ResultStringString { .. } => 12,
            IndirectReturnShape::OptionString => 12,
            // Canonical option layout: disc byte at +0, payload at
            // `align_to(1, align) = align`, total `align + size`
            // (already a multiple of the variant alignment for every
            // scalar), padded to the 4-byte ret-area minimum.
            IndirectReturnShape::OptionScalar { prim } => {
                let (size, align) = prim_size_align(*prim);
                (align + size).max(4)
            }
            IndirectReturnShape::ListString => 8,
            IndirectReturnShape::ListScalar { .. } => 8,
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
    /// True for a WIT bare `result;` return (no ok/err payloads). A
    /// *direct* return — one i32 discriminant — but the component-level
    /// type is `result<_, _>` and the call site flips the discriminant
    /// into a Canon `Result` struct (WIT: 0=ok; Canon: Err=0, Ok=1).
    pub(super) bare_result: bool,
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
