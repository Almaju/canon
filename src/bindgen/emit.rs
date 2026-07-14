//! Emit Canon source from a parsed `wit_parser::Resolve`.
//!
//! For each interface in the resolve, this produces one `.can` file
//! containing:
//!
//!   - Type declarations (records → products, variants → unions, etc.),
//!     emitted in alphabetical order.
//!   - Function declarations as `extern Wasm("ns:pkg/iface@ver#fn") name = (...) => T`,
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
    /// The WIT interface URN this file was generated from, of the form
    /// `"<ns>:<pkg>/<iface>@<version>"`. Used by `canon install` to
    /// populate the install index alongside the generated source; the
    /// loader then reconstructs per-function `extern Wasm` paths from
    /// this URN plus the function name.
    pub urn: String,
    /// Items the generator skipped because their WIT shape isn't yet
    /// representable in Canon (resources, async, streams, futures, …).
    /// The caller surfaces these on stderr — the file itself is kept as
    /// clean Canon source since the language has no comments.
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
    // Batch-level collision census. Imports are resolved by bare name
    // (there is no `use`), so a Canon module's namespace is flat: two
    // interfaces both exporting `now` can never coexist as free
    // functions. A function whose camelCase name appears in more than
    // one interface of this install set is emitted as a method on its
    // interface's capability marker (`MonotonicClock.now()`), so
    // discovery resolves on the unique marker type — see
    // `emit_function`.
    let mut fn_name_uses: BTreeMap<String, u32> = BTreeMap::new();
    for (_, iface) in ifaces.values() {
        for name in iface.functions.keys() {
            *fn_name_uses.entry(kebab_to_camel(name)).or_default() += 1;
        }
    }
    for (qualified_id, (id, iface)) in ifaces {
        if let Some(emitted) = emit_interface(resolve, id, iface, &qualified_id, &fn_name_uses) {
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
    fn_name_uses: &BTreeMap<String, u32>,
) -> Option<EmittedFile> {
    let (ns, pkg, iface_name, ver) = split_interface_id(qualified_id)?;
    let relative_path = interface_file_path(&ns, &pkg, &iface_name, ver.as_deref());

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

    // Function declarations, alphabetical by the type each constructs.
    // Keyed by `(constructed-type, wit-fn-name)` so two constructors of
    // the same type (a constructor family) both survive and stay ordered.
    let fn_ctx = FnEmitCtx {
        iface_name: &iface_name,
        fn_name_uses,
        wit_informed: ns == "wasi",
    };
    let mut fn_decls: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut needs_capability = false;
    for (name, func) in &iface.functions {
        match emit_function(
            resolve,
            &fn_ctx,
            name,
            func,
            &mut external_use_paths,
            self_iface_id,
        ) {
            Ok((sort_key, decl, used_capability)) => {
                needs_capability |= used_capability;
                fn_decls.insert((sort_key, name.clone()), decl);
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

    // A colliding function is emitted as a method on an interface
    // capability marker (see `emit_function`); declare that marker type
    // here. It is a zero-data `Unit` capability, so the codegen drops
    // the receiver — the WIT function stays zero-arg — and the marker's
    // unique PascalCase name is what reference discovery resolves on
    // (`MonotonicClock.now()` finds this file), never the bare `now`.
    if needs_capability {
        let cap = kebab_to_pascal(&iface_name);
        type_decls.insert(cap.clone(), format!("{cap} = Unit\n"));
    }

    // Binding files carry no header: the vendored path spells the
    // interface URN, and the loader derives each declaration's binding
    // from it (a binding file is recognized by shape). Cross-interface
    // type references need no import lines either — the loader resolves
    // them by name against the sibling binding files
    // (`external_use_paths` still gates which type entries are alias
    // re-exports vs. local declarations).
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
        urn: qualified_id.to_string(),
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

/// Build `<ns>/<pkg>/<iface>` (without the trailing `.can`) for use in a
/// `use` directive.
fn interface_use_path(resolve: &Resolve, iface_id: InterfaceId) -> Option<String> {
    let iface = &resolve.interfaces[iface_id];
    let qualified_id = qualified_interface_id(resolve, iface)?;
    let (ns, pkg, iface_name, _ver) = split_interface_id(&qualified_id)?;
    // A WIT interface lives at `<ns>/<pkg>/<iface>` from a consumer's
    // point of view. After `canon install` writes the bindings to
    // `<project>/bindgen/<ns>/<pkg>/<iface>.can`, the loader resolves
    // `use <ns>/<pkg>/<iface>` against that file (via the project's
    // `bindgen/` lookup for user code, or via the same-package bundled
    // lookup for compiler-shipped `canon/std`). Before the manifest-
    // driven flow landed, bindings lived inside the `canon/wasi`
    // bundled package and this function emitted an `canon/wasi/…`
    // prefix; that prefix is gone now.
    Some(format!(
        "{}/{}/{}",
        kebab_to_snake(&ns),
        kebab_to_snake(&pkg),
        kebab_to_snake(&iface_name),
    ))
}

/// Returns `Ok(Some(decl))` if a Canon declaration was produced for this
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
            // satisfies Canon's "product members are distinct types" rule
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
            // a `bold` field without colliding at the Canon type level.
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
            // When every field shares the same rendered type and there
            // are at least 2 of them, collapse to the `T^N` repeat form.
            // This is both shorter and — more importantly — sidesteps the
            // "product components must be distinct types" rule that would
            // otherwise reject a WIT `tuple<u16, u16, u16, u16, u16, u16,
            // u16, u16>` (e.g. `wasi:sockets/types#ipv6-address`).
            let rhs = if fields.len() >= 2 && fields.iter().all(|(_, ty)| ty == &fields[0].1) {
                format!("{}^{}", fields[0].1, fields.len())
            } else {
                fields
                    .iter()
                    .map(|(_, ty)| ty.as_str())
                    .collect::<Vec<_>>()
                    .join(" * ")
            };
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

/// Per-batch context `emit_function` needs beyond the function itself:
/// the owning interface's identity plus the install-set-wide name census
/// that drives collision qualification.
struct FnEmitCtx<'a> {
    iface_name: &'a str,
    fn_name_uses: &'a BTreeMap<String, u32>,
    /// True when the interface lives in the `wasi` namespace — the one
    /// namespace whose WIT the compiler carries vendored, so codegen's
    /// WIT-informed lowering can recover exact integer widths that
    /// Canon's single `Int` erases. Narrow widths inside `list`/`option`
    /// returns are only emittable when this is set; elsewhere the
    /// decode stride would be unknowable at codegen time.
    wit_informed: bool,
}

fn emit_function(
    resolve: &Resolve,
    ctx: &FnEmitCtx<'_>,
    name: &str,
    func: &Function,
    external_use_paths: &mut BTreeSet<String>,
    self_iface: InterfaceId,
) -> Result<(String, String, bool), String> {
    // Skip async + resource methods. Resource *types* are emitted as
    // `Foo = Handle` newtypes; their methods stay skipped until the
    // codegen learns to lower `own<T>`/`borrow<T>` canonical-ABI shapes
    // (see CLAUDE.md "Known codegen gaps"). The original WIT function
    // name in kebab is no longer used here — the loader recovers it
    // from the Canon camelCase identifier at patch time — so we don't
    // bind it.
    match &func.kind {
        FunctionKind::Freestanding => {}
        FunctionKind::AsyncFreestanding => return Err("async (v1 skips async)".into()),
        FunctionKind::Method(_)
        | FunctionKind::Static(_)
        | FunctionKind::Constructor(_)
        | FunctionKind::AsyncMethod(_)
        | FunctionKind::AsyncStatic(_) => {
            return Err("resource method (codegen lowering pending, see CLAUDE.md)".into());
        }
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

    // `list<T>` / `option<T>` returns: string payloads and 8-byte scalars
    // (u64/s64/f64) decode directly; narrower scalars (u8..u32, s8..s32,
    // f32, bool) need the per-width read-back whose true width codegen
    // reads from the vendored WIT — available only for `wasi:*` imports.
    // Compound payloads (records, variants, nested lists) aren't lowered
    // yet and stay skipped.
    check_payload_return(resolve, &func.result, ctx.wit_informed)?;

    // Codegen gap: a bare `result` (no `<ok, err>` payloads) as a
    // *parameter* has no Canon-value lowering yet (see
    // `wasi:cli/exit#exit`; `exit-with-code` covers the same use case
    // via plain `u8`). Bare-result *returns* decode into an ordinary
    // Canon `Result` and are emitted.
    if func.params.iter().any(|p| is_bare_result(resolve, &p.ty)) {
        return Err("bare `result` parameter (codegen gap, no ok/err payloads)".into());
    }

    // Narrow integer widths (u8..u32, s8..s32) are supported when they
    // appear as *top-level* scalars — the codegen's WIT-informed extern
    // lowering (see `collect_extern_imports` in
    // `src/codegen/wasm/mod.rs`) reads the true width from the vendored
    // WIT and inserts i64↔i32 conversions at call sites. Narrow ints
    // buried inside compounds (records, options, lists, …) still ride
    // on the unsupported compound shape and are skipped by the shape
    // checks around this one; the explicit check left here is only for
    // compounds that would otherwise slip through.
    let compound_narrow = |t: &Type| has_narrow_int(resolve, t) && !is_plain_int(resolve, t);
    if func.params.iter().any(|p| compound_narrow(&p.ty))
        || func.result.as_ref().is_some_and(|t| {
            compound_narrow(t)
                && !is_scalar_record(resolve, t)
                // Scalar list/option payloads were already vetted by
                // `check_payload_return` above — a narrow payload that
                // survived it is WIT-informed and decodable.
                && list_or_option_payload(resolve, t).is_none()
        })
    {
        return Err("sub-u64 integer inside a compound shape (codegen gap)".into());
    }

    let camel = kebab_to_camel(name);

    // Build the param product. Each WIT parameter becomes a component
    // typed by its declared WIT type; we surface the parameter *name*
    // only through the type (Canon has no named parameters), so two
    // params of the same WIT type force a newtype on the caller's side
    // anyway — same situation as hand-written stdlib bindings.
    let mut params: Vec<String> = Vec::new();
    for p in &func.params {
        params.push(render_type(resolve, &p.ty, external_use_paths, self_iface)?);
    }
    // Canon requires alphabetical product components; if two are
    // the same type that's an error the user (or the std/ wrapper)
    // must resolve. We sort here so the source is always valid for
    // the common single-of-each-type case.
    params.sort();

    // A name that collides across the install set (two interfaces both
    // exporting `now`) can't be discovered by its bare leaf name in the
    // flat, `use`-free namespace, so it takes the interface's zero-data
    // capability marker as the *first* input (`MonotonicClock => …`);
    // reference discovery then resolves on the unique constructed type
    // while the codegen drops the Unit marker so the WIT call keeps its
    // real arity. The string body carries the WIT fragment verbatim — no
    // camelCase-to-kebab derivation.
    let collides = ctx.fn_name_uses.get(&camel).copied().unwrap_or(0) > 1;

    // The binding's return type and its minted result newtype. Every
    // binding is a types-only anonymous constructor keyed by the type it
    // constructs, so each mints a distinct newtype rather than reusing a
    // WIT type name — that keeps a binding's constructed name from ever
    // colliding with a hand-written wrapper's type (the monotonic
    // `Mark = Int` wrapper vs the system-clock `instant` record). The
    // mint is interface-qualified when the WIT leaf name collides, so the
    // two `now`s become `MonotonicClockNow` / `SystemClockNow`.
    let mint_name = if collides {
        format!(
            "{}{}",
            kebab_to_pascal(ctx.iface_name),
            kebab_to_pascal(name)
        )
    } else {
        kebab_to_pascal(name)
    };
    let (ctor_return, mint) =
        binding_return(resolve, func, &mint_name, external_use_paths, self_iface)?;
    let used_capability = collides;
    let mut input_components: Vec<String> = Vec::new();
    if collides {
        input_components.push(kebab_to_pascal(ctx.iface_name));
    }
    input_components.extend(params);
    let input_str = match input_components.as_slice() {
        [] => "Unit".to_string(),
        [one] => one.clone(),
        many => format!("({})", many.join(" * ")),
    };

    let mut decl = String::new();
    if let Some((mint_name, mint_body)) = &mint {
        let _ = writeln!(decl, "{mint_name} = {mint_body}\n");
    }
    let _ = writeln!(decl, "{input_str} => {ctor_return} {{\n    \"{name}\"\n}}");

    // The declaration is keyed for alphabetical emission by the type it
    // constructs (the loader indexes anonymous constructors by that
    // name), not by the WIT leaf name.
    Ok((constructed_key(&ctor_return), decl, used_capability))
}

/// The name an anonymous-constructor binding is indexed and sorted under:
/// its constructed type, with `Result`/`Option`/`Future` peeled to the
/// payload (mirrors `ast::constructed_type_name`).
fn constructed_key(ctor_return: &str) -> String {
    let s = ctor_return.trim();
    for wrapper in ["Result<", "Option<", "Future<"] {
        if let Some(rest) = s.strip_prefix(wrapper) {
            let inner = rest.trim_start();
            let head: String = inner
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            return head;
        }
    }
    s.to_string()
}

/// Computes a binding constructor's return type and the minted result
/// newtype it declares. The mint always aliases the WIT result (the
/// `ok` payload for a `result`, the inner type for an `option`, the whole
/// type otherwise), so the constructor's constructed name is `mint_name`
/// — unique, and never a WIT type name that a wrapper might also declare.
fn binding_return(
    resolve: &Resolve,
    func: &Function,
    mint_name: &str,
    external_use_paths: &mut BTreeSet<String>,
    self_iface: InterfaceId,
) -> Result<(String, Option<(String, String)>), String> {
    let mint = mint_name.to_string();
    let Some(t) = &func.result else {
        // No WIT result — a pure effect. Mint a `Unit` marker so the
        // constructor still has a distinct constructed type.
        return Ok((mint.clone(), Some((mint, "Unit".to_string()))));
    };
    match structural_kind(resolve, t) {
        StructuralKind::Result { ok, err } => {
            let err_str = match err {
                Some(e) => render_type(resolve, &e, external_use_paths, self_iface)?,
                None => "Unit".to_string(),
            };
            let ok_str = match ok {
                Some(ok_ty) => render_type(resolve, &ok_ty, external_use_paths, self_iface)?,
                None => "Unit".to_string(),
            };
            Ok((format!("Result<{mint}, {err_str}>"), Some((mint, ok_str))))
        }
        StructuralKind::Plain => {
            let ty_str = render_type(resolve, t, external_use_paths, self_iface)?;
            Ok((mint.clone(), Some((mint, ty_str))))
        }
    }
}

enum StructuralKind {
    /// An inline `result<ok, err>` — the mint aliases the `ok` payload and
    /// the constructor returns `Result<mint, err>`, so a wrapper's `?`
    /// still sees a `Result`.
    Result { ok: Option<Type>, err: Option<Type> },
    /// A bare primitive, a named type, an `option`, or a `list` — the mint
    /// aliases the whole rendered type and the constructor returns the
    /// mint directly. Wrapping `option`/`list` in the mint (rather than
    /// minting the inner) keeps a zero-arg constructor's result type equal
    /// to its constructed name, which is what call-site inference expects.
    Plain,
}

/// Classifies a WIT result type for `binding_return`, walking transparent
/// aliases through to an inline `result`. A named type (even one whose
/// underlying kind is `result`) is `Plain` — the mint aliases it wholesale.
fn structural_kind(resolve: &Resolve, t: &Type) -> StructuralKind {
    let Type::Id(id) = t else {
        return StructuralKind::Plain;
    };
    let td = &resolve.types[*id];
    if td.name.is_some() {
        return StructuralKind::Plain;
    }
    match &td.kind {
        TypeDefKind::Result(r) => StructuralKind::Result {
            ok: r.ok,
            err: r.err,
        },
        TypeDefKind::Type(inner) => structural_kind(resolve, inner),
        _ => StructuralKind::Plain,
    }
}

/// Returns true if the function's result type is (or transparently aliases)
/// a WIT `list<T>`. Used to filter out functions whose extern import the
/// current core codegen can't lower correctly.
///
/// Supported payloads:
///   - `string` — the canonical element/payload layout matches Canon's
///     string-shaped decodes (`ListString` / `OptionString`).
///   - `u64` / `s64` / `f64` — 8-byte scalars share Canon's value layout,
///     so the decode needs no width information.
///   - narrow scalars (`u8`..`u32`, `s8`..`s32`, `f32`, `bool`) — decoded
///     by per-width read-back, but only when the vendored WIT can tell
///     codegen the true width (`wit_informed`, i.e. `wasi:*`).
///
/// Everything else (records, variants, nested lists, `char`) is skipped.
fn check_payload_return(
    resolve: &Resolve,
    result: &Option<Type>,
    wit_informed: bool,
) -> Result<(), String> {
    let Some(t) = result else { return Ok(()) };
    let Some((ctor, payload)) = list_or_option_payload_raw(resolve, t) else {
        return Ok(());
    };
    if matches!(payload, Type::String) {
        return Ok(());
    }
    let Some(prim) = wit_scalar_prim(resolve, &payload) else {
        return Err(format!(
            "{ctor}<T> return with compound payload (codegen gap, see CLAUDE.md)"
        ));
    };
    match prim {
        ScalarPrim::Wide => Ok(()),
        ScalarPrim::Narrow if wit_informed => Ok(()),
        ScalarPrim::Narrow => Err(format!(
            "narrow {ctor} payload outside `wasi:` (payload width unknowable at codegen)"
        )),
        ScalarPrim::Char => Err(format!("{ctor}<char> return (no Canon value shape)")),
    }
}

/// Width class of a scalar WIT payload, for the skip rule above.
enum ScalarPrim {
    /// `u64` / `s64` / `f64` — matches Canon's 8-byte value slots as-is.
    Wide,
    /// Every sub-8-byte scalar — decodable only with WIT width info.
    Narrow,
    /// `char` — renders as `String` in Canon, which would misclassify
    /// the decode as string-shaped; no value shape yet.
    Char,
}

/// Resolves a WIT type (walking `type x = y` aliases) to its scalar
/// width class. `None` for strings and compound shapes.
fn wit_scalar_prim(resolve: &Resolve, t: &Type) -> Option<ScalarPrim> {
    match t {
        Type::U64 | Type::S64 | Type::F64 => Some(ScalarPrim::Wide),
        Type::Bool
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::S8
        | Type::S16
        | Type::S32
        | Type::F32 => Some(ScalarPrim::Narrow),
        Type::Char => Some(ScalarPrim::Char),
        Type::Id(id) => match &resolve.types[*id].kind {
            TypeDefKind::Type(inner) => wit_scalar_prim(resolve, inner),
            _ => None,
        },
        _ => None,
    }
}

/// If `t` (after walking aliases) is a `list<T>` or `option<T>`, return
/// the constructor's name and the raw payload type.
fn list_or_option_payload_raw(resolve: &Resolve, t: &Type) -> Option<(&'static str, Type)> {
    match t {
        Type::Id(id) => match &resolve.types[*id].kind {
            TypeDefKind::Type(inner) => list_or_option_payload_raw(resolve, inner),
            TypeDefKind::List(elem) => Some(("list", *elem)),
            TypeDefKind::Option(payload) => Some(("option", *payload)),
            _ => None,
        },
        _ => None,
    }
}

/// True when `t` is a `list`/`option` whose payload is a scalar or
/// string — the shapes `check_payload_return` vets. Used to exempt them
/// from the narrow-in-compound skip below.
fn list_or_option_payload(resolve: &Resolve, t: &Type) -> Option<()> {
    let (_, payload) = list_or_option_payload_raw(resolve, t)?;
    if matches!(payload, Type::String) || wit_scalar_prim(resolve, &payload).is_some() {
        return Some(());
    }
    None
}

/// True when `t` is a WIT `result` with neither `ok` nor `err` payloads
/// (the bare `result;` form). See the parameter skip-rule comment in
/// `emit_function`.
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
/// True when `t` is a named record whose fields are all scalar
/// primitives — the shape the codegen's `ScalarRecord` indirect
/// return decodes (see `IndirectReturnShape::ScalarRecord`).
fn is_scalar_record(resolve: &Resolve, t: &Type) -> bool {
    let Type::Id(id) = t else { return false };
    match &resolve.types[*id].kind {
        TypeDefKind::Type(inner) => is_scalar_record(resolve, inner),
        TypeDefKind::Record(rec) => rec.fields.iter().all(|f| {
            matches!(
                f.ty,
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
            )
        }),
        _ => false,
    }
}

/// True when `t` is (an alias chain to) a plain WIT integer type —
/// the shape the WIT-informed extern lowering handles directly.
fn is_plain_int(resolve: &Resolve, t: &Type) -> bool {
    match t {
        Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::S8
        | Type::S16
        | Type::S32
        | Type::S64 => true,
        Type::Id(id) => match &resolve.types[*id].kind {
            TypeDefKind::Type(inner) => is_plain_int(resolve, inner),
            _ => false,
        },
        _ => false,
    }
}

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
                    r.ok.as_ref().is_some_and(|t| has_narrow_int(resolve, t))
                        || r.err.as_ref().is_some_and(|t| has_narrow_int(resolve, t))
                }
                TypeDefKind::Tuple(t) => t.types.iter().any(|t| has_narrow_int(resolve, t)),
                TypeDefKind::Record(r) => r.fields.iter().any(|f| has_narrow_int(resolve, &f.ty)),
                TypeDefKind::Variant(v) => v
                    .cases
                    .iter()
                    .any(|c| c.ty.as_ref().is_some_and(|t| has_narrow_int(resolve, t))),
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
                        .is_some_and(|t| mentions_handle_ty(resolve, t))
                        || r.err
                            .as_ref()
                            .is_some_and(|t| mentions_handle_ty(resolve, t))
                }
                TypeDefKind::Tuple(t) => t.types.iter().any(|t| mentions_handle_ty(resolve, t)),
                TypeDefKind::Record(r) => {
                    r.fields.iter().any(|f| mentions_handle_ty(resolve, &f.ty))
                }
                TypeDefKind::Variant(v) => v.cases.iter().any(|c| {
                    c.ty.as_ref()
                        .is_some_and(|t| mentions_handle_ty(resolve, t))
                }),
                TypeDefKind::Map(k, v) => {
                    mentions_handle_ty(resolve, k) || mentions_handle_ty(resolve, v)
                }
                TypeDefKind::Future(inner) | TypeDefKind::Stream(inner) => inner
                    .as_ref()
                    .is_some_and(|t| mentions_handle_ty(resolve, t)),
                _ => false,
            }
        }
    }
}

/// Render a WIT `Type` as Canon source. Returns `Err` for unsupported
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
            // Anonymous tuple in argument/return position. Canon has no
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
