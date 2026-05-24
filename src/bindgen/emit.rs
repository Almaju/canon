//! Emit Oneway source from a parsed `wit_parser::Resolve`.
//!
//! For each interface in the resolve, this produces one `.ow` file
//! containing:
//!
//!   - Type declarations (records → products, variants → unions, etc.),
//!     emitted in alphabetical order.
//!   - Function declarations as `extern Wasm("ns:pkg/iface@ver#fn") name = (...) -> T`,
//!     also in alphabetical order.
//!
//! V1 scope (per the design doc): no resources, no async, no streams /
//! futures. When such an item is encountered we emit a `# skipped: …`
//! comment line and move on. The rest of the interface is still emitted.

use std::collections::BTreeMap;
use std::fmt::Write;

use std::collections::BTreeSet;

use wit_parser::{
    Function, FunctionKind, Handle, Interface, InterfaceId, Resolve, Type, TypeDef, TypeDefKind,
    TypeId, TypeOwner,
};

use super::naming::{
    interface_file_path, kebab_to_camel, kebab_to_pascal, kebab_to_snake, split_interface_id,
};

/// One file's worth of generated output.
pub struct EmittedFile {
    pub relative_path: String,
    pub content: String,
    /// Items the generator skipped because their WIT shape isn't yet
    /// representable in Oneway (resources, async, streams, futures, …).
    /// The caller surfaces these on stderr — the file itself is kept as
    /// clean Oneway source since the language has no comments.
    pub skipped: Vec<String>,
}

pub fn emit_all(resolve: &Resolve) -> Vec<EmittedFile> {
    let mut out = Vec::new();
    // Alphabetical iteration over interfaces by their fully-qualified id.
    let mut ifaces: BTreeMap<String, (InterfaceId, &Interface)> = BTreeMap::new();
    for (id, iface) in resolve.interfaces.iter() {
        if let Some(qid) = qualified_interface_id(resolve, iface) {
            ifaces.insert(qid, (id, iface));
        }
    }
    for (qualified_id, (id, iface)) in ifaces {
        if let Some(emitted) = emit_interface(resolve, id, iface, &qualified_id) {
            out.push(emitted);
        }
    }
    out
}

/// `wasi:clocks/monotonic-clock@0.3.0-rc-2026-03-15`, or `None` if the
/// interface is anonymous (inline `world` decls) — we don't emit those.
fn qualified_interface_id(resolve: &Resolve, iface: &Interface) -> Option<String> {
    let name = iface.name.as_deref()?;
    let pkg_id = iface.package?;
    let pkg = &resolve.packages[pkg_id];
    let ver = pkg
        .name
        .version
        .as_ref()
        .map(|v| format!("@{}", v))
        .unwrap_or_default();
    Some(format!(
        "{}:{}/{}{}",
        pkg.name.namespace, pkg.name.name, name, ver
    ))
}

fn emit_interface(
    resolve: &Resolve,
    self_iface_id: InterfaceId,
    iface: &Interface,
    qualified_id: &str,
) -> Option<EmittedFile> {
    let (ns, pkg, iface_name, _ver) = split_interface_id(qualified_id)?;
    let relative_path = interface_file_path(&ns, &pkg, &iface_name);

    let mut content = String::new();
    let mut skipped: Vec<String> = Vec::new();

    // Tracks every other interface whose types we transitively reference
    // (via `use other.{foo}` in WIT, or as a field/param/return type).
    // We emit one `use <ns>/<pkg>/<iface>` per entry, alphabetical.
    let mut external_use_paths: BTreeSet<String> = BTreeSet::new();

    // Type declarations, alphabetical by their PascalCase name.
    let mut type_decls: BTreeMap<String, String> = BTreeMap::new();
    for (name, type_id) in &iface.types {
        // If the entry is just an alias to a type owned by a different
        // interface (i.e. a WIT `use other.{name}` reference), don't emit
        // a local decl — record the source interface so we emit a `use`.
        if let Some(source_iface) = external_alias_source(resolve, *type_id, self_iface_id) {
            if let Some(use_path) = interface_use_path(resolve, source_iface) {
                external_use_paths.insert(use_path);
            }
            continue;
        }

        let pascal = kebab_to_pascal(name);
        match emit_type_decl(
            resolve,
            &pascal,
            *type_id,
            &mut external_use_paths,
            self_iface_id,
        ) {
            Ok(Some(decl)) => {
                type_decls.insert(pascal, decl);
            }
            Ok(None) => {} // pure alias to a local type — nothing to emit
            Err(reason) => {
                skipped.push(format!("{}: type {} ({})", qualified_id, pascal, reason));
            }
        }
    }

    // Function declarations, alphabetical by their camelCase name.
    let mut fn_decls: BTreeMap<String, String> = BTreeMap::new();
    for (name, func) in &iface.functions {
        match emit_function(
            resolve,
            qualified_id,
            name,
            func,
            &mut external_use_paths,
            self_iface_id,
        ) {
            Ok((camel, decl)) => {
                fn_decls.insert(camel, decl);
            }
            Err(reason) => {
                skipped.push(format!(
                    "{}: fn {} ({})",
                    qualified_id,
                    kebab_to_camel(name),
                    reason
                ));
            }
        }
    }

    // Emit `use` lines first (alphabetical — BTreeSet already sorted), then
    // type decls, then function decls.
    for use_path in &external_use_paths {
        let _ = writeln!(content, "use {}", use_path);
    }
    if !external_use_paths.is_empty() {
        content.push('\n');
    }
    for decl in type_decls.values() {
        content.push_str(decl);
        content.push('\n');
    }
    for decl in fn_decls.values() {
        content.push_str(decl);
        content.push('\n');
    }

    Some(EmittedFile {
        relative_path,
        content,
        skipped,
    })
}

/// If `type_id` is a transparent alias whose target is owned by a different
/// interface than `self_iface`, return the source interface id. WIT models
/// `use other.{name}` as a local `TypeDefKind::Type(Type::Id(target))`
/// alias where `target.owner` is the other interface.
fn external_alias_source(
    resolve: &Resolve,
    type_id: TypeId,
    self_iface: InterfaceId,
) -> Option<InterfaceId> {
    let td = &resolve.types[type_id];
    // The local alias entry itself is owned by the current interface; we
    // care about where its target lives.
    if td.owner != TypeOwner::Interface(self_iface) {
        // Not a local alias — caller should handle separately.
        return None;
    }
    if let TypeDefKind::Type(Type::Id(target)) = td.kind {
        let target_td = &resolve.types[target];
        if let TypeOwner::Interface(other) = target_td.owner {
            if other != self_iface {
                return Some(other);
            }
        }
    }
    None
}

/// Build `<ns>/<pkg>/<iface>` (without the trailing `.ow`) for use in a
/// `use` directive.
fn interface_use_path(resolve: &Resolve, iface_id: InterfaceId) -> Option<String> {
    let iface = &resolve.interfaces[iface_id];
    let qualified_id = qualified_interface_id(resolve, iface)?;
    let (ns, pkg, iface_name, _ver) = split_interface_id(&qualified_id)?;
    // A WIT interface lives at `oneway/<ns>/<pkg>/<iface>` from a consumer's
    // point of view: it's part of the `oneway/<ns>` package (today only
    // `oneway/wasi`) and the consumer writes a `use` against the package's
    // public path, which is `<pkg>/<iface>` inside it. The `src/` directory
    // is a layout convention invisible to `use`.
    //
    // If we ever generate bindings for a third-party namespace, this prefix
    // becomes a parameter — see the bindgen TODO.
    Some(format!(
        "oneway/{}/{}/{}",
        kebab_to_snake(&ns),
        kebab_to_snake(&pkg),
        kebab_to_snake(&iface_name),
    ))
}

/// Returns `Ok(Some(decl))` if a Oneway declaration was produced for this
/// type, `Ok(None)` if the type is a transparent alias to another local
/// type that needs no declaration of its own, or `Err(reason)` if the
/// type uses a feature we don't yet support (resource, future, stream).
fn emit_type_decl(
    resolve: &Resolve,
    name: &str,
    type_id: TypeId,
    external_use_paths: &mut BTreeSet<String>,
    self_iface: InterfaceId,
) -> Result<Option<String>, String> {
    let td: &TypeDef = &resolve.types[type_id];
    match &td.kind {
        TypeDefKind::Record(record) => {
            // Product type. Each field becomes its own newtype prefixed
            // by the record name (e.g. record `point` with fields `x`/`y`
            // → `Point = PointX * PointY` + `PointX = Float` + `PointY = Float`).
            // The prefix prevents collisions when two records in the same
            // interface share a field name, and — more importantly —
            // satisfies Oneway's "product members are distinct types" rule
            // even when several fields share a WIT type.
            let mut fields: Vec<(String, String)> = Vec::new();
            for f in &record.fields {
                let fname = format!("{}{}", name, kebab_to_pascal(&f.name));
                let fty = render_type(resolve, &f.ty, external_use_paths, self_iface)?;
                fields.push((fname, fty));
            }
            fields.sort_by(|a, b| a.0.cmp(&b.0));
            let rhs = fields
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(" * ");
            let mut s = String::new();
            let _ = writeln!(s, "{} = {}", name, rhs);
            for (n, ty) in &fields {
                let _ = writeln!(s, "{} = {}", n, ty);
            }
            Ok(Some(s))
        }
        TypeDefKind::Enum(e) => {
            // Zero-data union. Each tag becomes its own zero-data type,
            // prefixed by the enum name to avoid collisions across enums.
            let mut variants: Vec<String> = e
                .cases
                .iter()
                .map(|c| format!("{}{}", name, kebab_to_pascal(&c.name)))
                .collect();
            variants.sort();
            let mut s = String::new();
            let _ = writeln!(s, "{} = {}", name, variants.join(" + "));
            for v in &variants {
                let _ = writeln!(s, "{} = Unit", v);
            }
            Ok(Some(s))
        }
        TypeDefKind::Variant(v) => {
            // Variant — like an enum but some arms may carry a payload.
            // Data-carrying arms become 1-component products. Arms are
            // prefixed by the variant name (same reason as enum cases).
            let mut arms: Vec<(String, Option<String>)> = Vec::new();
            for c in &v.cases {
                let pname = format!("{}{}", name, kebab_to_pascal(&c.name));
                let payload = match &c.ty {
                    Some(t) => Some(render_type(resolve, t, external_use_paths, self_iface)?),
                    None => None,
                };
                arms.push((pname, payload));
            }
            arms.sort_by(|a, b| a.0.cmp(&b.0));
            let union_rhs = arms
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(" + ");
            let mut s = String::new();
            let _ = writeln!(s, "{} = {}", name, union_rhs);
            for (n, payload) in &arms {
                match payload {
                    Some(p) => {
                        let _ = writeln!(s, "{} = {}", n, p);
                    }
                    None => {
                        let _ = writeln!(s, "{} = Unit", n);
                    }
                }
            }
            Ok(Some(s))
        }
        TypeDefKind::Flags(f) => {
            // Each flag is a Bool field; the whole thing is a product.
            // Prefix each flag with the parent type's name so two `flags`
            // declarations in the same interface can both contain e.g.
            // a `bold` field without colliding at the Oneway type level.
            let mut fields: Vec<String> = f
                .flags
                .iter()
                .map(|fl| format!("{}{}", name, kebab_to_pascal(&fl.name)))
                .collect();
            fields.sort();
            let mut s = String::new();
            let _ = writeln!(s, "{} = {}", name, fields.join(" * "));
            for fl in &fields {
                let _ = writeln!(s, "{} = Bool", fl);
            }
            Ok(Some(s))
        }
        TypeDefKind::Tuple(t) => {
            // Anonymous positional product. Use _0, _1, … field names
            // (alphabetised by index).
            let mut fields: Vec<(String, String)> = Vec::new();
            for (i, ty) in t.types.iter().enumerate() {
                let n = format!("_{}", i);
                let r = render_type(resolve, ty, external_use_paths, self_iface)?;
                fields.push((n, r));
            }
            fields.sort_by(|a, b| a.0.cmp(&b.0));
            let rhs = fields
                .iter()
                .map(|(_, ty)| ty.as_str())
                .collect::<Vec<_>>()
                .join(" * ");
            Ok(Some(format!("{} = {}\n", name, rhs)))
        }
        TypeDefKind::Type(t) => {
            // Pure alias: `type foo = bar`. Cross-interface aliases are
            // intercepted by `external_alias_source` in `emit_interface`
            // before we get here, so any alias reaching this branch is
            // local-to-local.
            let r = render_type(resolve, t, external_use_paths, self_iface)?;
            Ok(Some(format!("{} = {}\n", name, r)))
        }
        TypeDefKind::List(t) => {
            let r = render_type(resolve, t, external_use_paths, self_iface)?;
            Ok(Some(format!("{} = List<{}>\n", name, r)))
        }
        TypeDefKind::Option(t) => {
            let r = render_type(resolve, t, external_use_paths, self_iface)?;
            Ok(Some(format!("{} = Option<{}>\n", name, r)))
        }
        TypeDefKind::Result(r) => {
            let ok = match &r.ok {
                Some(t) => render_type(resolve, t, external_use_paths, self_iface)?,
                None => "Unit".to_string(),
            };
            let err = match &r.err {
                Some(t) => render_type(resolve, t, external_use_paths, self_iface)?,
                None => "Unit".to_string(),
            };
            Ok(Some(format!("{} = Result<{}, {}>\n", name, ok, err)))
        }
        TypeDefKind::Map(k, v) => {
            let kr = render_type(resolve, k, external_use_paths, self_iface)?;
            let vr = render_type(resolve, v, external_use_paths, self_iface)?;
            Ok(Some(format!("{} = Map<{}, {}>\n", name, kr, vr)))
        }
        TypeDefKind::Resource => {
            // Opaque resource — declare as a `Handle` newtype. Methods
            // (own/borrow consumers) are emitted as ordinary free fns by
            // `emit_function`, gated by codegen support.
            Ok(Some(format!("{} = Handle\n", name)))
        }
        TypeDefKind::Handle(h) => {
            // A `type foo = own<bar>` (or `borrow<bar>`) alias: render as
            // the target resource's PascalCase name. Own/borrow is
            // intentionally invisible at the source level.
            let target = match h {
                Handle::Own(id) | Handle::Borrow(id) => *id,
            };
            let target_name = resolve.types[target]
                .name
                .as_deref()
                .map(kebab_to_pascal)
                .unwrap_or_else(|| "Unknown".into());
            Ok(Some(format!("{} = {}\n", name, target_name)))
        }
        TypeDefKind::Future(_) => Err("future (v1 skips futures)".into()),
        TypeDefKind::Stream(_) => Err("stream (v1 skips streams)".into()),
        TypeDefKind::FixedLengthList(_, _) => Err("fixed-length list (not yet supported)".into()),
        TypeDefKind::Unknown => Err("unknown type kind".into()),
    }
}

fn emit_function(
    resolve: &Resolve,
    qualified_iface_id: &str,
    name: &str,
    func: &Function,
    external_use_paths: &mut BTreeSet<String>,
    self_iface: InterfaceId,
) -> Result<(String, String), String> {
    // Skip async + resource methods. Resource *types* are emitted as
    // `Foo = Handle` newtypes; their methods stay skipped until the
    // codegen learns to lower `own<T>`/`borrow<T>` canonical-ABI shapes
    // (see CLAUDE.md "Known codegen gaps").
    let (is_async, fn_label) = match &func.kind {
        FunctionKind::Freestanding => (false, name.to_string()),
        FunctionKind::AsyncFreestanding => (true, name.to_string()),
        FunctionKind::Method(_)
        | FunctionKind::Static(_)
        | FunctionKind::Constructor(_)
        | FunctionKind::AsyncMethod(_)
        | FunctionKind::AsyncStatic(_) => {
            return Err("resource method (codegen lowering pending, see CLAUDE.md)".into());
        }
    };
    if is_async {
        return Err("async (v1 skips async)".into());
    }

    // A free-standing function that takes or returns a `Handle`/`own<T>`/
    // `borrow<T>` somewhere in its signature still can't be lowered yet —
    // skip with the same reason as resource methods so the gap is reported
    // uniformly.
    if mentions_handle(resolve, &func.result)
        || func
            .params
            .iter()
            .any(|p| mentions_handle_ty(resolve, &p.ty))
    {
        return Err("handle in signature (codegen lowering pending, see CLAUDE.md)".into());
    }

    // V1 codegen gap: `extern Wasm` functions returning `list<T>` produce
    // a broken core/component type signature pair — the import is rejected
    // by the validator even when the function isn't called. Until the
    // codegen learns to emit indirect-return list shapes correctly we skip
    // such functions here so the binding file as a whole stays usable.
    if returns_list(resolve, &func.result) {
        return Err("list<T> return type (codegen gap, see CLAUDE.md)".into());
    }

    // Codegen gap: WIT `result` (no `<ok, err>` payloads) lowers to a
    // discriminant-only canonical-ABI shape that the current core-module
    // emitter renders as `u32`, mismatched with the host's `result` shape.
    // The whole component is rejected when this import is declared, even
    // if the user code never calls the function (see `wasi:cli/exit#exit`).
    // Skip until codegen learns the bare-result shape; `exit-with-code`
    // covers the same use case via plain `u8`.
    if has_bare_result(resolve, &func.result)
        || func.params.iter().any(|p| is_bare_result(resolve, &p.ty))
    {
        return Err("bare `result` (codegen gap, no ok/err payloads)".into());
    }

    // Codegen gap: Oneway has a single `Int` type and always lowers it as
    // `u64` (8 bytes) in the canonical ABI. WIT distinguishes u8/u16/u32/
    // s8/s16/s32 from s64/u64, and the host rejects any import whose
    // canonical-ABI signature mismatches. Until codegen learns to honor
    // the WIT-declared width, skip functions that use the narrow forms
    // anywhere in params or return.
    let narrow_in_params = func.params.iter().any(|p| has_narrow_int(resolve, &p.ty));
    let narrow_in_return = func
        .result
        .as_ref()
        .map(|t| has_narrow_int(resolve, t))
        .unwrap_or(false);
    if narrow_in_params || narrow_in_return {
        return Err("sub-u64 integer width (codegen lowers all `Int` as u64)".into());
    }

    let camel = kebab_to_camel(name);

    // Build the param product. Each WIT parameter becomes a component
    // typed by its declared WIT type; we surface the parameter *name*
    // only through the type (Oneway has no named parameters), so two
    // params of the same WIT type force a newtype on the caller's side
    // anyway — same situation as hand-written stdlib bindings.
    let mut params: Vec<String> = Vec::new();
    for p in &func.params {
        params.push(render_type(resolve, &p.ty, external_use_paths, self_iface)?);
    }
    // Oneway requires alphabetical product components; if two are
    // the same type that's an error the user (or the std/ wrapper)
    // must resolve. We sort here so the source is always valid for
    // the common single-of-each-type case.
    params.sort();
    let params_str = if params.is_empty() {
        "()".to_string()
    } else {
        format!("({})", params.join(" * "))
    };

    let ret = match &func.result {
        Some(t) => render_type(resolve, t, external_use_paths, self_iface)?,
        None => "Unit".to_string(),
    };

    // Reconstruct the canonical-ABI path: `ns:pkg/iface@ver#fn-name`.
    let extern_path = format!("{}#{}", qualified_iface_id, fn_label);

    let mut decl = String::new();
    let _ = writeln!(decl, "extern Wasm(\"{}\")", extern_path);
    let _ = writeln!(decl, "{} = {} -> {}", camel, params_str, ret);

    Ok((camel, decl))
}

/// Returns true if the function's result type is (or transparently aliases)
/// a WIT `list<T>`. Used to filter out functions whose extern import the
/// current core codegen can't lower correctly.
fn returns_list(resolve: &Resolve, result: &Option<Type>) -> bool {
    let Some(t) = result else { return false };
    is_list_shape(resolve, t)
}

/// True when the function returns a WIT `result` with neither `ok` nor `err`
/// payloads (i.e. the bare `result;` form). See the skip-rule comment in
/// `emit_function` for why this matters.
fn has_bare_result(resolve: &Resolve, result: &Option<Type>) -> bool {
    let Some(t) = result else { return false };
    is_bare_result(resolve, t)
}

fn is_bare_result(resolve: &Resolve, t: &Type) -> bool {
    match t {
        Type::Id(id) => {
            let td = &resolve.types[*id];
            match &td.kind {
                TypeDefKind::Result(r) => r.ok.is_none() && r.err.is_none(),
                TypeDefKind::Type(inner) => is_bare_result(resolve, inner),
                _ => false,
            }
        }
        _ => false,
    }
}

/// True when `t` references a WIT integer type narrower than `s64`/`u64`.
/// Walks aliases and structural types (records, variants, options, lists,
/// tuples, results) so e.g. `option<u32>` or `record { x: u8 }` are
/// detected. Resources and futures/streams are already filtered
/// upstream and treated as terminal misses.
fn has_narrow_int(resolve: &Resolve, t: &Type) -> bool {
    match t {
        Type::U8 | Type::U16 | Type::U32 | Type::S8 | Type::S16 | Type::S32 => true,
        Type::U64
        | Type::S64
        | Type::F32
        | Type::F64
        | Type::Bool
        | Type::Char
        | Type::String
        | Type::ErrorContext => false,
        Type::Id(id) => {
            let td = &resolve.types[*id];
            match &td.kind {
                TypeDefKind::Type(inner) => has_narrow_int(resolve, inner),
                TypeDefKind::List(inner) => has_narrow_int(resolve, inner),
                TypeDefKind::Option(inner) => has_narrow_int(resolve, inner),
                TypeDefKind::Result(r) => {
                    r.ok.as_ref()
                        .map(|t| has_narrow_int(resolve, t))
                        .unwrap_or(false)
                        || r.err
                            .as_ref()
                            .map(|t| has_narrow_int(resolve, t))
                            .unwrap_or(false)
                }
                TypeDefKind::Tuple(t) => t.types.iter().any(|t| has_narrow_int(resolve, t)),
                TypeDefKind::Record(r) => r.fields.iter().any(|f| has_narrow_int(resolve, &f.ty)),
                TypeDefKind::Variant(v) => v.cases.iter().any(|c| {
                    c.ty.as_ref()
                        .map(|t| has_narrow_int(resolve, t))
                        .unwrap_or(false)
                }),
                TypeDefKind::Map(k, v) => has_narrow_int(resolve, k) || has_narrow_int(resolve, v),
                // Resources / futures / streams / flags / enums never
                // carry a narrow-int themselves; they're filtered earlier
                // by separate skip rules.
                _ => false,
            }
        }
    }
}

/// True when `t` is (or transparently aliases) a WIT `resource`, an
/// `own<T>`/`borrow<T>` handle, or a structural type that transitively
/// contains one. Used to filter functions whose signature mentions a
/// resource so they can be skipped uniformly while codegen support for
/// handle-typed extern imports is pending.
fn mentions_handle(resolve: &Resolve, result: &Option<Type>) -> bool {
    let Some(t) = result else { return false };
    mentions_handle_ty(resolve, t)
}

fn mentions_handle_ty(resolve: &Resolve, t: &Type) -> bool {
    match t {
        Type::Bool
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::S8
        | Type::S16
        | Type::S32
        | Type::S64
        | Type::F32
        | Type::F64
        | Type::Char
        | Type::String
        | Type::ErrorContext => false,
        Type::Id(id) => {
            let td = &resolve.types[*id];
            match &td.kind {
                TypeDefKind::Resource | TypeDefKind::Handle(_) => true,
                TypeDefKind::Type(inner) => mentions_handle_ty(resolve, inner),
                TypeDefKind::List(inner) => mentions_handle_ty(resolve, inner),
                TypeDefKind::Option(inner) => mentions_handle_ty(resolve, inner),
                TypeDefKind::Result(r) => {
                    r.ok.as_ref()
                        .map(|t| mentions_handle_ty(resolve, t))
                        .unwrap_or(false)
                        || r.err
                            .as_ref()
                            .map(|t| mentions_handle_ty(resolve, t))
                            .unwrap_or(false)
                }
                TypeDefKind::Tuple(t) => t.types.iter().any(|t| mentions_handle_ty(resolve, t)),
                TypeDefKind::Record(r) => {
                    r.fields.iter().any(|f| mentions_handle_ty(resolve, &f.ty))
                }
                TypeDefKind::Variant(v) => v.cases.iter().any(|c| {
                    c.ty.as_ref()
                        .map(|t| mentions_handle_ty(resolve, t))
                        .unwrap_or(false)
                }),
                TypeDefKind::Map(k, v) => {
                    mentions_handle_ty(resolve, k) || mentions_handle_ty(resolve, v)
                }
                TypeDefKind::Future(inner) | TypeDefKind::Stream(inner) => inner
                    .as_ref()
                    .map(|t| mentions_handle_ty(resolve, t))
                    .unwrap_or(false),
                _ => false,
            }
        }
    }
}

fn is_list_shape(resolve: &Resolve, t: &Type) -> bool {
    match t {
        Type::Id(id) => {
            let td = &resolve.types[*id];
            match &td.kind {
                TypeDefKind::List(_) => true,
                TypeDefKind::Type(inner) => is_list_shape(resolve, inner),
                _ => false,
            }
        }
        _ => false,
    }
}

/// Render a WIT `Type` as Oneway source. Returns `Err` for unsupported
/// shapes (resource/handle/future/stream).
///
/// `external_use_paths` is the accumulator for cross-interface references
/// the caller will emit as `use ` lines at the top of the file.
fn render_type(
    resolve: &Resolve,
    t: &Type,
    external_use_paths: &mut BTreeSet<String>,
    self_iface: InterfaceId,
) -> Result<String, String> {
    match t {
        Type::Bool => Ok("Bool".into()),
        // All WIT integer widths collapse to `Int` per the design doc.
        Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::S8
        | Type::S16
        | Type::S32
        | Type::S64 => Ok("Int".into()),
        Type::F32 | Type::F64 => Ok("Float".into()),
        Type::Char => Ok("String".into()),
        Type::String => Ok("String".into()),
        Type::ErrorContext => Err("error-context".into()),
        Type::Id(id) => render_type_id(resolve, *id, external_use_paths, self_iface),
    }
}

fn render_type_id(
    resolve: &Resolve,
    id: TypeId,
    external_use_paths: &mut BTreeSet<String>,
    self_iface: InterfaceId,
) -> Result<String, String> {
    let td = &resolve.types[id];

    // Named, interface-owned type — use its bare PascalCase name. If the
    // owning interface is a different one, record the cross-interface
    // dependency so we emit a `use` line at the top.
    if let (Some(name), TypeOwner::Interface(owner)) = (&td.name, &td.owner) {
        if *owner != self_iface {
            if let Some(path) = interface_use_path(resolve, *owner) {
                external_use_paths.insert(path);
            }
        }
        return Ok(kebab_to_pascal(name));
    }

    // Anonymous (inline) — recurse into the kind.
    match &td.kind {
        TypeDefKind::List(t) => Ok(format!(
            "List<{}>",
            render_type(resolve, t, external_use_paths, self_iface)?
        )),
        TypeDefKind::Option(t) => Ok(format!(
            "Option<{}>",
            render_type(resolve, t, external_use_paths, self_iface)?
        )),
        TypeDefKind::Result(r) => {
            let ok = match &r.ok {
                Some(t) => render_type(resolve, t, external_use_paths, self_iface)?,
                None => "Unit".into(),
            };
            let err = match &r.err {
                Some(t) => render_type(resolve, t, external_use_paths, self_iface)?,
                None => "Unit".into(),
            };
            Ok(format!("Result<{}, {}>", ok, err))
        }
        TypeDefKind::Tuple(t) => {
            // Anonymous tuple in argument/return position. Oneway has no
            // anonymous product — bail with a clear message so the caller
            // turns it into a skipped item.
            Err(format!(
                "inline tuple of arity {} (introduce a named record in the WIT)",
                t.types.len()
            ))
        }
        TypeDefKind::Type(t) => render_type(resolve, t, external_use_paths, self_iface),
        TypeDefKind::Resource => {
            // Inline (unnamed) `resource` references shouldn't appear in
            // well-formed WIT — every resource has a name. The named-case
            // early return above handles the normal path; this arm just
            // surfaces a clear error if the impossible happens.
            Err("anonymous resource".into())
        }
        TypeDefKind::Handle(h) => {
            // Anonymous `own<bar>` / `borrow<bar>` in a parameter or return
            // position. Render as the target's PascalCase name; the
            // ownership distinction is encoded in the canonical-ABI
            // lowering, not in the source-level type.
            let target = match h {
                Handle::Own(id) | Handle::Borrow(id) => *id,
            };
            let target_td = &resolve.types[target];
            if let TypeOwner::Interface(owner) = target_td.owner {
                if owner != self_iface {
                    if let Some(path) = interface_use_path(resolve, owner) {
                        external_use_paths.insert(path);
                    }
                }
            }
            match &target_td.name {
                Some(n) => Ok(kebab_to_pascal(n)),
                None => Err("handle to anonymous resource".into()),
            }
        }
        TypeDefKind::Future(_) => Err("future".into()),
        TypeDefKind::Stream(_) => Err("stream".into()),
        _ => {
            // Records/variants/enums/flags should always be named when
            // they appear here. If not, fall back to a synthetic name.
            if let Some(n) = &td.name {
                Ok(kebab_to_pascal(n))
            } else {
                Err("anonymous type without a name".into())
            }
        }
    }
}
