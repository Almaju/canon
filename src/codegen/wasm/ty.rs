//! Canon type representation and WASM layout.
//!
//! [`Ty`] is what a compiled expression leaves on the stack; the free
//! helpers and the `WasmGen` repr methods map Canon type expressions to
//! their WASM value types, sizes, and alignments.
use super::*;

/// True when `ty` is `String` or any alias chain that ultimately resolves
/// to `String` (e.g. `Path = String`, `File = Path`, …).
pub(super) fn resolves_to_string(ty: &TypeExpr, type_defs: &HashMap<String, TypeExpr>) -> bool {
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

pub(super) fn named_type_name(ty: &TypeExpr) -> Option<String> {
    if let TypeExpr::Named { name, .. } = ty {
        Some(name.clone())
    } else {
        None
    }
}

/// Builds a quick `name -> body` map of all type aliases declared in the
/// module. Used by the helpers below to resolve user-named aliases of scalar
/// types (e.g. `OtherInt = Int`).
pub(super) fn build_type_defs_map(ast: &OModule) -> HashMap<String, TypeExpr> {
    let mut map = HashMap::new();
    for item in ast.items.iter() {
        if let Item::TypeDef(td) = item {
            map.insert(td.name.name.clone(), td.body.clone());
        }
    }
    map
}

/// True for WIT integer widths below 64 bits — these lower to core
/// `i32` while Canon's `Int` is `i64`, so call sites wrap/extend.
pub(super) fn is_narrow_prim(p: wasm_encoder::PrimitiveValType) -> bool {
    use wasm_encoder::PrimitiveValType as P;
    matches!(p, P::U8 | P::U16 | P::U32 | P::S8 | P::S16 | P::S32)
}

/// Canonical-ABI size and alignment of a scalar primitive.
pub(super) fn prim_size_align(p: wasm_encoder::PrimitiveValType) -> (u32, u32) {
    use wasm_encoder::PrimitiveValType as P;
    match p {
        P::Bool | P::U8 | P::S8 => (1, 1),
        P::U16 | P::S16 => (2, 2),
        P::U32 | P::S32 | P::F32 | P::Char => (4, 4),
        _ => (8, 8),
    }
}

pub(super) fn scalar_val_type_to_primitive(vt: ValType, signed: bool) -> wasm_encoder::PrimitiveValType {
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
pub(super) fn type_expr_val_types(ty: &TypeExpr, type_defs: &HashMap<String, TypeExpr>) -> Vec<ValType> {
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
pub(super) fn resolve_name_val_types(name: &str, type_defs: &HashMap<String, TypeExpr>) -> Vec<ValType> {
    fn go(name: &str, type_defs: &HashMap<String, TypeExpr>, depth: u32) -> Vec<ValType> {
        if depth > 20 {
            return vec![ValType::I32];
        }
        match name {
            "Int" => vec![ValType::I64],
            "Float" => vec![ValType::F64],
            "Bool" => vec![ValType::I32],
            "Unit" | "Never" => vec![],
            // Prelude String-aliases (see `resolve_repr_depth`): known
            // without the stdlib module being loaded.
            "String" | "Html" | "Json" => vec![ValType::I32, ValType::I32],
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

/// What a compiled expression leaves on the WASM stack.
///
/// The `Named*` variants carry the Canon type name so method dispatch can
/// find the right user-defined function.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Ty {
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
    pub(super) fn val_types(&self) -> Vec<ValType> {
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
    pub(super) fn canon_name(&self) -> Option<&str> {
        match self {
            Ty::NamedStr(n) | Ty::NamedPtr(n) | Ty::NamedPtrStr(n, _, _) => Some(n.as_str()),
            _ => None,
        }
    }

    pub(super) fn is_str_like(&self) -> bool {
        matches!(self, Ty::Str | Ty::NamedStr(_))
    }
}

// ── Local scope ───────────────────────────────────────────────────────────────

pub(super) fn ret_area_size_for(ty: &Ty) -> u32 {
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

impl<'m> WasmGen<'m> {
    pub(super) fn resolve_repr(&self, name: &str) -> Ty {
        self.resolve_repr_depth(name, 0)
    }

    pub(super) fn resolve_repr_depth(&self, name: &str, depth: u32) -> Ty {
        if depth > 20 {
            return Ty::NamedPtr(name.to_string());
        }
        match name {
            "Int" | "Byte" | "Hex" => Ty::I64,
            "Float" => Ty::F64,
            "Bool" | "True" | "False" => Ty::I32,
            "String" => Ty::Str,
            // `Html` / `Json` are prelude types — `= String` intrinsically.
            // Hardcode their repr so an `Html`- or `Json`-returning function
            // compiles even when nothing pulls the stdlib alias into scope
            // (e.g. a web `view` whose whole body is one HTML literal).
            // `NamedStr` matches what the `type_defs` alias path yields when
            // the module *is* loaded, so dispatch is unchanged either way.
            "Html" | "Json" => Ty::NamedStr(name.to_string()),
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

    pub(super) fn resolve_type_expr_repr(&self, ty: &TypeExpr) -> Ty {
        match ty {
            TypeExpr::Named { name, .. } => self.resolve_repr(name),
            TypeExpr::Union { .. } => Ty::Ptr,
            TypeExpr::Product { .. } => Ty::Ptr,
            _ => Ty::Unit,
        }
    }

    pub(super) fn resolve_return_ty(&self, func: &FunctionDef) -> Ty {
        self.resolve_type_expr_repr(&func.return_ty)
    }

    /// Byte size of a value when stored as a FIELD inside a product/union struct.
    pub(super) fn field_byte_size(&self, name: &str) -> u32 {
        match self.resolve_repr(name) {
            Ty::I64 | Ty::F64 => 8,
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => 4,
            Ty::Str | Ty::NamedStr(_) | Ty::List => 8,
            Ty::Unit => 0,
        }
    }

    /// Field layout for a product type (name → payload byte size used for offsets).
    /// Returns (field_repr, byte_offset_within_payload).
    pub(super) fn product_field_layout(&self, product_name: &str) -> Vec<(String, Ty, u32)> {
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
    pub(super) fn variant_payload_size(&self, variant_name: &str) -> u32 {
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
    pub(super) fn union_total_size(&self, union_name: &str) -> u32 {
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
    pub(super) fn func_wasm_params(&self, func: &FunctionDef) -> Vec<ValType> {
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
    pub(super) fn func_wasm_results(&self, func: &FunctionDef) -> Vec<ValType> {
        self.resolve_type_expr_repr(&func.return_ty).val_types()
    }

    // ── Built-in function builders ─────────────────────────────────────────────
}
