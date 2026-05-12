use rustc_ast::ast;
use rustc_lint::{EarlyContext, EarlyLintPass, LintContext};
use rustc_session::{declare_lint, impl_lint_pass};
use rustc_span::Span;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns `Some((index, prev_name, curr_name))` for the first pair of
/// adjacent names that are out of alphabetical order.
fn first_unsorted(names: &[String]) -> Option<(usize, String, String)> {
    names.windows(2).enumerate().find_map(|(i, w)| {
        if w[0] > w[1] {
            Some((i + 1, w[0].clone(), w[1].clone()))
        } else {
            None
        }
    })
}

/// Emit a lint diagnostic at the given span.
fn emit_lint(cx: &EarlyContext<'_>, lint: &'static rustc_lint::Lint, span: Span, msg: String) {
    cx.opt_span_lint(lint, Some(span), |diag| {
        diag.primary_message(msg);
    });
}

// ---------------------------------------------------------------------------
// 1. UNSORTED_STRUCT_FIELDS
// ---------------------------------------------------------------------------

declare_lint! {
    /// **Deny** — struct fields must be in alphabetical order.
    pub UNSORTED_STRUCT_FIELDS,
    Deny,
    "struct fields must be in alphabetical order"
}

pub struct UnsortedStructFields;
impl_lint_pass!(UnsortedStructFields => [UNSORTED_STRUCT_FIELDS]);

impl EarlyLintPass for UnsortedStructFields {
    fn check_item(&mut self, cx: &EarlyContext<'_>, item: &ast::Item) {
        // ItemKind::Struct is Struct(Ident, Generics, VariantData)
        if let ast::ItemKind::Struct(_, _, ref vdata) = item.kind {
            let fields = vdata.fields();
            let names: Vec<String> = fields
                .iter()
                .filter_map(|f| f.ident.map(|id| id.name.to_string()))
                .collect();
            // Only check structs where every field is named (skip tuple structs)
            if names.len() != fields.len() || names.len() < 2 {
                return;
            }
            if let Some((idx, prev, curr)) = first_unsorted(&names) {
                emit_lint(
                    cx,
                    UNSORTED_STRUCT_FIELDS,
                    fields[idx].span,
                    format!(
                        "struct field `{curr}` should come before `{prev}` (alphabetical order required)"
                    ),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 2. UNSORTED_ENUM_VARIANTS
// ---------------------------------------------------------------------------

declare_lint! {
    /// **Deny** — enum variants must be in alphabetical order.
    pub UNSORTED_ENUM_VARIANTS,
    Deny,
    "enum variants must be in alphabetical order"
}

pub struct UnsortedEnumVariants;
impl_lint_pass!(UnsortedEnumVariants => [UNSORTED_ENUM_VARIANTS]);

impl EarlyLintPass for UnsortedEnumVariants {
    fn check_item(&mut self, cx: &EarlyContext<'_>, item: &ast::Item) {
        // ItemKind::Enum is Enum(Ident, Generics, EnumDef)
        if let ast::ItemKind::Enum(_, _, ref enum_def) = item.kind {
            let names: Vec<String> = enum_def
                .variants
                .iter()
                .map(|v| v.ident.name.to_string())
                .collect();
            if names.len() < 2 {
                return;
            }
            if let Some((idx, prev, curr)) = first_unsorted(&names) {
                emit_lint(
                    cx,
                    UNSORTED_ENUM_VARIANTS,
                    enum_def.variants[idx].span,
                    format!(
                        "enum variant `{curr}` should come before `{prev}` (alphabetical order required)"
                    ),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 3. UNSORTED_MATCH_ARMS
// ---------------------------------------------------------------------------

declare_lint! {
    /// **Deny** — match arms must be sorted by pattern text. Wildcard `_` must
    /// always be last.
    pub UNSORTED_MATCH_ARMS,
    Deny,
    "match arms must be sorted by pattern text; wildcard `_` must be last"
}

pub struct UnsortedMatchArms;
impl_lint_pass!(UnsortedMatchArms => [UNSORTED_MATCH_ARMS]);

impl EarlyLintPass for UnsortedMatchArms {
    fn check_expr(&mut self, cx: &EarlyContext<'_>, expr: &ast::Expr) {
        if expr.span.from_expansion() {
            return;
        }
        if let ast::ExprKind::Match(_, ref arms, ..) = expr.kind {
            if arms.len() < 2 {
                return;
            }

            let source_map = cx.sess().source_map();

            // Collect (pattern_text, is_wildcard, span) for each arm.
            let mut arm_keys: Vec<(String, bool, Span)> = Vec::new();
            for arm in arms.iter() {
                let is_wild = matches!(arm.pat.kind, ast::PatKind::Wild);
                let snippet = source_map
                    .span_to_snippet(arm.pat.span)
                    .unwrap_or_else(|_| "_".into());
                arm_keys.push((snippet, is_wild, arm.pat.span));
            }

            // 1. Wildcards must be last.
            let mut seen_wild = false;
            for (snippet, is_wild, span) in &arm_keys {
                if seen_wild && !is_wild {
                    emit_lint(
                        cx,
                        UNSORTED_MATCH_ARMS,
                        *span,
                        format!(
                            "match arm `{snippet}` appears after wildcard `_`; wildcard must be last"
                        ),
                    );
                    return;
                }
                if *is_wild {
                    seen_wild = true;
                }
            }

            // 2. Non-wildcard arms must be alphabetically sorted.
            let non_wild: Vec<&(String, bool, Span)> =
                arm_keys.iter().filter(|(_, w, _)| !w).collect();
            let names: Vec<String> = non_wild.iter().map(|(s, _, _)| s.clone()).collect();
            if let Some((idx, prev, curr)) = first_unsorted(&names) {
                emit_lint(
                    cx,
                    UNSORTED_MATCH_ARMS,
                    non_wild[idx].2,
                    format!(
                        "match arm `{curr}` should come before `{prev}` (alphabetical order required)"
                    ),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 4. MOD_AFTER_USE
// ---------------------------------------------------------------------------

declare_lint! {
    /// **Deny** — every `mod` declaration in a module must appear before any
    /// `use` statement in that module. `cargo fmt` already orders `use`
    /// statements alphabetically, but it does not enforce the mod/use split.
    pub MOD_AFTER_USE,
    Deny,
    "`mod` declarations must precede `use` statements"
}

pub struct ModAfterUse;
impl_lint_pass!(ModAfterUse => [MOD_AFTER_USE]);

fn check_mod_after_use<T: std::ops::Deref<Target = ast::Item>>(
    cx: &EarlyContext<'_>,
    items: &[T],
) {
    let mut seen_use = false;
    for item in items.iter() {
        if item.span.from_expansion() {
            continue;
        }
        match item.kind {
            ast::ItemKind::Use(_) => {
                seen_use = true;
            }
            ast::ItemKind::Mod(..) if seen_use => {
                emit_lint(
                    cx,
                    MOD_AFTER_USE,
                    item.span,
                    "`mod` declaration must come before any `use` statement".to_string(),
                );
            }
            _ => {}
        }
    }
}

impl EarlyLintPass for ModAfterUse {
    fn check_crate(&mut self, cx: &EarlyContext<'_>, krate: &ast::Crate) {
        check_mod_after_use(cx, &krate.items);
    }

    fn check_item(&mut self, cx: &EarlyContext<'_>, item: &ast::Item) {
        if let ast::ItemKind::Mod(_, _, ast::ModKind::Loaded(ref items, ..)) = item.kind {
            check_mod_after_use(cx, items);
        }
    }
}

// ---------------------------------------------------------------------------
// 5. UNSORTED_IMPL_METHODS
// ---------------------------------------------------------------------------

declare_lint! {
    /// **Deny** — methods within an `impl` block must be grouped as
    /// (1) constructors / static methods, (2) public methods, (3) private
    /// methods, and alphabetically sorted within each group.
    pub UNSORTED_IMPL_METHODS,
    Deny,
    "impl methods must be grouped (static, public, private) and sorted alphabetically within each group"
}

pub struct UnsortedImplMethods;
impl_lint_pass!(UnsortedImplMethods => [UNSORTED_IMPL_METHODS]);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MethodGroup {
    Static = 0,
    Public = 1,
    Private = 2,
}

impl MethodGroup {
    fn label(self) -> &'static str {
        match self {
            MethodGroup::Static => "static",
            MethodGroup::Public => "public",
            MethodGroup::Private => "private",
        }
    }
}

fn classify_fn(fn_box: &ast::Fn, vis: &ast::Visibility) -> MethodGroup {
    let has_self = fn_box
        .sig
        .decl
        .inputs
        .first()
        .is_some_and(|p| p.is_self());
    if !has_self {
        MethodGroup::Static
    } else if matches!(
        vis.kind,
        ast::VisibilityKind::Public | ast::VisibilityKind::Restricted { .. }
    ) {
        MethodGroup::Public
    } else {
        MethodGroup::Private
    }
}

impl EarlyLintPass for UnsortedImplMethods {
    fn check_item(&mut self, cx: &EarlyContext<'_>, item: &ast::Item) {
        if let ast::ItemKind::Impl(ref impl_block) = item.kind {
            let methods: Vec<(String, MethodGroup, Span)> = impl_block
                .items
                .iter()
                .filter_map(|assoc| {
                    if let ast::AssocItemKind::Fn(ref fn_box) = assoc.kind {
                        let name = fn_box.ident.name.to_string();
                        let group = classify_fn(fn_box, &assoc.vis);
                        Some((name, group, assoc.span))
                    } else {
                        None
                    }
                })
                .collect();

            if methods.len() < 2 {
                return;
            }

            for w in methods.windows(2) {
                let (prev_name, prev_group, _) = &w[0];
                let (curr_name, curr_group, curr_span) = &w[1];
                if curr_group < prev_group {
                    emit_lint(
                        cx,
                        UNSORTED_IMPL_METHODS,
                        *curr_span,
                        format!(
                            "{} method `{curr_name}` must come before {} method `{prev_name}` (group order: static, public, private)",
                            curr_group.label(),
                            prev_group.label(),
                        ),
                    );
                    return;
                }
            }

            for group in [MethodGroup::Static, MethodGroup::Public, MethodGroup::Private] {
                let in_group: Vec<(String, Span)> = methods
                    .iter()
                    .filter(|(_, g, _)| *g == group)
                    .map(|(n, _, s)| (n.clone(), *s))
                    .collect();
                let names: Vec<String> = in_group.iter().map(|(n, _)| n.clone()).collect();
                if let Some((idx, prev, curr)) = first_unsorted(&names) {
                    emit_lint(
                        cx,
                        UNSORTED_IMPL_METHODS,
                        in_group[idx].1,
                        format!(
                            "{} method `{curr}` should come before `{prev}` (alphabetical within group)",
                            group.label(),
                        ),
                    );
                    return;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 6. UNSORTED_DERIVES
// ---------------------------------------------------------------------------

declare_lint! {
    /// **Deny** — `#[derive(...)]` attributes must list traits in alphabetical
    /// order.
    pub UNSORTED_DERIVES,
    Deny,
    "#[derive(...)] traits must be in alphabetical order"
}

pub struct UnsortedDerives;
impl_lint_pass!(UnsortedDerives => [UNSORTED_DERIVES]);

fn is_local_source_path(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy();
    !s.contains("/.cargo/")
        && !s.contains("/.rustup/")
        && !s.contains("/rustlib/")
        && !s.starts_with("<")
}

/// Find every `#[derive(...)]` attribute in `src` and return its
/// `(open_bracket, close_bracket_inclusive_end)` byte range plus the
/// inner traits list (raw, untrimmed) for sort-checking.
fn find_derive_attrs(src: &str) -> Vec<(usize, usize, String)> {
    let bytes = src.as_bytes();
    let needle = b"#[derive(";
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let start = i;
            let mut j = i + needle.len();
            let mut depth: i32 = 1;
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            if depth == 0 && j < bytes.len() && bytes[j] == b']' {
                let inner_start = i + needle.len();
                let inner_end = j - 1;
                let inner = &src[inner_start..inner_end];
                out.push((start, j + 1, inner.to_string()));
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

impl EarlyLintPass for UnsortedDerives {
    fn check_crate(&mut self, cx: &EarlyContext<'_>, _krate: &ast::Crate) {
        let source_map = cx.sess().source_map();
        for file in source_map.files().iter() {
            let path = match &file.name {
                rustc_span::FileName::Real(real) => real.local_path_if_available().to_path_buf(),
                _ => continue,
            };
            if !is_local_source_path(&path) {
                continue;
            }
            let Some(src) = file.src.as_ref() else {
                continue;
            };
            let base = file.start_pos;
            for (lo, hi, inner) in find_derive_attrs(src) {
                let names: Vec<String> = inner
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if names.len() < 2 {
                    continue;
                }
                if let Some((_idx, prev, curr)) = first_unsorted(&names) {
                    let span = Span::with_root_ctxt(
                        base + rustc_span::BytePos(lo as u32),
                        base + rustc_span::BytePos(hi as u32),
                    );
                    emit_lint(
                        cx,
                        UNSORTED_DERIVES,
                        span,
                        format!(
                            "derive trait `{curr}` should come before `{prev}` (alphabetical order required)"
                        ),
                    );
                }
            }
        }
    }
}
