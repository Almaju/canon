use crate::ast::*;
use crate::error::{CanonError, Span};
use std::collections::{HashMap, HashSet};

pub mod auto_await;

const BUILTIN_TYPES: &[&str] = &[
    "Bool",
    "Deserialize",
    "False",
    "Float",
    // Opaque, non-copyable, non-printable primitive that backs every WIT
    // `resource` type. Generated `canon/wasi/...` bindings declare each
    // resource as `Foo = Handle`. Users never write `Handle` directly —
    // they receive `Foo` values from binding constructors and thread them
    // through binding methods. The own/borrow distinction WIT exposes is
    // intentionally invisible at the source level (see the language spec §Resources):
    // the canonical-ABI lowering reads it from the WIT signature, the
    // source-level type is just `Foo`.
    "Handle",
    "Int",
    "Network",
    "Never",
    "Serialize",
    "Stderr",
    "Stdin",
    "Stdout",
    "String",
    "True",
    "Unit",
];

// `Random` used to live here as a capability marker, but the stdlib now
// owns it as a data-carrying newtype (`Random = Int`, see `std/random.can`)
// constructed via `Random()`. Random bytes aren't a capability in any
// meaningful sense — they're just data — so this matches the new layering
// where `std/` defines user-facing types and `wasi/` provides the FFI.
const CAPABILITY_TYPES: &[&str] = &["Network", "Stderr", "Stdin", "Stdout"];

fn is_capability_type(name: &str) -> bool {
    CAPABILITY_TYPES.contains(&name)
}

// `Map` and `Set` are NOT built in — they are ordinary pure-Canon stdlib
// types (`canon/std/Map`, `canon/std/Set`), so their names arrive through
// the imported typedefs like any other user type.
const BUILTIN_GENERIC_TYPES: &[&str] = &["Future", "List", "Option", "Result", "Stream"];

/// Zero-data builtin types that may be constructed with empty parens: `Unit()`.
/// `True()` and `False()` are covered by `is_variant` (variants of `Bool`).
/// `List()` is the empty list — the type's zero value, and the base case
/// recursive std collections (`canon/std/Map`'s `keys`, `canon/std/Set`'s
/// `List`) build up from via `concat`.
const ZERO_DATA_BUILTINS: &[&str] = &["False", "List", "True", "Unit"];

/// Synthetic concurrency combinators recognised by the codegen as built-in
/// `compile_parallel` / `compile_race` paths. They appear in source as if
/// they were ordinary calls (`parallel(a, b)`, `race(a, b)`) but the
/// checker accepts them without resolving the bindings declaration in
/// `packages/canon/std/src/canon/builtins@0.1.0/concurrent.can` — the declaration's PascalCase
/// first parameter (`Future<T>`) would otherwise force the checker to
/// look them up as methods on `Future`, which the `Constructor(…)` call
/// shape doesn't support. See `compile_parallel` in `src/codegen/wasm/mod.rs`.
const CONCURRENT_COMBINATORS: &[&str] = &["parallel", "race", "Parallel", "Race"];

pub struct SymbolTable {
    pub types: HashSet<String>,
    pub generic_types: HashSet<String>,
    pub variant_of: HashMap<String, String>,
    pub methods: HashMap<(String, String), MethodSig>,
    /// For each product TypeDef `T = A * B * ...`, the names of its
    /// component types (in declaration order). Used to validate
    /// `value.Field` access.
    pub product_fields: HashMap<String, Vec<String>>,
    /// Type names that have an explicit `TypeDef` in this module.
    /// Used to distinguish user-defined types (which resolve to themselves
    /// in method lookup) from bare variant tags (which widen to the parent).
    pub standalone_types: HashSet<String>,
    /// Free functions — functions declared *without* a receiver, like
    /// `Now = () -> Now` or `randomInt = () -> Int`. The user invokes them
    /// with constructor syntax (`Now()`, `randomInt()`), which the parser
    /// uniformly produces as `Expr::Constructor`. The checker consults this
    /// map to accept those calls even when the name isn't a type.
    pub free_funcs: HashMap<String, MethodSig>,
    /// One-level type aliases: each entry `A -> B` records that the user
    /// wrote `A = B`. The checker walks this chain (via `resolve_alias`)
    /// when looking up methods on a value so that methods declared on the
    /// underlying type are accessible on the alias — e.g. `"hello".print`
    /// on `Path` (which is `Path = String`) resolves through to `String`'s
    /// `print` method without anyone having to redeclare it for `Path`.
    pub aliases: HashMap<String, String>,
}

pub struct MethodSig {
    pub arity: usize,
    pub return_ty: String,
    /// When the method's return type is `Result<X, Y>` (or `Option<X>`),
    /// the type name `X`. Used by `?` to compute the type of the extracted
    /// payload — e.g. `path.File()?` is typed as `File` rather than
    /// `<unknown>` so subsequent method dispatch on the payload works.
    pub result_ok_ty: Option<String>,
}

impl SymbolTable {
    /// Walks the alias chain starting at `name` and returns the underlying
    /// type name. Falls back to `name` itself when there's no alias entry,
    /// and is bounded against cycles by a depth cap.
    pub fn resolve_alias<'a>(&'a self, name: &'a str) -> &'a str {
        let mut current = name;
        let mut depth = 0;
        while depth < 20 {
            match self.aliases.get(current) {
                Some(next) => {
                    current = next.as_str();
                    depth += 1;
                }
                None => break,
            }
        }
        current
    }

    pub fn knows_type(&self, name: &str) -> bool {
        self.types.contains(name) || self.generic_types.contains(name)
    }
}

pub fn check(module: &Module) -> Vec<CanonError> {
    check_with_entry(module, 0)
}

/// The compiler's front-door check: the format phase fused with the
/// semantic checker, over a loaded target. Formatting is part of the
/// language, so each user-authored source that has drifted from
/// canonical form contributes a `FormatError` (spanning its first
/// divergence) to the same error list as sort-order and type errors —
/// one run reports both. Skips `.md` assets (their `LoadedSource`
/// carries synthesized Canon the author never edits) and sources that
/// don't parse (the parse diagnostic is better located); bundled
/// packages never appear in `local_sources`. `check_with_entry` stays
/// the AST-only layer for callers without source text (fixtures,
/// synthetic-module tests).
pub fn check_loaded(loaded: &crate::loader::LoadResult) -> Vec<CanonError> {
    let mut errors: Vec<CanonError> = loaded
        .local_sources
        .iter()
        .filter(|src| src.path.extension().and_then(|e| e.to_str()) != Some("md"))
        .filter_map(|src| {
            crate::formatter::format_error(&src.source, &src.path.display().to_string())
        })
        .collect();
    errors.extend(check_with_entry(&loaded.module, loaded.entry_items_start));
    errors
}

/// Variant of `check` that limits per-file ordering rules (free-function
/// and type-definition alphabetical order) to items at or after
/// `entry_items_start`. Items before that index originated from `use`
/// imports and follow their own ordering — they are not the entry file's
/// concern.
pub fn check_with_entry(module: &Module, entry_items_start: usize) -> Vec<CanonError> {
    let mut errors = Vec::new();
    let symbols = collect_symbols(module, &mut errors);

    let mut main_found = false;
    for item in &module.items {
        match item {
            Item::Function(func) => check_function(func, &symbols, &mut errors, &mut main_found),
            Item::TypeDef(td) => check_type_def(td, &symbols, &mut errors),
        }
    }

    // Detect HTTP entries (free functions returning `Response` or
    // `Result<Response, _>`). See the entry-point rule in the language
    // spec (docs/src/spec/functions.md).
    let http_entries: Vec<&FunctionDef> = module.items[entry_items_start..]
        .iter()
        .filter_map(|item| match item {
            Item::Function(func)
                if func.receiver.is_none()
                    && entry_world_of(&func.return_ty) == Some(EntryWorld::Http) =>
            {
                Some(func)
            }
            _ => None,
        })
        .collect();

    let http_entry_name = http_entries.first().map(|f| f.name.name.as_str());

    check_ordering(module, entry_items_start, http_entry_name, &mut errors);

    // Detect the web-app entry triple (view / init / update, see
    // docs/src/reference/web-target.md). Scanned over the whole module —
    // the marker newtypes (`Init` / `Update`) alias-resolve to the model
    // through type definitions that may live in sibling files, and the
    // detection is uniqueness-guarded so imports can't create a false
    // positive. Codegen resolves it the same way.
    let web_entry = crate::ast::find_web_entry(&module.items);

    match (main_found, http_entries.len(), web_entry.is_some()) {
        // CLI program: `main` exists, no other entry. Existing behaviour.
        (true, 0, false) => {}
        // Library or malformed: no entry shape is present.
        (false, 0, false) => errors.push(CanonError::CheckError {
            message: "no entry point defined: expected a CLI entry (`Args => Exit`), an \
                      HTTP handler (`Request => Response`), or a web-app triple (a \
                      `Model => Html` view with its `Unit => Init` and `Model * Msg => Update` \
                      constructors)."
                .to_string(),
            span: module.span,
        }),
        // Mixed worlds: a component exports exactly one world.
        (true, n, _) if n > 0 => errors.push(CanonError::CheckError {
            message: format!(
                "mixed worlds: this module defines a CLI entry (`Args => Exit`) and also `{}` \
                  returning `Response` (HTTP entry). A component exports exactly one world. \
                  Remove one.",
                http_entries[0].name.name
            ),
            span: http_entries[0].span,
        }),
        (true, 0, true) => errors.push(CanonError::CheckError {
            message: "mixed worlds: this module defines a CLI entry (`Args => Exit`) and also \
                      the `init`/`update`/`view` triple (web app). A component exports \
                      exactly one world. Remove one."
                .to_string(),
            span: module.span,
        }),
        (false, n, true) if n > 0 => errors.push(CanonError::CheckError {
            message: format!(
                "mixed worlds: this module defines `{}` returning `Response` (HTTP entry) \
                  and also the `init`/`update`/`view` triple (web app). A component exports \
                  exactly one world. Remove one.",
                http_entries[0].name.name
            ),
            span: http_entries[0].span,
        }),
        // Ambiguous HTTP entry.
        (false, n, false) if n > 1 => errors.push(CanonError::CheckError {
            message: format!(
                "ambiguous HTTP entry: `{}` and `{}` both return `Response`. Exactly one \
                  free function may be the entry. Refactor helpers to return a non-world type.",
                http_entries[0].name.name, http_entries[1].name.name
            ),
            span: http_entries[1].span,
        }),
        // Exactly one HTTP entry: a well-formed `wasi:http/service`
        // program. Codegen routes it through `wrap_http_service`.
        (false, 1, false) => {}
        // Web app: the triple is present and no other world competes.
        // Codegen routes it through `compile_web`.
        (false, 0, true) => {}
        _ => unreachable!(),
    }

    // Dead code is an error, not a warning: unreachable declarations are
    // not allowed to accumulate. (Empty for library modules — see
    // `lint_dead_code`.)
    errors.extend(lint_dead_code(module, entry_items_start));

    errors
}

fn check_ordering(
    module: &Module,
    entry_items_start: usize,
    http_entry_name: Option<&str>,
    errors: &mut Vec<CanonError>,
) {
    let entry_items = &module.items[entry_items_start..];
    // Union variants and product fields are checked in check_type_expr (covers
    // every position they appear in, not just top-level TypeDef bodies).

    // Function declarations in the entry file must be alphabetical by
    // *surface* name — the single sequence `canon check --fix` sorts (constructors
    // spell their type name; a self-named constructor is rewritten to
    // `Self` by `resolve_new_syntax`, so map it back). Constructors,
    // shape implementations, and free functions all share the one
    // sequence: a per-receiver-only check would never compare two
    // anonymous arrows constructing different types. Equal surface names
    // (one shape's implementations for several receivers) keep their
    // written order — `canon check --fix`'s sort is stable, so the checker
    // accepts any order among equals. Imported items are exempt — they
    // follow their own file's ordering. `main` is also exempt: it's the
    // entry point, a distinguished role rather than a regular free
    // function, and forcing it into alphabetical position with peers (or
    // with synthesised mains produced by `canon test`) is arbitrary.
    let funcs: Vec<(&str, crate::error::Span)> = entry_items
        .iter()
        .filter_map(|item| {
            if let Item::Function(func) = item {
                if func.name.name == "main" || Some(func.name.name.as_str()) == http_entry_name {
                    return None;
                }
                let surface = if func.name.name == "Self" {
                    func.receiver
                        .as_ref()
                        .map(|r| r.name.as_str())
                        .unwrap_or(func.name.name.as_str())
                } else {
                    func.name.name.as_str()
                };
                return Some((surface, func.name.span));
            }
            None
        })
        .collect();
    check_sorted_named("function declaration", &funcs, errors);

    // Type definitions in the entry file must be alphabetical.
    let type_defs: Vec<(&str, crate::error::Span)> = entry_items
        .iter()
        .filter_map(|item| {
            if let Item::TypeDef(td) = item {
                Some((td.name.name.as_str(), td.name.span))
            } else {
                None
            }
        })
        .collect();
    check_sorted_named("type definition", &type_defs, errors);
}

fn check_sorted_named(
    kind: &str,
    items: &[(&str, crate::error::Span)],
    errors: &mut Vec<CanonError>,
) {
    for window in items.windows(2) {
        let (prev, _) = window[0];
        let (next, span) = window[1];
        if next < prev {
            errors.push(CanonError::CheckError {
                message: format!(
                    "{}s must be in alphabetical order: `{}` should come before `{}`",
                    kind, next, prev
                ),
                span,
            });
        }
    }
}

/// Dead-code lint: entry-file declarations not reachable from the
/// entry point (`main` or the HTTP handler).
///
/// Canon has no private visibility and no comments — the code *is* the
/// documentation — so unreachable declarations are pure noise and get
/// flagged (see the language spec § Dead Code). The walk is name-based and
/// conservative: a method call `x.foo()` marks every declaration named
/// `foo`, a type mention marks the type and (through its definition)
/// its variants, and declarations sharing a name (trait impls,
/// validated constructors) live or die together.
///
/// Returns one error per unused name. Empty when the module has no
/// entry point: a library's declarations are all exported surface, so
/// there is nothing to flag. Only entry-file items
/// (`entry_items_start..`) are reported — imported files are their own
/// compilation concern.
pub fn lint_dead_code(module: &Module, entry_items_start: usize) -> Vec<CanonError> {
    let entry_items = &module.items[entry_items_start..];

    let mut seeds: HashSet<String> = HashSet::new();
    for item in entry_items {
        if let Item::Function(func) = item {
            if func.receiver.is_none()
                && (func.name.name == "main"
                    || entry_world_of(&func.return_ty) == Some(EntryWorld::Http))
            {
                seeds.insert(func.name.name.clone());
            }
        }
    }
    if seeds.is_empty() {
        return Vec::new();
    }

    // name -> union of names referenced by every declaration of it.
    let mut refs: HashMap<String, HashSet<String>> = HashMap::new();
    let mut declared_order: Vec<String> = Vec::new();
    let mut declared: HashSet<String> = HashSet::new();
    for item in entry_items {
        let (name, out) = match item {
            Item::Function(func) => {
                let mut out = HashSet::new();
                if let Some(r) = &func.receiver {
                    out.insert(r.name.clone());
                }
                for p in &func.params {
                    collect_type_names(&p.ty, &mut out);
                }
                collect_type_names(&func.return_ty, &mut out);
                for e in &func.body.exprs {
                    collect_expr_names(e, &mut out);
                }
                // A self-constructor (`() => IndexBody { … }`, rewritten
                // to name `Self` with receiver `IndexBody`) is reached
                // through a `IndexBody()` call, which references the
                // *type* name, not `Self`. Key it under the receiver so
                // reachability connects — otherwise every anonymous/
                // self constructor reads as dead code.
                let key = if func.name.name == "Self" {
                    func.receiver
                        .as_ref()
                        .map(|r| r.name.clone())
                        .unwrap_or_else(|| func.name.name.clone())
                } else {
                    func.name.name.clone()
                };
                (key, out)
            }
            Item::TypeDef(td) => {
                let mut out = HashSet::new();
                collect_type_names(&td.body, &mut out);
                // A union is alive through its variants: dispatching on
                // `Dark()` uses `Mode = Dark + Light` even when `Mode`
                // itself is never spelled. Record the reverse edge
                // variant → union.
                if let TypeExpr::Union { variants, .. } = &td.body {
                    for v in variants {
                        if let TypeExpr::Named { name: vname, .. } = v {
                            refs.entry(vname.clone())
                                .or_default()
                                .insert(td.name.name.clone());
                        }
                    }
                }
                (td.name.name.clone(), out)
            }
        };
        if declared.insert(name.clone()) {
            declared_order.push(name.clone());
        }
        refs.entry(name).or_default().extend(out);
    }

    let mut reached: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = seeds.into_iter().collect();
    while let Some(n) = queue.pop() {
        if !reached.insert(n.clone()) {
            continue;
        }
        if let Some(out) = refs.get(&n) {
            for o in out {
                if !reached.contains(o) {
                    queue.push(o.clone());
                }
            }
        }
    }

    // Span lookup for reporting: the first declaration of each name.
    let mut spans: HashMap<&str, crate::error::Span> = HashMap::new();
    for item in entry_items {
        let (name, span) = match item {
            Item::Function(func) => {
                let key = if func.name.name == "Self" {
                    func.receiver
                        .as_ref()
                        .map(|r| r.name.as_str())
                        .unwrap_or(func.name.name.as_str())
                } else {
                    func.name.name.as_str()
                };
                (key, func.name.span)
            }
            Item::TypeDef(td) => (td.name.name.as_str(), td.name.span),
        };
        spans.entry(name).or_insert(span);
    }

    declared_order
        .iter()
        .filter(|n| !reached.contains(*n))
        .map(|n| CanonError::CheckError {
            message: format!(
                "`{}` is never used: dead code is not allowed to accumulate; \
                 delete it or wire it into the program",
                n
            ),
            span: spans.get(n.as_str()).copied().unwrap_or_default(),
        })
        .collect()
}

fn collect_type_names(ty: &TypeExpr, out: &mut HashSet<String>) {
    match ty {
        TypeExpr::Named { name, generics, .. } => {
            out.insert(name.clone());
            for g in generics {
                collect_type_names(g, out);
            }
        }
        TypeExpr::Union { variants, .. } => {
            for v in variants {
                collect_type_names(v, out);
            }
        }
        TypeExpr::Product { fields, .. } => {
            for f in fields {
                collect_type_names(f, out);
            }
        }
        TypeExpr::Repeat { ty, .. } | TypeExpr::Spread { ty, .. } => collect_type_names(ty, out),
        TypeExpr::Function {
            params, return_ty, ..
        } => {
            for p in params {
                collect_type_names(p, out);
            }
            collect_type_names(return_ty, out);
        }
    }
}

fn collect_expr_names(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Ident(id) => {
            out.insert(id.name.clone());
        }
        Expr::Constructor { name, args, .. } => {
            out.insert(name.name.clone());
            for a in args {
                collect_expr_names(a, out);
            }
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            out.insert(method.name.clone());
            collect_expr_names(receiver, out);
            for a in args {
                collect_expr_names(a, out);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_expr_names(scrutinee, out);
            for arm in arms {
                collect_type_names(&arm.param_ty, out);
                collect_type_names(&arm.return_ty, out);
                for e in &arm.body.exprs {
                    collect_expr_names(e, out);
                }
            }
        }
        Expr::Try { inner, .. } | Expr::Await { inner, .. } => collect_expr_names(inner, out),
        Expr::Lambda {
            params,
            return_ty,
            body,
            ..
        } => {
            for p in params {
                collect_type_names(&p.ty, out);
            }
            collect_type_names(return_ty, out);
            for e in &body.exprs {
                collect_expr_names(e, out);
            }
        }
        Expr::ProductValue { fields, .. } => {
            for f in fields {
                collect_expr_names(f, out);
            }
        }
        Expr::FieldAccess {
            receiver, field, ..
        } => {
            collect_expr_names(receiver, out);
            out.insert(field.name.clone());
        }
        Expr::JsonLit { parts, .. } => {
            for p in parts {
                if let JsonLitPart::Interp(e) = p {
                    collect_expr_names(e, out);
                }
            }
        }
        Expr::HtmlLit { parts, .. } => {
            for p in parts {
                if let HtmlLitPart::Interp(e) = p {
                    collect_expr_names(e, out);
                }
            }
        }
        Expr::FormatLit { parts, .. } => {
            for p in parts {
                if let FormatLitPart::Interp(e) = p {
                    collect_expr_names(e, out);
                }
            }
        }
        Expr::StringLit { .. } | Expr::IntLit { .. } | Expr::FloatLit { .. } => {}
    }
}

// ---------------------------------------------------------------------------
// Known codegen gaps
//
// The checker deliberately accepts more than the code generator implements:
// a handful of features parse and type-check but fail later, at codegen. That
// is a silent trap for users — a program passes `canon check`, then blows up
// at `canon build`. To close the gap in *feedback* (not the gap in codegen),
// the checker emits a non-fatal **warning** when a program reaches, from its
// entry, a declaration that relies on one of these features. The warning
// points at `docs/src/reference/codegen-gaps.md`, the tracking page.
//
// `CODEGEN_GAPS` below is the single source of truth for the list; the doc
// page mirrors it in prose and `tests/codegen_gaps.rs` pins that every gap
// here is documented there.
// ---------------------------------------------------------------------------

/// One feature the checker accepts but the code generator does not implement
/// yet. See the module-level comment above and the doc page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodegenGap {
    /// Short description, phrased to read both inside the warning sentence
    /// and as the heading for this gap in `codegen-gaps.md`. The doc-mirror
    /// test matches on this string verbatim, so the two stay in lockstep.
    pub title: &'static str,
}

/// Binding declarations returning `list<T>` for compound `T`. String and
/// scalar elements (including narrow widths, whose stride the codegen
/// reads back from the vendored WIT) decode; lists of records/variants
/// don't.
pub const GAP_LIST_COMPOUND_RETURN: CodegenGap = CodegenGap {
    title: "binding declarations returning `list<T>` for compound `T`",
};

/// Sub-`u64` integers (`u8`/`u16`/`u32`/`s8`/`s16`/`s32`) inside a compound
/// WIT shape. Not statically detectable from Canon source (Canon has only
/// `Int`), so documented but not warned about.
pub const GAP_SUB_U64_COMPOUND: CodegenGap = CodegenGap {
    title: "sub-`u64` integers inside a compound WIT shape",
};

/// WIT `result` with no payloads as a binding *parameter*. (Bare-result
/// returns decode into an ordinary Canon `Result` now.) Not statically
/// detectable from Canon source, so documented but not warned about.
pub const GAP_RESULT_NO_PAYLOAD: CodegenGap = CodegenGap {
    title: "WIT `result` with no payloads as a binding parameter",
};

/// Binding declarations returning `option<T>` for compound `T`. String
/// and scalar payloads decode; option-of-record/variant payloads don't.
pub const GAP_OPTION_COMPOUND_RETURN: CodegenGap = CodegenGap {
    title: "binding declarations returning `option<T>` for compound `T`",
};

/// WIT `resource` / `own<T>` / `borrow<T>` in binding signatures. Bindgen
/// skips the offending functions, so they rarely survive as declarations;
/// documented but not warned about to avoid flagging the hand-written
/// wrappers that supersede them.
pub const GAP_RESOURCES: CodegenGap = CodegenGap {
    title: "WIT `resource` / `own<T>` / `borrow<T>` in binding signatures",
};

/// `At(i)` / `First` on `List<String>` and nested `Mapped` — codegen erases
/// the element type. A runtime-value concern, documented but not warned about.
pub const GAP_LIST_STRING_INDEXING: CodegenGap = CodegenGap {
    title: "`At(i)` / `First` on `List<String>` and nested `Mapped`",
};

/// HTTP handler request headers and body. The handler body compiles, but
/// reading request headers/body is not wired up. Documented but not warned
/// about (not statically distinguishable from a working handler).
pub const GAP_HTTP_REQUEST_HEADERS_BODY: CodegenGap = CodegenGap {
    title: "HTTP handler request headers and body",
};

/// `Stream<T>` lowering and streaming response bodies. Codegen drops imports
/// whose signatures mention `Stream<T>`, so such programs fail to link.
pub const GAP_STREAM: CodegenGap = CodegenGap {
    title: "`Stream<T>` lowering and streaming response bodies",
};

/// Every known codegen gap, in the same order as the doc page. Single source
/// of truth for the list.
pub const CODEGEN_GAPS: &[CodegenGap] = &[
    GAP_LIST_COMPOUND_RETURN,
    GAP_SUB_U64_COMPOUND,
    GAP_RESULT_NO_PAYLOAD,
    GAP_OPTION_COMPOUND_RETURN,
    GAP_RESOURCES,
    GAP_LIST_STRING_INDEXING,
    GAP_HTTP_REQUEST_HEADERS_BODY,
    GAP_STREAM,
];

/// A non-fatal codegen-gap diagnostic. The program type-checks; this warns
/// that code generation will reject it. Kept separate from the fatal
/// `CanonError` stream so callers never treat it as a build failure.
#[derive(Debug, Clone)]
pub struct GapWarning {
    pub message: String,
    pub span: crate::error::Span,
}

/// Scan a fully-loaded module for *reachable* use of features the code
/// generator doesn't implement yet, returning one warning per offending
/// declaration. Intended to run after `check` succeeds (only clean programs
/// reach codegen).
///
/// Reachability is computed from the entry file: the loader is file-granular,
/// so referencing one binding pulls in every sibling binding in the same
/// file. Warning on all of them would be noise; we warn only on declarations
/// actually reachable from the entry, which are exactly the ones a build will
/// try to compile.
pub fn codegen_gap_warnings(module: &Module, entry_items_start: usize) -> Vec<GapWarning> {
    let reachable = reachable_decl_names(module, entry_items_start);

    // Alias map so payload checks can chase user newtypes (`Duration =
    // Int`) to the scalar they erase to.
    let mut type_defs: HashMap<String, TypeExpr> = HashMap::new();
    for item in &module.items {
        if let Item::TypeDef(td) = item {
            type_defs.insert(td.name.name.clone(), td.body.clone());
        }
    }

    let mut warnings = Vec::new();
    let mut seen: HashSet<(usize, usize, &'static str)> = HashSet::new();
    for item in &module.items {
        let Item::Function(func) = item else { continue };
        if !reachable.contains(&decl_key(func)) {
            continue;
        }
        for gap in detect_fn_gaps(func, &type_defs) {
            let span = func.name.span;
            if seen.insert((span.start, span.end, gap.title)) {
                warnings.push(GapWarning {
                    message: gap_warning_message(&gap),
                    span,
                });
            }
        }
    }
    warnings
}

fn gap_warning_message(gap: &CodegenGap) -> String {
    format!(
        "this program reaches a feature the code generator hasn't implemented yet — {} — \
         so it type-checks but `canon build` will fail during code generation. \
         Tracked in docs/src/reference/codegen-gaps.md.",
        gap.title
    )
}

/// The codegen gaps a single declaration triggers. Only the gaps that are
/// statically detectable from Canon source are matched here; the rest live in
/// `CODEGEN_GAPS` (and the doc page) for tracking but never warn.
fn detect_fn_gaps(func: &FunctionDef, type_defs: &HashMap<String, TypeExpr>) -> Vec<CodegenGap> {
    let mut gaps = Vec::new();

    // `Stream<T>` anywhere in the signature: codegen drops imports whose
    // signatures mention it, and can't lower a `Stream` return. Applies to
    // any function, not just externs — a plain helper returning `Stream<T>`
    // fails to link the same way.
    if sig_mentions(func, "Stream") {
        gaps.push(GAP_STREAM);
    }

    // The remaining detectable gaps are properties of *binding* declarations.
    if func.extern_wasm.is_some() {
        let ret = unwrap_binding_return(&func.return_ty);
        if let Some(elem) = generic_arg(ret, "List") {
            if !is_scalar_or_string_payload(elem, type_defs) {
                gaps.push(GAP_LIST_COMPOUND_RETURN);
            }
        }
        if let Some(elem) = generic_arg(ret, "Option") {
            if !is_scalar_or_string_payload(elem, type_defs) {
                gaps.push(GAP_OPTION_COMPOUND_RETURN);
            }
        }
    }

    gaps
}

/// True when a `List<T>` / `Option<T>` binding-return payload is a shape
/// the extern decode implements: `String`, a scalar (`Int` / `Float` /
/// `Bool` and the prelude `Int`-aliases), or a user alias chain ending in
/// one of those. Compound payloads (products, unions, nested generics)
/// are the remaining gap.
fn is_scalar_or_string_payload(ty: &TypeExpr, type_defs: &HashMap<String, TypeExpr>) -> bool {
    let mut cur = ty;
    for _ in 0..20 {
        let TypeExpr::Named { name, generics, .. } = cur else {
            return false;
        };
        if !generics.is_empty() {
            return false;
        }
        if matches!(
            name.as_str(),
            "String" | "Int" | "Float" | "Bool" | "Byte" | "Hex"
        ) {
            return true;
        }
        match type_defs.get(name) {
            Some(body) => cur = body,
            None => return false,
        }
    }
    false
}

/// Peel the wrappers `apply_bindings` may leave on an extern's return so the
/// list/option element check sees the underlying shape: `Future<T>` (async
/// unwrap) and `Result<T, _>` (the success arm carries the decoded value).
fn unwrap_binding_return(ty: &TypeExpr) -> &TypeExpr {
    let mut cur = ty;
    loop {
        match generic_arg(cur, "Future").or_else(|| generic_arg(cur, "Result")) {
            Some(inner) => cur = inner,
            None => return cur,
        }
    }
}

/// The sole generic argument of `Ctor<Arg>`, if `ty` is exactly that.
fn generic_arg<'a>(ty: &'a TypeExpr, ctor: &str) -> Option<&'a TypeExpr> {
    match ty {
        TypeExpr::Named { name, generics, .. } if name == ctor && generics.len() == 1 => {
            Some(&generics[0])
        }
        _ => None,
    }
}

/// Whether any type in the function's signature (parameters or return)
/// mentions `ty_name`.
fn sig_mentions(func: &FunctionDef, ty_name: &str) -> bool {
    let mut names = HashSet::new();
    for p in &func.params {
        collect_type_names(&p.ty, &mut names);
    }
    collect_type_names(&func.return_ty, &mut names);
    names.contains(ty_name)
}

/// The name a function declaration is reached *by*: its own name, except a
/// self-constructor (rewritten to `Self`) is reached through its receiver
/// type name — mirrors the keying in `lint_dead_code`.
fn decl_key(func: &FunctionDef) -> String {
    if func.name.name == "Self" {
        func.receiver
            .as_ref()
            .map(|r| r.name.clone())
            .unwrap_or_else(|| func.name.name.clone())
    } else {
        func.name.name.clone()
    }
}

/// The referenced-name closure over the *whole* module (imports included),
/// seeded from the entry file. Every entry-file declaration is reachable
/// (unreachable ones are a dead-code error, so a clean program has none);
/// walking their references descends into exactly the imported bindings the
/// program actually uses.
fn reachable_decl_names(module: &Module, entry_items_start: usize) -> HashSet<String> {
    let mut refs: HashMap<String, HashSet<String>> = HashMap::new();
    for item in &module.items {
        let (name, out) = item_refs(item);
        refs.entry(name).or_default().extend(out);
        // A union is alive through its variants: record the reverse edge
        // variant → union so dispatching on `Dark()` keeps `Mode` and its
        // siblings reachable (mirrors `lint_dead_code`).
        if let Item::TypeDef(td) = item {
            if let TypeExpr::Union { variants, .. } = &td.body {
                for v in variants {
                    if let TypeExpr::Named { name: vname, .. } = v {
                        refs.entry(vname.clone())
                            .or_default()
                            .insert(td.name.name.clone());
                    }
                }
            }
        }
    }

    let mut queue: Vec<String> = Vec::new();
    for item in &module.items[entry_items_start..] {
        let (name, out) = item_refs(item);
        queue.push(name);
        queue.extend(out);
    }

    let mut reached: HashSet<String> = HashSet::new();
    while let Some(n) = queue.pop() {
        if !reached.insert(n.clone()) {
            continue;
        }
        if let Some(out) = refs.get(&n) {
            for o in out {
                if !reached.contains(o) {
                    queue.push(o.clone());
                }
            }
        }
    }
    reached
}

/// The reachability key of an item and the set of names it references.
/// Shared shape with `lint_dead_code`'s inline collection.
fn item_refs(item: &Item) -> (String, HashSet<String>) {
    match item {
        Item::Function(func) => {
            let mut out = HashSet::new();
            if let Some(r) = &func.receiver {
                out.insert(r.name.clone());
            }
            for p in &func.params {
                collect_type_names(&p.ty, &mut out);
            }
            collect_type_names(&func.return_ty, &mut out);
            for e in &func.body.exprs {
                collect_expr_names(e, &mut out);
            }
            (decl_key(func), out)
        }
        Item::TypeDef(td) => {
            let mut out = HashSet::new();
            collect_type_names(&td.body, &mut out);
            (td.name.name.clone(), out)
        }
    }
}

/// Build a module's `SymbolTable`, discarding diagnostics. Tooling
/// entry point — LSP completion needs the alias/variant/method indexes
/// over a possibly half-typed buffer, where duplicate-definition noise
/// (the buffer plus its own import closure) is expected and harmless.
pub(crate) fn symbols_for_tooling(module: &Module) -> SymbolTable {
    let mut errors = Vec::new();
    collect_symbols(module, &mut errors)
}

fn collect_symbols(module: &Module, errors: &mut Vec<CanonError>) -> SymbolTable {
    let mut types: HashSet<String> = BUILTIN_TYPES.iter().map(|s| s.to_string()).collect();
    let mut generic_types: HashSet<String> = BUILTIN_GENERIC_TYPES
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut variant_of: HashMap<String, String> = HashMap::new();

    variant_of.insert("None".to_string(), "Option".to_string());
    variant_of.insert("Some".to_string(), "Option".to_string());
    variant_of.insert("Ok".to_string(), "Result".to_string());
    variant_of.insert("Err".to_string(), "Result".to_string());
    types.insert("None".to_string());
    types.insert("Some".to_string());
    types.insert("Ok".to_string());
    types.insert("Err".to_string());
    variant_of.insert("False".to_string(), "Bool".to_string());
    variant_of.insert("True".to_string(), "Bool".to_string());

    let mut type_canon: HashMap<String, String> = HashMap::new();
    for item in &module.items {
        if let Item::TypeDef(td) = item {
            let name = td.name.name.clone();
            let canon = crate::ast::type_expr_canonical(&td.body);
            let already_known = types.contains(&name) || generic_types.contains(&name);
            if already_known {
                // Structurally identical duplicates merge — type
                // equality is syntactic (one canonical spelling per
                // type), so `Length = Int` declared by two loaded files
                // is one type, not a clash. Only a *differing* body
                // under the same name is an error.
                if type_canon.get(&name) != Some(&canon) {
                    errors.push(CanonError::CheckError {
                        message: format!("duplicate type definition `{}`", name),
                        span: td.name.span,
                    });
                }
            } else if td.generic_params.is_empty() {
                types.insert(name.clone());
                type_canon.insert(name, canon);
            } else {
                generic_types.insert(name.clone());
                type_canon.insert(name, canon);
            }
        }
    }

    for item in &module.items {
        if let Item::TypeDef(td) = item {
            if let TypeExpr::Union { variants, .. } = &td.body {
                for variant in variants {
                    if let Some(name) = variant.simple_name() {
                        let name_s = name.to_string();
                        // A variant may also have its own TypeDef (carrying a
                        // payload). Register `types` only if it isn't already
                        // there, but always record the variant → union link
                        // so dispatch patterns and constructor-type lookups
                        // resolve correctly.
                        if !types.contains(&name_s) && !generic_types.contains(&name_s) {
                            types.insert(name_s.clone());
                        }
                        variant_of
                            .entry(name_s)
                            .or_insert_with(|| td.name.name.clone());
                    }
                }
            }
        }
    }

    // Field access table. Two shapes of typedef contribute:
    //
    //   * `T = A * B * ...`  — a real product; each named component is a
    //     field on `T`.
    //   * `T = U`           — a newtype alias; `T` has a single field
    //     named after the underlying type `U` (see the language spec § "Newtypes
    //     Are 1-Component Products"). This makes `value.U` a valid
    //     unwrap expression: `Greeting("hi").String` yields the inner
    //     `String`. Method lookup still walks the alias chain (so
    //     `Greeting("hi").print()` works without an explicit unwrap),
    //     but the unwrap form exists for cases where the explicit step
    //     reads more clearly.
    let mut product_fields: HashMap<String, Vec<String>> = HashMap::new();
    for item in &module.items {
        if let Item::TypeDef(td) = item {
            match &td.body {
                TypeExpr::Product { fields, .. } => {
                    let names: Vec<String> = fields
                        .iter()
                        .filter_map(|f| {
                            if let TypeExpr::Named { name, .. } = f {
                                Some(name.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !names.is_empty() {
                        product_fields.insert(td.name.name.clone(), names);
                    }
                }
                TypeExpr::Named { name, .. } => {
                    // Newtype `T = U` (or `T = U<…>`): one component named
                    // after the underlying type. The generic args don't
                    // affect the field's name — `MessageContent = Option<Content>`
                    // still has a single field named `Option`. See the language spec
                    // § "Newtypes Are 1-Component Products".
                    product_fields.insert(td.name.name.clone(), vec![name.clone()]);
                }
                _ => {}
            }
        }
    }

    let mut methods: HashMap<(String, String), MethodSig> = HashMap::new();
    for item in &module.items {
        if let Item::Function(func) = item {
            if let Some(recv) = &func.receiver {
                let (return_ty, result_ok_ty) = method_return_summary(&func.return_ty);
                let primary_key = (recv.name.clone(), func.name.name.clone());
                let primary_sig = MethodSig {
                    arity: func.params.len(),
                    return_ty: return_ty.clone(),
                    result_ok_ty: result_ok_ty.clone(),
                };
                // Constructor families: several `Self` members share the
                // `(Type, "Self")` key. The zero-arg member owns it — the
                // `T()` legality check reads this slot's arity — while
                // parameterized members are typed through the per-param
                // commutative entries below. Mirrors codegen's
                // `assign_func_indices` keying.
                if func.name.name == "Self" && !func.params.is_empty() {
                    methods.entry(primary_key).or_insert(primary_sig);
                } else {
                    methods.insert(primary_key, primary_sig);
                }
                // Register under each param type for commutative calling.
                // For constructors (name == "Self"), also register the TYPE NAME as the method
                // so that `param_val.TypeName()` (commutative constructor call) is recognized.
                // For product-type params (A * B), register each component separately.
                // `ctor_arity` is the number of remaining args when that component is the receiver.
                let mut components: Vec<String> = Vec::new();
                for param in &func.params {
                    match &param.ty {
                        TypeExpr::Named { .. } => {
                            if let Some(n) = param.ty.simple_name() {
                                components.push(n.to_string());
                            }
                        }
                        TypeExpr::Product { fields, .. } => {
                            for field in fields {
                                if let Some(n) = field.simple_name() {
                                    components.push(n.to_string());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                // When one component is the receiver, the caller passes
                // the rest as arguments — the same count regardless of
                // whether the components were declared as one product
                // param or (post-flatten, for constructors) as N params.
                let ctor_arity = components.len().saturating_sub(1);
                for param_name in &components {
                    methods
                        .entry((param_name.clone(), func.name.name.clone()))
                        .or_insert(MethodSig {
                            arity: func.params.len(),
                            return_ty: return_ty.clone(),
                            result_ok_ty: result_ok_ty.clone(),
                        });
                    if func.name.name == "Self" {
                        // e.g. "str".JsonValue() (arity 0) or Port(...).HttpServer(state) (arity 1)
                        methods
                            .entry((param_name.clone(), recv.name.clone()))
                            .or_insert(MethodSig {
                                arity: ctor_arity,
                                return_ty: return_ty.clone(),
                                result_ok_ty: result_ok_ty.clone(),
                            });
                    }
                }
            }
        }
    }

    // Duplicate-definition guard. Two function bodies that collide on
    // (receiver, name, first-input-component) would land on the same
    // dispatch slot in codegen — historically that surfaced as an
    // internal invalid-wasm error ("inconsistent lengths") when a user
    // name collided with a transitively-loaded stdlib declaration. It
    // is a checked error now. Constructor *families* are the legal form
    // this key deliberately carves out: same type, several `Self`
    // members, each with a distinct first input component (selection is
    // by argument type, so colliding first components would make the
    // call site ambiguous).
    let mut seen_defs: HashSet<(Option<String>, String, Option<String>)> = HashSet::new();
    for item in &module.items {
        if let Item::Function(func) = item {
            if func.name.name == "main" && func.receiver.is_none() {
                continue;
            }
            let first_component = func.params.first().and_then(|p| match &p.ty {
                TypeExpr::Named { name, .. } => Some(name.clone()),
                TypeExpr::Product { fields, .. } => fields
                    .first()
                    .and_then(|f| f.simple_name().map(|s| s.to_string())),
                _ => None,
            });
            let key = (
                func.receiver.as_ref().map(|r| r.name.clone()),
                func.name.name.clone(),
                first_component,
            );
            if !seen_defs.insert(key.clone()) {
                let (recv, fname, comp) = key;
                let message = if fname == "Self" {
                    let ty = recv.unwrap_or_default();
                    match comp {
                        Some(c) => format!(
                            "duplicate constructor: `{}` already has a constructor whose first input is `{}`",
                            ty, c
                        ),
                        None => format!(
                            "duplicate constructor: `{}` already has a zero-argument constructor",
                            ty
                        ),
                    }
                } else {
                    match recv {
                        Some(r) => format!("duplicate function `{}` on `{}`", fname, r),
                        None => format!("duplicate function `{}`", fname),
                    }
                };
                errors.push(CanonError::CheckError {
                    message,
                    span: func.name.span,
                });
            }
        }
    }

    let mut standalone_types: HashSet<String> = HashSet::new();
    for item in &module.items {
        if let Item::TypeDef(td) = item {
            standalone_types.insert(td.name.name.clone());
        }
    }

    // Type aliases: `A = B` or `A = B<...>` where the right-hand side is a
    // single named type. The checker uses these to resolve method lookups
    // and dispatch-arm pattern matches through alias chains, so methods
    // and variants declared on the base type apply to the alias too.
    //
    // Generic arguments on the right-hand side are stripped at the alias
    // level — `MessageContent = Option<Content>` is recorded as
    // `MessageContent -> Option`. This makes a dispatch on a
    // `MessageContent` value match patterns like `None` / `Some<Content>`
    // (which live under `variant_of["None"] == "Option"`) by walking
    // through the alias.
    let mut aliases: HashMap<String, String> = HashMap::new();
    for item in &module.items {
        if let Item::TypeDef(td) = item {
            if let TypeExpr::Named { name, .. } = &td.body {
                aliases.insert(td.name.name.clone(), name.clone());
            }
        }
    }

    // JSON prelude: a JSON literal types as `Json` even when the module
    // never mentions `canon/std/Json` (the loader auto-injects the stdlib
    // module only when interpolation / the `Json` validator / `Encoded` is
    // actually used — a fully static literal is a plain constant). For the
    // static case the checker still needs `Json` to be a known type whose
    // alias chain reaches `String`, so `{"k":"v"}.print()` and a `-> Json`
    // annotation work import-free. A user- or stdlib-defined `Json` wins.
    if !types.contains("Json") && !generic_types.contains("Json") {
        types.insert("Json".to_string());
        aliases.insert("Json".to_string(), "String".to_string());
    }

    // Same for `Html`: a fully static HTML literal (`<div>hi</div>` —
    // a compile-time constant) is typed `Html` without the stdlib's
    // `web/html.can` in scope, so the checker needs the alias chain to
    // reach `String` intrinsically. A user- or stdlib-defined `Html`
    // wins (the stdlib's is the same `Html = String`).
    if !types.contains("Html") && !generic_types.contains("Html") {
        types.insert("Html".to_string());
        aliases.insert("Html".to_string(), "String".to_string());
    }

    // Free functions: every `FunctionDef` with no receiver and a name
    // distinct from `main` and `Self`. These can be invoked with constructor
    // syntax (`Foo()`, `bar(x)`); the checker accepts them in that role and
    // the codegen routes the call through `func_table` lookups.
    let mut free_funcs: HashMap<String, MethodSig> = HashMap::new();
    for item in &module.items {
        if let Item::Function(func) = item {
            if func.receiver.is_some() {
                continue;
            }
            if func.name.name == "main" || func.name.name == "Self" {
                continue;
            }
            let (return_ty, result_ok_ty) = method_return_summary(&func.return_ty);
            free_funcs.insert(
                func.name.name.clone(),
                MethodSig {
                    arity: func.params.len(),
                    return_ty,
                    result_ok_ty,
                },
            );
        }
    }

    SymbolTable {
        types,
        generic_types,
        variant_of,
        methods,
        product_fields,
        standalone_types,
        free_funcs,
        aliases,
    }
}

fn check_self_constructor_signature(
    func: &FunctionDef,
    receiver_name: &str,
    errors: &mut Vec<CanonError>,
) {
    // Collect this constructor's generic param names so we can accept
    // return types like `HttpServer<S>` when S is a declared generic.
    let generic_names: std::collections::HashSet<&str> = func
        .generic_params
        .iter()
        .map(|g| g.name.name.as_str())
        .collect();

    let is_self_ty = |name: &str, generics: &[TypeExpr]| {
        name == receiver_name
            && (generics.is_empty()
                || generics.iter().all(|g| {
                    matches!(g, TypeExpr::Named { name, generics: inner, .. }
                        if generic_names.contains(name.as_str()) && inner.is_empty())
                }))
    };

    let valid = match &func.return_ty {
        TypeExpr::Named { name, generics, .. } => {
            if is_self_ty(name, generics) {
                true
            } else if (name == "Result" || name == "Option") && !generics.is_empty() {
                matches!(
                    &generics[0],
                    TypeExpr::Named { name, generics, .. } if is_self_ty(name, generics)
                )
            } else {
                false
            }
        }
        _ => false,
    };
    if !valid {
        errors.push(CanonError::CheckError {
            message: format!(
                "constructor `{}` must return `{}`, `Result<{}, E>`, or `Option<{}>`",
                receiver_name, receiver_name, receiver_name, receiver_name
            ),
            span: func.return_ty.span(),
        });
    }

    check_endomorphism_input(func, receiver_name, errors);
}

/// An arrow may not construct a type that is also one of its inputs. An
/// endomorphism (`Map * String => Map`) is the one operation whose types
/// cannot identify it — insert, remove, and update all share that
/// signature — so the operation takes a **result newtype** (`Inserted =
/// Map`, `Removed = Map`): the name relocates into a type the compiler
/// checks, sorts, and resolves. Exact-name comparison only: an input
/// that is a *newtype* of the constructed type (`Rest = Map` flowing
/// into a `Map` constructor) is a different type and carries its own
/// information. Binding files are exempt — WIT shapes its signatures.
fn check_endomorphism_input(func: &FunctionDef, constructed: &str, errors: &mut Vec<CanonError>) {
    if func.extern_wasm.is_some() {
        return;
    }
    for param in &func.params {
        if let TypeExpr::Named { name, generics, .. } = &param.ty {
            if name == constructed && generics.is_empty() {
                errors.push(CanonError::CheckError {
                    message: format!(
                        "an arrow that returns its own input type needs a name the types can't \
                         supply: mint a result newtype (`X = {constructed}`) and construct that \
                         (`… => X`) instead of `{constructed}` itself",
                    ),
                    span: param.span,
                });
            }
        }
    }
}

/// `Json("…")` / `Html("…")` fed a **static string literal** that the
/// corresponding literal form can already express is ceremony around a
/// literal — the parse can never fail, so the validating-constructor
/// spelling is a second way of writing the literal, and the language
/// keeps one. The check fires only when the string's content, pasted
/// into source, parses as a single all-static JSON/HTML literal;
/// runtime strings (the actual parsing use case), scalar documents
/// (`Json("\"text\"")` — no literal form), and malformed input all pass
/// through untouched.
fn check_literal_form_ceremony(name: &str, args: &[Expr], errors: &mut Vec<CanonError>) {
    if name != "Json" && name != "Html" {
        return;
    }
    let [Expr::StringLit { value, span }] = args else {
        return;
    };
    let probe = format!("Unit => Probe {{\n    {}\n}}\n", value);
    let Ok(tokens) = crate::lexer::Scanner::new(&probe).scan_tokens() else {
        return;
    };
    let Ok(module) = crate::parser::Parser::new(tokens).parse() else {
        return;
    };
    let [Item::Function(f)] = &module.items[..] else {
        return;
    };
    let [expr] = &f.body.exprs[..] else {
        return;
    };
    let expressible = match (name, expr) {
        ("Json", Expr::JsonLit { parts, .. }) => {
            parts.iter().all(|p| matches!(p, JsonLitPart::Static(_)))
        }
        ("Html", Expr::HtmlLit { parts, .. }) => {
            parts.iter().all(|p| matches!(p, HtmlLitPart::Static(_)))
        }
        _ => false,
    };
    if expressible {
        errors.push(CanonError::CheckError {
            message: format!(
                "`{name}(\"…\")` wraps a document the {name} literal already expresses: \
                 write the literal directly ({example}) — the validating constructor is \
                 for strings built at runtime",
                name = name,
                example = if name == "Json" {
                    "`{\"k\":v}` / `[v]`"
                } else {
                    "`<tag>…</tag>`"
                },
            ),
            span: *span,
        });
    }
}

fn check_type_def(td: &TypeDef, symbols: &SymbolTable, errors: &mut Vec<CanonError>) {
    // Types are PascalCase. A camelCase type alias only means something
    // in a binding file, where `apply_bindings` has already rewritten it
    // into an extern function before the checker runs.
    if starts_lowercase(&td.name.name) {
        errors.push(CanonError::CheckError {
            message: format!(
                "camelCase names are not allowed: types are PascalCase — rename `{}`",
                td.name.name
            ),
            span: td.name.span,
        });
    }
    // A body-less shape declaration opens a second spelling of a
    // constructor family with none of a shape's justifications
    // implemented yet (generic constraints, bare-type-parameter returns,
    // default bodies). Even the literal-interpolation hooks are ordinary
    // result-newtype families now (`Encoded = Json`, `Escaped = Html`),
    // so no shape survives. See the spec (functions.md § Shape or
    // Result Newtype).
    if matches!(td.body, TypeExpr::Function { .. }) {
        errors.push(CanonError::CheckError {
            message: format!(
                "`{name}` declares a shape, and operations take result newtypes: replace it \
                 with `{name} = <ReturnType>` and anonymous arrows (`(Receiver) => {name}`) — \
                 shapes return when generic constraints land",
                name = td.name.name
            ),
            span: td.name.span,
        });
    }
    let mut generic_scope: HashSet<String> = td
        .generic_params
        .iter()
        .map(|g| g.name.name.clone())
        .collect();
    for param in &td.generic_params {
        if let Some(bound) = &param.bound {
            check_type_expr(bound, symbols, &generic_scope, errors);
        }
    }
    check_type_expr(&td.body, symbols, &generic_scope, errors);
    let _ = &mut generic_scope;
}

fn check_function(
    func: &FunctionDef,
    symbols: &SymbolTable,
    errors: &mut Vec<CanonError>,
    main_found: &mut bool,
) {
    if func.name.name == "main" {
        if *main_found {
            errors.push(CanonError::CheckError {
                message: "duplicate entry point: only one CLI entry (`Args => Exit { … }`) \
                          may be defined"
                    .to_string(),
                span: func.span,
            });
        }
        *main_found = true;

        if func.receiver.is_some() {
            errors.push(CanonError::CheckError {
                message: "the entry point must not have a receiver".to_string(),
                span: func.span,
            });
        }

        // Entries are anonymous, selected by their world-shaped return
        // (`Args => Exit`). A literal `main` name is a leftover of
        // the pre-types-only surface. Anonymous entries reach here
        // already renamed to the internal `main` by
        // `resolve_new_syntax`, distinguished by the `anonymous` flag.
        if !func.anonymous {
            errors.push(CanonError::CheckError {
                message: "`main` is not a name: entries are anonymous and selected by their \
                          world-shaped return — write `Args => Exit { … }`"
                    .to_string(),
                span: func.name.span,
            });
        }
    } else if func.extern_wasm.is_none() && starts_lowercase(&func.name.name) {
        // Types-only: the only names are type names (PascalCase).
        // camelCase survives in exactly one place — binding files (the
        // FFI boundary, `extern_wasm` above).
        errors.push(CanonError::CheckError {
            message: format!(
                "camelCase names are not allowed: the only names are type names — \
                 replace `{}` with a PascalCase constructor (`Input => Type {{ … }}`)",
                func.name.name
            ),
            span: func.name.span,
        });
    } else if func.extern_wasm.is_none() && func.receiver.is_none() && !func.anonymous {
        // The unified declaration rule (the language spec, § Types-Only
        // Canon): a bodied declaration is named after the type it
        // constructs — the name is checkable from the signature alone.
        // Constructors normalised to `Self` and anonymous arrows satisfy
        // it by construction; what reaches here is a free named function,
        // where the rule bites: the name must be the constructed return
        // type (modulo `Result`/`Option`/`Future` peeling and newtype
        // chains).
        if let Some(constructed) = constructed_type_name(&func.return_ty) {
            let is_generic_param = func
                .generic_params
                .iter()
                .any(|g| g.name.name == constructed);
            if !is_generic_param
                && func.name.name != constructed
                && symbols.resolve_alias(&func.name.name) != symbols.resolve_alias(&constructed)
            {
                errors.push(CanonError::CheckError {
                    message: format!(
                        "a bodied declaration is named after the type it constructs: \
                         `{}` returns `{}`",
                        func.name.name, constructed
                    ),
                    span: func.name.span,
                });
            }
        }
    } else if func.extern_wasm.is_none()
        && func.name.name != "Self"
        && !func.anonymous
        && func.receiver.is_some()
    {
        // The same rule, receiver-carrying half. `resolve_new_syntax`
        // turns any named bodied declaration whose name is not a type in
        // its file into a method on its first component — which would
        // otherwise let an arbitrary verb wear PascalCase (`Frobnicated =
        // (Int) => Int` is not a Frobnicated constructor; the name lies).
        // The name must construct the type it names (modulo
        // `Result`/`Option`/`Future` peeling and newtype chains) — the
        // cross-file result-newtype case, where the newtype's TypeDef
        // lives in another loaded file.
        let constructs_name = constructed_type_name(&func.return_ty)
            .map(|constructed| {
                func.name.name == constructed
                    || symbols.resolve_alias(&func.name.name) == symbols.resolve_alias(&constructed)
            })
            .unwrap_or(false);
        if !constructs_name {
            let constructed =
                constructed_type_name(&func.return_ty).unwrap_or_else(|| "…".to_string());
            errors.push(CanonError::CheckError {
                message: format!(
                    "`{name}` is not the type this declaration constructs: mint a result \
                     newtype (`{name} = {constructed}`) and construct it with an anonymous \
                     arrow — a name carries no information the types don't",
                    name = func.name.name,
                    constructed = constructed,
                ),
                span: func.name.span,
            });
        }
    }

    let generic_scope: HashSet<String> = func
        .generic_params
        .iter()
        .map(|g| g.name.name.clone())
        .collect();

    for param in &func.generic_params {
        if let Some(bound) = &param.bound {
            check_type_expr(bound, symbols, &generic_scope, errors);
        }
    }
    check_type_expr(&func.return_ty, symbols, &generic_scope, errors);
    for param in &func.params {
        check_type_expr(&param.ty, symbols, &generic_scope, errors);
    }

    if let Some(recv) = &func.receiver {
        if !symbols.knows_type(&recv.name) && !generic_scope.contains(&recv.name) {
            errors.push(CanonError::CheckError {
                message: format!("unknown receiver type `{}`", recv.name),
                span: recv.span,
            });
        }
        if func.name.name == "Self" {
            check_self_constructor_signature(func, &recv.name, errors);
        }
    } else if func.name.name != "main" {
        // A receiver-less constructor (the constructed type's TypeDef
        // lives in another loaded file, so `resolve_new_syntax` left it
        // free) gets the same endomorphism check as a `Self` constructor:
        // the constructed identity is the function's own name.
        check_endomorphism_input(func, &func.name.name, errors);
    }

    if func.extern_wasm.is_some() {
        return;
    }

    let scope = ExprScope::from_function(func);
    check_block(&func.body, &func.return_ty, &scope, symbols, errors);
}

fn starts_lowercase(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_lowercase)
}

/// Union-dispatch shape rules (the ordering spec, "Dispatch arms follow
/// the union's variant order", and "every variant must be handled;
/// there is no wildcard arm"):
///
///   * arms appear in the union's variant order — alphabetical, since
///     union variants themselves are enforced alphabetical;
///   * no variant appears twice;
///   * every variant of the scrutinee's union is covered.
///
/// Skipped when the scrutinee's union can't be resolved (unknown
/// scrutinee, or an arm already failed membership — those cases produce
/// their own errors) so one mistake doesn't cascade.
fn check_union_dispatch_shape(
    arms: &[MatchArm],
    scrutinee_ty: &str,
    symbols: &SymbolTable,
    errors: &mut Vec<CanonError>,
) {
    if scrutinee_ty.is_empty() || scrutinee_ty == "<unknown>" || arms.is_empty() {
        return;
    }
    // Resolve the scrutinee to a union by walking the alias chain
    // (`MessageContent = Option<Content>` dispatches on `Option`'s
    // variants). The bound keeps a malformed alias cycle finite.
    let is_union = |name: &str| symbols.variant_of.values().any(|u| u == name);
    let mut union_name = scrutinee_ty;
    for _ in 0..20 {
        if is_union(union_name) {
            break;
        }
        match symbols.aliases.get(union_name) {
            Some(next) => union_name = next.as_str(),
            None => return,
        }
    }
    if !is_union(union_name) {
        return;
    }

    let arm_names: Vec<(&str, crate::error::Span)> = arms
        .iter()
        .filter_map(|a| match &a.param_ty {
            TypeExpr::Named { name, span, .. } => Some((name.as_str(), *span)),
            _ => None,
        })
        .collect();

    // Every arm must be a variant of this union for the shape rules to
    // apply cleanly — membership failures already errored above.
    if !arm_names
        .iter()
        .all(|(n, _)| symbols.variant_of.get(*n).is_some_and(|u| u == union_name))
    {
        return;
    }

    for pair in arm_names.windows(2) {
        let ((a, _), (b, bspan)) = (pair[0], pair[1]);
        if a > b {
            errors.push(CanonError::CheckError {
                message: format!(
                    "dispatch arms must follow the union's variant order: `{}` before `{}`",
                    b, a
                ),
                span: bspan,
            });
        }
        if a == b {
            errors.push(CanonError::CheckError {
                message: format!("duplicate dispatch arm `{}`", a),
                span: bspan,
            });
        }
    }

    let covered: HashSet<&str> = arm_names.iter().map(|(n, _)| *n).collect();
    let mut missing: Vec<&str> = symbols
        .variant_of
        .iter()
        .filter(|(_, u)| u.as_str() == union_name)
        .map(|(v, _)| v.as_str())
        .filter(|v| !covered.contains(v))
        .collect();
    missing.sort_unstable();
    if !missing.is_empty() {
        errors.push(CanonError::CheckError {
            message: format!(
                "non-exhaustive dispatch on `{}`: every variant must be handled (there is \
                 no wildcard arm) — missing `{}`",
                union_name,
                missing.join("`, `")
            ),
            span: arm_names.first().map(|(_, s)| *s).unwrap_or_default(),
        });
    }
}

/// The type a declaration constructs: its return type with
/// `Result`/`Option`/`Future` peeled (the language spec, § Anonymous
/// Constructors). `None` when the return isn't a named type (a product,
/// a function type) — the naming rule doesn't apply there.
fn constructed_type_name(ty: &TypeExpr) -> Option<String> {
    match ty {
        TypeExpr::Named { name, generics, .. } => {
            if matches!(name.as_str(), "Result" | "Option" | "Future") && !generics.is_empty() {
                constructed_type_name(&generics[0])
            } else {
                Some(name.clone())
            }
        }
        _ => None,
    }
}

fn check_type_expr(
    ty: &TypeExpr,
    symbols: &SymbolTable,
    generic_scope: &HashSet<String>,
    errors: &mut Vec<CanonError>,
) {
    match ty {
        TypeExpr::Named {
            name,
            generics,
            span,
        } => {
            if name == "Self" {
                // allowed in method bodies / trait declarations; not validated here
            } else if name.starts_with("__extern__") {
                // extern type alias body — the Rust path isn't a Canon type
            } else if generic_scope.contains(name) {
                if !generics.is_empty() {
                    errors.push(CanonError::CheckError {
                        message: format!(
                            "type parameter `{}` cannot be applied to type arguments",
                            name
                        ),
                        span: *span,
                    });
                }
            } else if !symbols.knows_type(name) {
                errors.push(CanonError::CheckError {
                    message: format!("unknown type `{}`", name),
                    span: *span,
                });
            }
            for g in generics {
                check_type_expr(g, symbols, generic_scope, errors);
            }
        }
        TypeExpr::Union { variants, .. } => {
            let names: Vec<(&str, crate::error::Span)> = variants
                .iter()
                .filter_map(|v| {
                    if let TypeExpr::Named { name, span, .. } = v {
                        Some((name.as_str(), *span))
                    } else {
                        None
                    }
                })
                .collect();
            check_sorted_named("union variant", &names, errors);
            for v in variants {
                check_type_expr(v, symbols, generic_scope, errors);
            }
        }
        TypeExpr::Product { fields, .. } => {
            let names: Vec<(&str, crate::error::Span)> = fields
                .iter()
                .filter_map(|f| {
                    if let TypeExpr::Named { name, span, .. } = f {
                        Some((name.as_str(), *span))
                    } else {
                        None
                    }
                })
                .collect();
            check_sorted_named("product field", &names, errors);
            for f in fields {
                check_type_expr(f, symbols, generic_scope, errors);
            }
        }
        TypeExpr::Repeat { ty, .. } | TypeExpr::Spread { ty, .. } => {
            check_type_expr(ty, symbols, generic_scope, errors);
        }
        TypeExpr::Function {
            generic_params,
            params,
            return_ty,
            ..
        } => {
            let mut scope = generic_scope.clone();
            for gp in generic_params {
                scope.insert(gp.name.name.clone());
                if let Some(bound) = &gp.bound {
                    check_type_expr(bound, symbols, &scope, errors);
                }
            }
            for p in params {
                check_type_expr(p, symbols, &scope, errors);
            }
            check_type_expr(return_ty, symbols, &scope, errors);
        }
    }
}

struct ExprScope {
    names: Vec<String>,
}

impl ExprScope {
    fn from_function(func: &FunctionDef) -> Self {
        let mut names: Vec<String> = Vec::new();
        for p in &func.params {
            push_param_names(&p.ty, &mut names);
        }
        if let Some(recv) = &func.receiver {
            names.push(recv.name.clone());
        }
        Self { names }
    }

    fn contains(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }
}

fn push_param_names(ty: &TypeExpr, names: &mut Vec<String>) {
    match ty {
        TypeExpr::Named { name, generics, .. } if generics.is_empty() => {
            names.push(name.clone());
        }
        TypeExpr::Product { fields, .. } => {
            for f in fields {
                push_param_names(f, names);
            }
        }
        _ => {}
    }
}

fn check_block(
    block: &Block,
    return_ty: &TypeExpr,
    scope: &ExprScope,
    symbols: &SymbolTable,
    errors: &mut Vec<CanonError>,
) {
    if block.exprs.is_empty() {
        errors.push(CanonError::CheckError {
            message: "function body must contain at least one expression".to_string(),
            span: block.span,
        });
        return;
    }

    for expr in &block.exprs {
        check_expr(expr, scope, symbols, errors);
    }

    let last = block.exprs.last().unwrap();
    let last_ty = expr_type_name_in_scope(last, symbols);
    let return_ty_name = match return_ty {
        TypeExpr::Named { name, .. } => name.clone(),
        _ => "<complex>".to_string(),
    };
    // Newtype substitutability: the produced value satisfies the declared
    // return when either type's alias chain reaches the other — a body
    // producing `Html` satisfies `-> Button` where `Button = Html` (the
    // underlying flows into the alias slot), and a body producing
    // `Inserted` (`Inserted = Map`) satisfies `-> Map`.
    let alias_compatible = |from: &str, to: &str| -> bool {
        let mut cur = from.to_string();
        for _ in 0..20 {
            if cur == to {
                return true;
            }
            match symbols.aliases.get(&cur) {
                Some(next) => cur = next.clone(),
                None => return false,
            }
        }
        false
    };
    // `Unit` is zero-width and single-valued, so every type rooted at it
    // (`Program`, `Exited`, …) holds the same one value — they are freely
    // interchangeable in a return position. A `Unit => Program` entry
    // whose body ends in `Exited` (`= Unit`) is fine; so is any effect
    // marker flowing into another. (Data-carrying newtypes — `String`- or
    // `Int`-rooted — stay strict: only a reachable alias chain compatible.)
    let both_unit_rooted =
        alias_compatible(&last_ty, "Unit") && alias_compatible(&return_ty_name, "Unit");
    if last_ty != return_ty_name
        && last_ty != "<unknown>"
        && !both_unit_rooted
        && !alias_compatible(&last_ty, &return_ty_name)
        && !alias_compatible(&return_ty_name, &last_ty)
    {
        errors.push(CanonError::CheckError {
            message: format!(
                "function returns `{}` but last expression has type `{}`",
                return_ty_name, last_ty
            ),
            span: last.span(),
        });
    }
}

/// Validate a literal-pattern dispatch: `scrutinee.( * ("a") -> R {…}
/// * ("b") -> R {…} * (String) -> R {…} )`.
///
/// Rules (see the language spec § Literal Dispatch):
///   * The scrutinee must be `String` or `Int` (directly or through a
///     newtype alias chain), matching the literal kind of every arm.
///   * The final arm is a mandatory catch-all naming the scrutinee's
///     type — literal arms can never be exhaustive, so totality comes
///     from the catch-all.
///   * Literal arms follow canonical order (alphabetical for strings,
///     ascending for ints) with no duplicates — the same
///     "one canonical spelling" rule as everywhere else.
fn check_literal_dispatch(
    arms: &[MatchArm],
    scrutinee_ty: &str,
    scope: &ExprScope,
    symbols: &SymbolTable,
    errors: &mut Vec<CanonError>,
    span: crate::error::Span,
) {
    let generic_scope: HashSet<String> = HashSet::new();
    let base = symbols.resolve_alias(scrutinee_ty);
    let scrutinee_known = !scrutinee_ty.is_empty() && scrutinee_ty != "<unknown>";
    if scrutinee_known && base != "String" && base != "Int" {
        errors.push(CanonError::CheckError {
            message: format!(
                "literal dispatch requires a `String` or `Int` scrutinee: `{}` is neither",
                scrutinee_ty
            ),
            span,
        });
    }
    if !arms.iter().any(|a| a.literal.is_none()) {
        errors.push(CanonError::CheckError {
            message: format!(
                "literal dispatch must end with a catch-all arm `({})`: \
                 literal arms can never be exhaustive",
                if scrutinee_known {
                    scrutinee_ty
                } else {
                    "String"
                }
            ),
            span,
        });
    }
    for (i, arm) in arms.iter().enumerate() {
        check_type_expr(&arm.return_ty, symbols, &generic_scope, errors);
        match &arm.literal {
            Some(ArmLiteral::Str(_)) if scrutinee_known && base == "Int" => {
                errors.push(CanonError::CheckError {
                    message: "string literal arm on an `Int` scrutinee".to_string(),
                    span: arm.span,
                });
            }
            Some(ArmLiteral::Int(_)) if scrutinee_known && base == "String" => {
                errors.push(CanonError::CheckError {
                    message: "integer literal arm on a `String` scrutinee".to_string(),
                    span: arm.span,
                });
            }
            Some(_) => {}
            None => {
                if i != arms.len() - 1 {
                    errors.push(CanonError::CheckError {
                        message: "the catch-all arm must be the last arm of a literal dispatch"
                            .to_string(),
                        span: arm.span,
                    });
                }
                check_type_expr(&arm.param_ty, symbols, &generic_scope, errors);
                if let TypeExpr::Named {
                    name, span: pspan, ..
                } = &arm.param_ty
                {
                    if scrutinee_known
                        && name != scrutinee_ty
                        && symbols.resolve_alias(name) != base
                    {
                        errors.push(CanonError::CheckError {
                            message: format!(
                                "catch-all arm `({})` does not match the scrutinee type `{}`",
                                name, scrutinee_ty
                            ),
                            span: *pspan,
                        });
                    }
                }
            }
        }
        // The scrutinee value is in scope inside every arm body, under
        // its own type name (and its primitive base, mirroring the
        // receiver alias-chain rule in `build_local_scope`).
        let mut inner_scope = ExprScope {
            names: scope.names.clone(),
        };
        if scrutinee_known {
            inner_scope.names.push(scrutinee_ty.to_string());
            if base != scrutinee_ty {
                inner_scope.names.push(base.to_string());
            }
        }
        for expr in &arm.body.exprs {
            check_expr(expr, &inner_scope, symbols, errors);
        }
    }
    // Canonical order + no duplicates among the literal arms. Because
    // order is enforced, a duplicate always ends up adjacent in valid
    // code, so the windowed comparison covers both rules.
    let lits: Vec<(&ArmLiteral, crate::error::Span)> = arms
        .iter()
        .filter_map(|a| a.literal.as_ref().map(|l| (l, a.span)))
        .collect();
    for w in lits.windows(2) {
        let (prev, _) = &w[0];
        let (next, nspan) = &w[1];
        match (prev, next) {
            (ArmLiteral::Str(a), ArmLiteral::Str(b)) => {
                if b == a {
                    errors.push(CanonError::CheckError {
                        message: format!("duplicate literal arm `\"{}\"` in dispatch", b),
                        span: *nspan,
                    });
                } else if b < a {
                    errors.push(CanonError::CheckError {
                        message: format!(
                            "literal dispatch arms must be in alphabetical order: \
                             `\"{}\"` should come before `\"{}\"`",
                            b, a
                        ),
                        span: *nspan,
                    });
                }
            }
            (ArmLiteral::Int(a), ArmLiteral::Int(b)) => {
                if b == a {
                    errors.push(CanonError::CheckError {
                        message: format!("duplicate literal arm `{}` in dispatch", b),
                        span: *nspan,
                    });
                } else if b < a {
                    errors.push(CanonError::CheckError {
                        message: format!(
                            "literal dispatch arms must be in ascending order: \
                             `{}` should come before `{}`",
                            b, a
                        ),
                        span: *nspan,
                    });
                }
            }
            // Mixed literal kinds: already reported against the
            // scrutinee kind above.
            _ => {}
        }
    }
}

fn check_expr(expr: &Expr, scope: &ExprScope, symbols: &SymbolTable, errors: &mut Vec<CanonError>) {
    match expr {
        Expr::Ident(ident) => {
            if is_capability_type(&ident.name) {
                if !scope.contains(&ident.name) {
                    errors.push(CanonError::CheckError {
                        message: format!(
                            "capability `{}` must be received as a parameter: capabilities cannot be conjured",
                            ident.name
                        ),
                        span: ident.span,
                    });
                }
            } else {
                let known = symbols.knows_type(&ident.name)
                    || symbols.variant_of.contains_key(&ident.name)
                    || scope.contains(&ident.name)
                    || ident.name == "Self";
                if !known {
                    errors.push(CanonError::CheckError {
                        message: format!("unknown name `{}`", ident.name),
                        span: ident.span,
                    });
                }
            }
        }
        Expr::StringLit { .. } => {}
        Expr::IntLit { .. } | Expr::FloatLit { .. } => {}
        Expr::JsonLit { .. } => {}
        Expr::HtmlLit { .. } => {}
        Expr::FormatLit { .. } => {}
        Expr::Constructor { name, args, span } => {
            let is_variant = symbols.variant_of.contains_key(&name.name);
            // A free function with this exact name (like `Now = () -> Now`
            // or `randomInt = () -> Int`) makes the "constructor" call legal
            // even when the name isn't a known type. Its presence is signalled
            // by an entry in `extern_funcs` with a matching arity.
            let matches_free_func = symbols
                .free_funcs
                .get(&name.name)
                .is_some_and(|sig| sig.arity == args.len());
            // The concurrency combinators are methods, not bare calls —
            // `a.parallel(b)`, not `parallel(a, b)`. Canon has no bare
            // free-function call form anywhere else; steer to the method
            // spelling instead of reporting an unknown type.
            if CONCURRENT_COMBINATORS.contains(&name.name.as_str()) && !matches_free_func {
                errors.push(CanonError::CheckError {
                    message: format!(
                        "`{0}(…)` is not a call form: combinators are methods on the first future: `a.{0}(b)`",
                        name.name
                    ),
                    span: name.span,
                });
                for arg in args {
                    check_expr(arg, scope, symbols, errors);
                }
                return;
            }
            if !symbols.knows_type(&name.name) && !is_variant && !matches_free_func {
                errors.push(CanonError::CheckError {
                    message: format!("unknown type `{}` in constructor", name.name),
                    span: name.span,
                });
            }
            check_literal_form_ceremony(&name.name, args, errors);
            if args.is_empty() && !is_variant && !matches_free_func {
                let is_zero_data_builtin = ZERO_DATA_BUILTINS.contains(&name.name.as_str());
                let has_zero_arg_ctor = symbols
                    .methods
                    .get(&(name.name.clone(), "Self".to_string()))
                    .is_some_and(|sig| sig.arity == 0);
                if !is_zero_data_builtin && !has_zero_arg_ctor {
                    errors.push(CanonError::CheckError {
                        message: format!(
                            "constructor `{}()` is not allowed: empty constructors are disallowed",
                            name.name
                        ),
                        span: *span,
                    });
                }
            }
            if let Some(field_types) = symbols.product_fields.get(&name.name).cloned() {
                check_product_construction_arity(
                    &name.name,
                    &field_types,
                    effective_call_arity(args),
                    *span,
                    errors,
                );
                let arg_refs: Vec<&Expr> = args.iter().collect();
                check_product_construction_types(
                    &name.name,
                    &field_types,
                    &arg_refs,
                    symbols,
                    *span,
                    errors,
                );
            }
            for arg in args {
                check_expr(arg, scope, symbols, errors);
            }
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
            ..
        } => {
            check_expr(receiver, scope, symbols, errors);
            for arg in args {
                check_expr(arg, scope, symbols, errors);
            }
            let recv_ty = expr_type_name_in_scope(receiver, symbols);
            // For types that are both a standalone typedef and a variant of a union,
            // try method lookup on the specific type first (e.g. JsonObject.get before
            // falling back to JsonValue.get).
            let recv_ty_specific: String = match receiver.as_ref() {
                Expr::Ident(ident)
                    if symbols.standalone_types.contains(&ident.name)
                        && symbols.variant_of.contains_key(&ident.name) =>
                {
                    ident.name.clone()
                }
                Expr::Constructor { name, .. }
                    if symbols.standalone_types.contains(&name.name)
                        && symbols.variant_of.contains_key(&name.name) =>
                {
                    name.name.clone()
                }
                _ => recv_ty.clone(),
            };
            let effective_arity = effective_call_arity(args);
            // A method lookup tries, in order: the specific receiver type,
            // the broader receiver type (when they differ), and the alias
            // chain of each. This lets methods declared on `String`/`Int`/…
            // be invoked on user aliases (`Path`, `Now`, `Url`, …) without
            // redeclaring them.
            // Concurrency combinators are compiler builtins invoked as
            // methods on the first future: `a.parallel(b)` / `a.race(b)`.
            // The receiver's static type is a `Future<T>` produced by an
            // async call, which ordinary method lookup can't see.
            let is_concurrent_combinator =
                CONCURRENT_COMBINATORS.contains(&method.name.as_str()) && effective_arity == 1;
            // Piped construction: `A -> B(rest)` is the same call as
            // `B(A * rest)`, so a method whose name is a type constructor
            // (a typedef, a union variant, or a primitive) is really
            // building a `B` — the receiver fills the first input slot.
            let is_piped_construction = symbols.standalone_types.contains(&method.name)
                || symbols.variant_of.contains_key(&method.name)
                || matches!(
                    method.name.as_str(),
                    "Int" | "Float" | "String" | "Bool" | "Some" | "None" | "Ok" | "Err"
                );
            let has_alias_method =
                method_known_via_aliases(&recv_ty_specific, &method.name, effective_arity, symbols)
                    || (recv_ty_specific != recv_ty
                        && method_known_via_aliases(
                            &recv_ty,
                            &method.name,
                            effective_arity,
                            symbols,
                        ));
            let known = is_concurrent_combinator || is_piped_construction || has_alias_method;
            if !known {
                errors.push(CanonError::CheckError {
                    message: format!(
                        "no method `{}` on type `{}` with {} argument(s)",
                        method.name, recv_ty, effective_arity
                    ),
                    span: *span,
                });
            } else if is_piped_construction {
                // The canonical call form pipes the first arg (`A -> B(rest)`
                // for `B(A * rest)`), so a multi-field product's construction
                // reaches the checker as a `MethodCall`, not an
                // `Expr::Constructor` — the receiver fills the first field
                // slot and `args` (itself flattened the same way a direct
                // constructor's args are) fills the rest.
                if let Some(field_types) = symbols.product_fields.get(&method.name) {
                    check_product_construction_arity(
                        &method.name,
                        field_types,
                        1 + effective_call_arity(args),
                        *span,
                        errors,
                    );
                }
            }
            if is_piped_construction && !has_alias_method {
                // A scalar newtype (`Greeting = String`) or a bare
                // primitive pipe (`x -> Int`) erases to its underlying
                // primitive at the value level: codegen leaves the
                // receiver's own compiled representation on the stack
                // unchanged (see `compile_method_call`'s scalar-erasure
                // fallback). If the receiver's static type resolves to a
                // *different* primitive and no declared conversion method
                // covers the pair (that's what `has_alias_method` already
                // checked), the emitted wasm has the wrong stack shape —
                // an `i64` where the callee expects a string's `i32`
                // pointer/length pair, for instance — and wasmtime rejects
                // it as invalid. Catch the mismatch here instead.
                if let (Some(target_scalar), Some(recv_scalar)) = (
                    scalar_primitive_root(symbols, &method.name),
                    scalar_primitive_root(symbols, &recv_ty),
                ) {
                    // The Int ↔ Float pair is the one cross-primitive
                    // conversion codegen owns (wasm numerics:
                    // `i64.trunc_f64_s` / `f64.convert_i64_s`), so
                    // `x -> Int` on a Float truncates rather than
                    // mismatching. Every other conversion is a stdlib
                    // constructor (`has_alias_method` above).
                    let is_numeric_conversion = matches!(
                        (target_scalar, recv_scalar),
                        ("Int", "Float") | ("Float", "Int")
                    );
                    if target_scalar != recv_scalar && !is_numeric_conversion {
                        errors.push(CanonError::CheckError {
                            message: format!(
                                "`{}` expects a `{}`, found `{}`",
                                method.name, target_scalar, recv_scalar
                            ),
                            span: *span,
                        });
                    }
                }
            }
            if is_piped_construction {
                // The canonical call form pipes the first arg (`A -> B(rest)`
                // for `B(A * rest)`), so a multi-field product's construction
                // reaches the checker as a `MethodCall`, not an
                // `Expr::Constructor` — validate it the same way, with the
                // receiver standing in for the first constructor arg.
                if let Some(field_types) = symbols.product_fields.get(&method.name).cloned() {
                    let mut ctor_args: Vec<&Expr> = vec![receiver.as_ref()];
                    match args.as_slice() {
                        [Expr::ProductValue { fields, .. }] => ctor_args.extend(fields.iter()),
                        _ => ctor_args.extend(args.iter()),
                    }
                    check_product_construction_types(
                        &method.name,
                        &field_types,
                        &ctor_args,
                        symbols,
                        *span,
                        errors,
                    );
                }
            }
        }
        Expr::Match {
            scrutinee,
            arms,
            span,
        } => {
            check_expr(scrutinee, scope, symbols, errors);
            let scrutinee_ty = expr_type_name_in_scope(scrutinee, symbols);
            let generic_scope: HashSet<String> = HashSet::new();
            // Literal-pattern dispatch (`* ("/notes") -> …` on a String
            // scrutinee, `* (404) -> …` on an Int) has its own rules —
            // mandatory trailing catch-all, canonical literal order,
            // no duplicates — and skips the union-variant machinery.
            if arms.iter().any(|a| a.literal.is_some()) {
                check_literal_dispatch(arms, &scrutinee_ty, scope, symbols, errors, *span);
                return;
            }
            for arm in arms {
                // Validate param_ty and return_ty as type expressions
                check_type_expr(&arm.param_ty, symbols, &generic_scope, errors);
                check_type_expr(&arm.return_ty, symbols, &generic_scope, errors);
                // Verify the variant belongs to the scrutinee's type.
                //
                // The scrutinee may be a newtype wrapper around a union
                // (e.g. `MessageContent = Option<Content>`). We walk the
                // alias chain so dispatching `MessageContent.(None, Some)`
                // matches `Option`'s variants. See `aliases` in
                // `collect_symbols` for how the chain is built.
                if let TypeExpr::Named {
                    name: variant_name,
                    span: vspan,
                    ..
                } = &arm.param_ty
                {
                    if !scrutinee_ty.is_empty() && scrutinee_ty != "<unknown>" {
                        if let Some(pattern_enum) = symbols.variant_of.get(variant_name.as_str()) {
                            let mut current = scrutinee_ty.as_str();
                            let mut matched = current == pattern_enum.as_str();
                            // Walk aliases until we hit the target or run
                            // out of links. The bound keeps a malformed
                            // alias cycle from spinning forever.
                            for _ in 0..20 {
                                if matched {
                                    break;
                                }
                                match symbols.aliases.get(current) {
                                    Some(next) => {
                                        current = next.as_str();
                                        if current == pattern_enum.as_str() {
                                            matched = true;
                                        }
                                    }
                                    None => break,
                                }
                            }
                            if !matched {
                                errors.push(CanonError::CheckError {
                                    message: format!(
                                        "pattern `{}` is not a variant of `{}`",
                                        variant_name, scrutinee_ty
                                    ),
                                    span: *vspan,
                                });
                            }
                        }
                    }
                }
                // Build inner scope: generic type args become accessible by their type name
                let mut inner_scope = ExprScope {
                    names: scope.names.clone(),
                };
                if let TypeExpr::Named {
                    name: variant_name,
                    generics,
                    ..
                } = &arm.param_ty
                {
                    for g in generics {
                        push_param_names(g, &mut inner_scope.names);
                    }
                    // If the variant itself has a TypeDef (e.g. Branch = Left * Right * Value),
                    // the matched value is accessible under the variant name.
                    if symbols.knows_type(variant_name)
                        && symbols.variant_of.contains_key(variant_name.as_str())
                    {
                        inner_scope.names.push(variant_name.clone());
                    }
                }
                for expr in &arm.body.exprs {
                    check_expr(expr, &inner_scope, symbols, errors);
                }
            }
            if arms.is_empty() {
                errors.push(CanonError::CheckError {
                    message: "dispatch expression must have at least one arm".to_string(),
                    span: *span,
                });
            }
            check_union_dispatch_shape(arms, &scrutinee_ty, symbols, errors);
        }
        Expr::Try { inner, .. } => {
            check_expr(inner, scope, symbols, errors);
        }
        Expr::ProductValue { fields, .. } => {
            for f in fields {
                check_expr(f, scope, symbols, errors);
            }
        }
        Expr::FieldAccess {
            receiver,
            field,
            span,
        } => {
            check_expr(receiver, scope, symbols, errors);
            let recv_ty = expr_type_name_in_scope(receiver, symbols);
            let recv_ty_for_lookup: String = match receiver.as_ref() {
                Expr::Ident(ident)
                    if symbols.standalone_types.contains(&ident.name)
                        && symbols.variant_of.contains_key(&ident.name) =>
                {
                    ident.name.clone()
                }
                _ => recv_ty.clone(),
            };
            if recv_ty == "<unknown>" {
                return;
            }
            // Case 1: product field access. A typedef registers fields via
            // two shapes:
            //   * `T = A * B * ...`  — each named component is a field.
            //   * `T = U`            — newtype with one field named `U`
            //     (see the language spec § "Newtypes Are 1-Component Products").
            let is_product = symbols.product_fields.contains_key(&recv_ty_for_lookup);
            if let Some(fields) = symbols.product_fields.get(&recv_ty_for_lookup) {
                if fields.iter().any(|f| f == &field.name) {
                    return; // valid product field
                }
                // Fall through to method checks before erroring — a name
                // like `print` on a newtype value isn't a field but is a
                // valid method-as-value reference via the alias chain.
            }
            // Case 2: first-class method reference (extern or Canon-defined)
            if symbols
                .methods
                .contains_key(&(recv_ty_for_lookup.clone(), field.name.clone()))
            {
                return; // valid method reference used as a value
            }
            // Case 3: zero-arg built-in method used without parens (e.g. "hello".print)
            if method_known_via_aliases(&recv_ty_for_lookup, &field.name, 0, symbols) {
                return;
            }
            // Neither a known field nor a known method. If the receiver is
            // a product (or newtype), be specific about it being a missing
            // field; otherwise use the generic message.
            if is_product {
                errors.push(CanonError::CheckError {
                    message: format!(
                        "type `{}` has no field `{}`",
                        recv_ty_for_lookup, field.name
                    ),
                    span: *span,
                });
            } else {
                errors.push(CanonError::CheckError {
                    message: format!(
                        "field access `.{}` on `{}`: not a product field and no method `{}` found",
                        field.name, recv_ty_for_lookup, field.name
                    ),
                    span: *span,
                });
            }
        }
        Expr::Lambda {
            params,
            return_ty,
            body,
            ..
        } => {
            let generic_scope: HashSet<String> = HashSet::new();
            check_type_expr(return_ty, symbols, &generic_scope, errors);
            for param in params {
                check_type_expr(&param.ty, symbols, &generic_scope, errors);
            }
            let mut inner_scope = ExprScope {
                names: scope.names.clone(),
            };
            for param in params {
                push_param_names(&param.ty, &mut inner_scope.names);
            }
            for expr in &body.exprs {
                check_expr(expr, &inner_scope, symbols, errors);
            }
        }
        // Await nodes are inserted by the checker itself and are
        // never produced by the parser. Nothing to validate structurally.
        Expr::Await { inner, .. } => {
            check_expr(inner, scope, symbols, errors);
        }
    }
}

/// Validates that a multi-field product construction supplies exactly one
/// argument per field. Codegen's `build_product_value` (`src/codegen/wasm/compile.rs`)
/// binds each supplied value to a field by type and falls back to binding
/// any unmatched values *positionally* — a floor meant for values codegen
/// can't infer a type for at all. With too few arguments a field is left
/// unbound; with too many, the excess value either gets bound over a field
/// codegen already filled (silently discarding the correct one) or is
/// dropped. Either way the emitted wasm's stack shape no longer matches the
/// product's component layout, and wasmtime rejects it as invalid — so
/// reject the mismatched arity here, at the checker, with a clean error.
fn check_product_construction_arity(
    type_name: &str,
    field_types: &[String],
    arg_count: usize,
    span: Span,
    errors: &mut Vec<CanonError>,
) {
    if field_types.len() < 2 || arg_count == field_types.len() {
        return;
    }
    errors.push(CanonError::CheckError {
        message: format!(
            "cannot construct `{type_name}`: expected {} argument(s) (`{}`), found {arg_count}",
            field_types.len(),
            field_types.join(" * ")
        ),
        span,
    });
}

/// When the lone arg is a value-level product, flatten it: `m(A * B)` has
/// arity 2, not 1. Otherwise the arity is just `args.len()`.
fn effective_call_arity(args: &[Expr]) -> usize {
    if args.len() == 1 {
        if let Expr::ProductValue { fields, .. } = &args[0] {
            return fields.len();
        }
    }
    args.len()
}

/// Summarises a function's declared return type into:
///   - the bare name used for type-checking comparisons (`"Result"`,
///     `"Unit"`, …), and
///   - the Ok-payload name when the return is a `Result<X, Y>` or
///     `Option<X>`, so `?` can give the extracted value its proper type.
///
/// `Future<T>` is transparent here: a method declared as returning
/// `Future<Result<X, Y>>` is summarised exactly like one declared
/// `Result<X, Y>`. The auto-await rule (`auto_await::transform`) inserts
/// the implicit `Expr::Await` at use sites, so the user-visible type after
/// awaiting is the inner type. Keeping the summary in lock-step means
/// `?` and arm-pattern inference work the same way regardless of whether
/// the method is sync or async.
///
/// `Stream<T>` is **not** peeled. A function returning `Stream<T>` is
/// producing a stream value that downstream combinators (`map`, `take`,
/// `concat`, …) operate on directly. Stream consumption (auto-iteration)
/// is handled at call sites via `.each` / `.next` recognition in
/// `async_analysis::expr_has_async_trigger`, not by type-peel here.
fn method_return_summary(ty: &TypeExpr) -> (String, Option<String>) {
    match ty {
        TypeExpr::Named { name, generics, .. } => {
            // Peel one layer of `Future<…>` only — the await is implicit so
            // the rest of the checker only ever sees the already-awaited
            // type. `Stream<…>` stays as-is (producer-side type).
            if name == "Future" && generics.len() == 1 {
                return method_return_summary(&generics[0]);
            }
            let bare = name.clone();
            let ok_ty = match name.as_str() {
                "Result" | "Option" => generics.first().and_then(|g| match g {
                    TypeExpr::Named { name, .. } => Some(name.clone()),
                    _ => None,
                }),
                _ => None,
            };
            (bare, ok_ty)
        }
        _ => ("<complex>".to_string(), None),
    }
}

/// The scalar primitive a type name erases to at the value level, walking
/// its one-level alias chain (`Greeting = String` → `"String"`). `None`
/// when the chain doesn't bottom out at one of the four scalar primitives
/// (products, unions, and generic containers all return `None`).
fn scalar_primitive_root<'a>(symbols: &'a SymbolTable, name: &'a str) -> Option<&'a str> {
    match symbols.resolve_alias(name) {
        s @ ("Int" | "Float" | "Bool" | "String") => Some(s),
        _ => None,
    }
}

/// Combined method lookup: tries the receiver type as given, then walks the
/// type-alias chain (`Path → String`, `Now → String`, …) up to the depth
/// cap, checking both `is_known_method` (built-ins) and `symbols.methods`
/// (user/extern declarations) at each step.
fn method_known_via_aliases(
    receiver_ty: &str,
    method: &str,
    arg_count: usize,
    symbols: &SymbolTable,
) -> bool {
    // A types-only vocabulary method (`Mapped`, `Joined`, …) also
    // matches a camelCase stdlib/binding function of its aliased name —
    // `stream -> Mapped(f)` binds the `map` FFI function on `Stream`.
    let alias = crate::ast::builtin_method_alias(method);
    let mut current = receiver_ty;
    let mut depth = 0;
    loop {
        // Newtype unwrap projection reaches any ancestor type in the alias
        // chain: `Cleared = Todos = String` makes `Cleared.String` (or the
        // piped `-> String`) a valid unwrap. Newtypes are 1-component
        // products, so the projection composes the whole way down.
        if arg_count == 0 && current == method {
            return true;
        }
        if is_known_method(current, method, arg_count) {
            return true;
        }
        if symbols
            .methods
            .get(&(current.to_string(), method.to_string()))
            .is_some_and(|m| m.arity == arg_count)
        {
            return true;
        }
        if let Some(canonical) = alias {
            if symbols
                .methods
                .get(&(current.to_string(), canonical.to_string()))
                .is_some_and(|m| m.arity == arg_count)
            {
                return true;
            }
        }
        if depth >= 20 {
            return false;
        }
        match symbols.aliases.get(current) {
            Some(next) => {
                current = next.as_str();
                depth += 1;
            }
            None => return false,
        }
    }
}

fn is_known_method(receiver_ty: &str, method: &str, arg_count: usize) -> bool {
    if receiver_ty == "<unknown>" || receiver_ty == "Self" {
        return true;
    }
    // Types-only vocabulary (`Print`/`Sum`/`Joined`/…) maps to the
    // camelCase builtin; only consulted here after user/stdlib lookup
    // missed, so a same-named function still wins.
    let method = crate::ast::builtin_method_alias(method).unwrap_or(method);
    // `print` is strictly zero-arg. The legacy capability-passing form
    // `.print(Stdout)` compiled to a silent no-op (the builtin only
    // fires on zero args), so accepting it here was a
    // checker-accepts-runs-wrong hole.
    if matches!(
        (receiver_ty, method, arg_count),
        ("String", "print", 0) | ("Int", "print", 0) | ("Float", "print", 0) | ("Bool", "print", 0)
    ) {
        return true;
    }
    // Only the base comparisons (`eq`/`lt`) are builtins; the derived
    // `ne`/`le`/`gt`/`ge` are stdlib constructor families
    // (`canon/std/{int,float,string}.can`) found via `symbols.methods`.
    if matches!(receiver_ty, "Int" | "Float")
        && matches!(method, "add" | "sub" | "mul" | "div" | "rem" | "eq" | "lt")
        && arg_count == 1
    {
        return true;
    }
    if receiver_ty == "String"
        && matches!(
            (method, arg_count),
            ("concat", 1)
                | ("length", 0)
                | ("byteAt", 1)
                | ("substring", 2)
                | ("eq", 1)
                | ("lt", 1)
        )
    {
        return true;
    }
    // `list.Json()` — conversion-is-construction spelling of "encode this
    // list of pre-rendered JSON values as a JSON array".
    if receiver_ty == "List"
        && matches!(
            (method, arg_count),
            ("length", 0)
                | ("first", 0)
                | ("get", 1)
                | ("map", 1)
                | ("append", 1)
                | ("concat", 1)
                | ("Json", 0)
        )
    {
        return true;
    }
    // NOTE: `Map` and `Set` have no builtin entries — they are pure
    // Canon (`canon/std/Map`, `canon/std/Set`: sorted recursive
    // unions), so their methods arrive through `symbols.methods` like
    // any other stdlib declaration once the module is imported.
    false
}

/// Every type `name` widens to, most specific first: itself, its
/// alias-unwrap chain, and — if it names a union variant — its parent
/// union's alias chain too. Mirrors codegen's `WasmGen::widening_chain`
/// (`src/codegen/wasm/compile.rs`), which drives the same by-type field
/// binding at construction time; kept in lockstep so the checker rejects
/// exactly the arg/field pairings codegen can't route.
fn product_widening_chain(symbols: &SymbolTable, name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = name.to_string();
    for _ in 0..20 {
        if !out.contains(&current) {
            out.push(current.clone());
        }
        match symbols.aliases.get(&current) {
            Some(next) => current = next.clone(),
            None => break,
        }
    }
    if let Some(parent) = symbols.variant_of.get(name) {
        let mut cur = parent.clone();
        for _ in 0..20 {
            if !out.contains(&cur) {
                out.push(cur.clone());
            }
            match symbols.aliases.get(&cur) {
                Some(next) => cur = next.clone(),
                None => break,
            }
        }
    }
    out
}

/// How well a value of type `value_ty` fits a field of type `field_ty`:
/// `2` exact (own type name, or a union member whose parent chain
/// reaches `field_ty`), `1` shared erasure base, `0` unrelated. Mirrors
/// codegen's `WasmGen::field_match_score`.
fn product_field_match_score(symbols: &SymbolTable, value_ty: &str, field_ty: &str) -> u8 {
    if value_ty == field_ty {
        return 2;
    }
    if let Some(parent) = symbols.variant_of.get(value_ty) {
        if product_widening_chain(symbols, parent)
            .iter()
            .any(|n| n == field_ty)
        {
            return 2;
        }
    }
    let value_chain = product_widening_chain(symbols, value_ty);
    let field_chain = product_widening_chain(symbols, field_ty);
    if value_chain.iter().any(|n| field_chain.contains(n)) {
        return 1;
    }
    0
}

/// Validates that a product construction's argument types admit the same
/// by-type field assignment codegen's `build_product_value` computes,
/// instead of silently falling through to its positional floor with an
/// incompatible type. That floor exists for values codegen can't infer a
/// type for at all — never for a value whose type is known and simply
/// wrong (e.g. two `Int`s constructing a `Birthday * Username` product,
/// where `Username` is `String`) — so an unfillable field here means
/// codegen would emit a value of the wrong wasm shape into that field's
/// slot, producing a component wasmtime rejects as invalid.
///
/// Skips validation whenever any argument's type can't be statically
/// named (`expr_type_name_in_scope` returns `"<unknown>"`) to avoid
/// false positives on expressions this analysis can't see through.
fn check_product_construction_types(
    type_name: &str,
    field_types: &[String],
    args: &[&Expr],
    symbols: &SymbolTable,
    span: Span,
    errors: &mut Vec<CanonError>,
) {
    if field_types.len() < 2 || args.len() != field_types.len() {
        return;
    }
    let arg_types: Vec<String> = args
        .iter()
        .map(|a| expr_type_name_in_scope(a, symbols))
        .collect();
    if arg_types.iter().any(|t| t == "<unknown>") {
        return;
    }
    let mut used = vec![false; arg_types.len()];
    let mut slot_val: Vec<Option<usize>> = vec![None; field_types.len()];
    for threshold in [2u8, 1u8] {
        for (si, field_ty) in field_types.iter().enumerate() {
            if slot_val[si].is_some() {
                continue;
            }
            if let Some(vi) = (0..arg_types.len()).find(|&vi| {
                !used[vi]
                    && product_field_match_score(symbols, &arg_types[vi], field_ty) == threshold
            }) {
                slot_val[si] = Some(vi);
                used[vi] = true;
            }
        }
    }
    for (si, field_ty) in field_types.iter().enumerate() {
        if slot_val[si].is_none() {
            errors.push(CanonError::CheckError {
                message: format!(
                    "cannot construct `{type_name}`: no argument's type is compatible with field `{field_ty}`"
                ),
                span,
            });
        }
    }
}

/// The static type name of an expression, or `"<unknown>"` when the
/// analysis can't see through it. Crate-visible for tooling: LSP
/// completion types the chain left of the cursor with the same rules
/// the checker applies at construction sites.
pub(crate) fn expr_type_name_in_scope(expr: &Expr, symbols: &SymbolTable) -> String {
    match expr {
        Expr::Ident(ident) => {
            if let Some(parent) = symbols.variant_of.get(&ident.name) {
                parent.clone()
            } else {
                ident.name.clone()
            }
        }
        Expr::StringLit { .. } => "String".to_string(),
        Expr::IntLit { .. } => "Int".to_string(),
        Expr::FloatLit { .. } => "Float".to_string(),
        Expr::Constructor { name, args, .. } => {
            // Variants widen to their parent union (e.g. `Some(x)` typed as
            // `Option`); free-function constructors take their declared
            // return type; everything else is treated as the type name
            // itself (newtype wrap).
            if let Some(parent) = symbols.variant_of.get(&name.name) {
                parent.clone()
            } else if let Some(t) = args
                .first()
                .map(|a| expr_type_name_in_scope(a, symbols))
                .filter(|t| t != &name.name)
            {
                // A constructor is identity when the argument already has
                // the target type (`Int(1)` is an `Int`, `Label("x")`
                // wraps) and a *declared conversion* otherwise
                // (conversion is construction, the language spec
                // § Conversions). A conversion is a self-named
                // constructor on the source type — `Int = (String) ->
                // Result<Int, MalformedInt>` registers as `("String",
                // "Int")` — so its declared return type wins for
                // non-matching arguments. This is what types each member
                // of a constructor *family* (`Json = (Bool) -> Json` vs
                // `Json = (String) -> Result<Json, MalformedJson>`) at
                // its call site. The lookup walks the argument's alias
                // chain so a newtyped argument still finds a member
                // declared on its underlying type. No declaration found
                // = plain newtype/product wrap, typed as the
                // constructor's name.
                let mut cur = t;
                let mut found = None;
                for _ in 0..20 {
                    if let Some(sig) = symbols.methods.get(&(cur.clone(), name.name.clone())) {
                        found = Some(sig.return_ty.clone());
                        break;
                    }
                    match symbols.aliases.get(&cur) {
                        Some(next) => cur = next.clone(),
                        None => break,
                    }
                }
                found.unwrap_or_else(|| {
                    symbols
                        .free_funcs
                        .get(&name.name)
                        .map(|sig| sig.return_ty.clone())
                        .unwrap_or_else(|| name.name.clone())
                })
            } else if let Some(sig) = symbols.free_funcs.get(&name.name) {
                sig.return_ty.clone()
            } else if let Some(sig) = symbols
                .methods
                .get(&(name.name.clone(), "Self".to_string()))
                .filter(|sig| sig.arity == 0)
            {
                // A zero-arg construction takes the nullary constructor's
                // declared return type — an accessor-style constructor may
                // construct into a wrapper (`Unit => Option<Cwd>`), and the
                // dispatch/`?` on `Cwd()` must see `Option<Cwd>`, not `Cwd`.
                sig.return_ty.clone()
            } else {
                name.name.clone()
            }
        }
        Expr::MethodCall {
            receiver, method, ..
        } => {
            if method.name == "parallel" || method.name == "Parallel" {
                // `a.parallel(b)` returns `Future<List<T>>`; the
                // auto-await collapses `Future<List<T>>` → `List<T>` at
                // any consuming site, so we report `List` here to keep
                // method-chain typing (`.Json()`, `.get(i)`, etc.)
                // flowing.
                return "List".to_string();
            }
            if method.name == "race" || method.name == "Race" {
                // `a.race(b) -> Future<T>` collapses to `T`. The receiver
                // is `Future<T>` where T is what the inner async call
                // returns; the auto-await transform peels the outer
                // Future in most call shapes, so the receiver's static
                // type name is the best available answer.
                return expr_type_name_in_scope(receiver, symbols);
            }
            let recv_ty = expr_type_name_in_scope(receiver, symbols);
            if let Some(sig) = symbols.methods.get(&(recv_ty.clone(), method.name.clone())) {
                return sig.return_ty.clone();
            }
            if let Some(canonical) = crate::ast::builtin_method_alias(&method.name) {
                if let Some(sig) = symbols
                    .methods
                    .get(&(recv_ty.clone(), canonical.to_string()))
                {
                    return sig.return_ty.clone();
                }
            }
            // Newtype unwrap projection: `cleared -> String` where
            // `Cleared = Todos = String` yields `String` — the method name
            // is an ancestor type reached along the receiver's alias chain.
            {
                let mut current = recv_ty.clone();
                for _ in 0..20 {
                    if current == method.name {
                        return method.name.clone();
                    }
                    match symbols.aliases.get(&current) {
                        Some(next) => current = next.clone(),
                        None => break,
                    }
                }
            }
            // Piped construction (`A -> B(rest)` builds a `B`, exactly as
            // `B(A * rest)` does): a variant widens to its parent union
            // (`x -> Some` is an `Option`); a primitive or any other type
            // constructor produces its own type (`5 -> Int`, `7 -> Left`).
            // Mirrors the `Expr::Constructor` arm. Names with a
            // shape/constructor body (`Served`, `Route`, `TestResult`) are
            // excluded — their declared return type is resolved by the
            // lookups above (or the builtin fallback), so a shape's result
            // is never masked by its own newtype declaration.
            if let Some(parent) = symbols.variant_of.get(&method.name) {
                return parent.clone();
            }
            let is_shape = symbols.methods.keys().any(|(_, m)| m == &method.name);
            if !is_shape
                && (symbols.standalone_types.contains(&method.name)
                    || matches!(method.name.as_str(), "Int" | "Float" | "String" | "Bool"))
            {
                return method.name.clone();
            }
            method_return_type(&recv_ty, &method.name)
        }
        Expr::Match { arms, .. } => arms
            .first()
            .map(|arm| match &arm.return_ty {
                TypeExpr::Named { name, .. } => name.clone(),
                _ => "<unknown>".to_string(),
            })
            .unwrap_or_else(|| "<unknown>".to_string()),
        Expr::Try { inner, .. } => {
            // `?` extracts the Ok/Some payload. We compute its type by
            // either inspecting the inner constructor directly (`Ok(x)?`)
            // or, for a method call returning `Result<X, _>` / `Option<X>`,
            // looking up the method's signature in the symbol table.
            match &**inner {
                Expr::Constructor { name, args, .. } => {
                    if matches!(name.name.as_str(), "Ok" | "Some") && !args.is_empty() {
                        return expr_type_name_in_scope(&args[0], symbols);
                    }
                    // Free-function constructor (`Now = () -> Now`):
                    if let Some(sig) = symbols.free_funcs.get(&name.name) {
                        if let Some(ok) = &sig.result_ok_ty {
                            return ok.clone();
                        }
                    }
                    // Method-style constructor invoked as `Name(arg)`
                    // (e.g. `Url("…")` dispatches to
                    // `Url = (String) -> Result<Url, InvalidUrl>`). The
                    // receiver type is inferred from `args[0]` and the
                    // function is registered in `methods[(recv, name)]`.
                    if let Some(arg) = args.first() {
                        let arg_ty = expr_type_name_in_scope(arg, symbols);
                        let mut current = arg_ty.as_str();
                        for _ in 0..20 {
                            if let Some(sig) = symbols
                                .methods
                                .get(&(current.to_string(), name.name.clone()))
                            {
                                if let Some(ok) = &sig.result_ok_ty {
                                    return ok.clone();
                                }
                                break;
                            }
                            match symbols.aliases.get(current) {
                                Some(next) => current = next.as_str(),
                                None => break,
                            }
                        }
                    }
                    "<unknown>".to_string()
                }
                Expr::MethodCall {
                    receiver, method, ..
                } => {
                    let recv_ty = expr_type_name_in_scope(receiver, symbols);
                    let mut current = recv_ty.as_str();
                    for _ in 0..20 {
                        if let Some(sig) = symbols
                            .methods
                            .get(&(current.to_string(), method.name.clone()))
                        {
                            if let Some(ok) = &sig.result_ok_ty {
                                return ok.clone();
                            }
                            break;
                        }
                        match symbols.aliases.get(current) {
                            Some(next) => current = next.as_str(),
                            None => break,
                        }
                    }
                    "<unknown>".to_string()
                }
                _ => "<unknown>".to_string(),
            }
        }
        Expr::Lambda { return_ty, .. } => match return_ty {
            TypeExpr::Named { name, .. } => name.clone(),
            _ => "<unknown>".to_string(),
        },
        Expr::ProductValue { .. } => "<unknown>".to_string(),
        Expr::FieldAccess {
            receiver, field, ..
        } => {
            // If this is a zero-arg builtin method used without parens, return
            // its return type rather than the field name. We walk the alias
            // chain so `.print` on `Path` / `Now` / etc. resolves through to
            // the underlying `String`'s `print`.
            let recv_ty = expr_type_name_in_scope(receiver, symbols);
            let mut current = recv_ty.as_str();
            for _ in 0..20 {
                let ret = method_return_type(current, &field.name);
                if ret != "<unknown>" {
                    return ret;
                }
                if let Some(sig) = symbols
                    .methods
                    .get(&(current.to_string(), field.name.clone()))
                {
                    return sig.return_ty.clone();
                }
                match symbols.aliases.get(current) {
                    Some(next) => current = next.as_str(),
                    None => break,
                }
            }
            field.name.clone()
        }
        // A JSON literal expression has type `Json` (declared in
        // `canon/std/json.can` as `Json = String`). The checker resolves
        // method dispatch on `Json` through the normal newtype-aliasing
        // path, so `.print` (on String), `.concat` (on String), and
        // `Json`-specific methods all line up.
        //
        // Requires `use canon/std/Json` to be in scope at the call site
        // — same shape as any other stdlib type. JSON literal syntax
        // is first-class, but its *type name* is part of the stdlib.
        Expr::JsonLit { .. } => "Json".to_string(),
        // HTML literals are `Html` (a `String` newtype) — like `Json`,
        // the type name is intrinsically known to the checker so a
        // fully static literal needs no import (see `collect_symbols`).
        Expr::HtmlLit { .. } => "Html".to_string(),
        // A backtick format string produces a plain `String`.
        Expr::FormatLit { .. } => "String".to_string(),
        Expr::Await { inner, .. } => expr_type_name_in_scope(inner, symbols),
    }
}

/// The builtin-method fallback table: what `receiver_ty -> method`
/// returns when no user/stdlib declaration claims the pair, or
/// `"<unknown>"` when the builtin doesn't exist on that receiver.
/// Crate-visible for tooling: LSP completion gates the builtin pipe
/// vocabulary (`Sum`, `Print`, `Joined`, …) on the same table.
pub(crate) fn method_return_type(receiver_ty: &str, method: &str) -> String {
    let method = crate::ast::builtin_method_alias(method).unwrap_or(method);
    match (receiver_ty, method) {
        ("String", "print") | ("Int", "print") | ("Float", "print") | ("Bool", "print") => {
            "Unit".to_string()
        }
        ("Int", "add" | "sub" | "mul" | "div" | "rem") => "Int".to_string(),
        ("Float", "add" | "sub" | "mul" | "div" | "rem") => "Float".to_string(),
        ("Int", "eq" | "lt") => "Bool".to_string(),
        ("Float", "eq" | "lt") => "Bool".to_string(),
        ("String", "concat" | "substring") => "String".to_string(),
        ("String", "length" | "byteAt") => "Int".to_string(),
        ("String", "eq" | "lt") => "Bool".to_string(),
        ("List", "length") => "Int".to_string(),
        ("List", "map") => "List".to_string(),
        ("List", "first") => "Option".to_string(),
        ("List", "get") => "Option".to_string(),
        ("List", "append" | "concat") => "List".to_string(),
        ("List", "Json") => "Json".to_string(),
        _ => "<unknown>".to_string(),
    }
}
