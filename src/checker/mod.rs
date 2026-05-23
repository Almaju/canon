use crate::ast::*;
use crate::error::OnewayError;
use std::collections::{HashMap, HashSet};

const BUILTIN_TYPES: &[&str] = &[
    "Bool",
    "Deserialize",
    "False",
    "Float",
    "Hex",
    "Int",
    "Network",
    "Never",
    "Off",
    "On",
    "Random",
    "Serialize",
    "Stderr",
    "Stdin",
    "Stdout",
    "String",
    "True",
    "Unit",
];

const CAPABILITY_TYPES: &[&str] = &["Network", "Random", "Stderr", "Stdin", "Stdout"];

fn is_capability_type(name: &str) -> bool {
    CAPABILITY_TYPES.contains(&name)
}

const BUILTIN_GENERIC_TYPES: &[&str] = &["List", "Map", "Option", "Result", "Set"];

/// Zero-data builtin types that may be constructed with empty parens: `Unit()`, `Off()`, `On()`.
/// `True()` and `False()` are covered by `is_variant` (variants of `Bool`).
const ZERO_DATA_BUILTINS: &[&str] = &["False", "Off", "On", "True", "Unit"];

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
}

pub struct MethodSig {
    pub arity: usize,
    pub return_ty: String,
}

impl SymbolTable {
    pub fn knows_type(&self, name: &str) -> bool {
        self.types.contains(name) || self.generic_types.contains(name)
    }
}

pub fn check(module: &Module) -> Vec<OnewayError> {
    check_with_entry(module, 0)
}

/// Variant of `check` that limits per-file ordering rules (free-function
/// and type-definition alphabetical order) to items at or after
/// `entry_items_start`. Items before that index originated from `use`
/// imports and follow their own ordering — they are not the entry file's
/// concern.
pub fn check_with_entry(module: &Module, entry_items_start: usize) -> Vec<OnewayError> {
    let mut errors = Vec::new();
    let symbols = collect_symbols(module, &mut errors);

    let mut main_found = false;
    for item in &module.items {
        match item {
            Item::Function(func) => check_function(func, &symbols, &mut errors, &mut main_found),
            Item::TypeDef(td) => check_type_def(td, &symbols, &mut errors),
            Item::Use(_) => {}
        }
    }

    check_ordering(module, entry_items_start, &mut errors);

    if !main_found {
        errors.push(OnewayError::CheckError {
            message: "no `main` entry point defined".to_string(),
            span: module.span,
        });
    }

    errors
}

fn check_ordering(module: &Module, entry_items_start: usize, errors: &mut Vec<OnewayError>) {
    let entry_items = &module.items[entry_items_start..];
    // Union variants and product fields are checked in check_type_expr (covers
    // every position they appear in, not just top-level TypeDef bodies).

    // Functions on the same receiver type must be declared alphabetically.
    let mut methods_per_receiver: HashMap<String, Vec<(String, crate::error::Span)>> =
        HashMap::new();
    for item in entry_items {
        if let Item::Function(func) = item {
            if let Some(recv) = &func.receiver {
                methods_per_receiver
                    .entry(recv.name.clone())
                    .or_default()
                    .push((func.name.name.clone(), func.name.span));
            }
        }
    }
    for methods in methods_per_receiver.values() {
        let pairs: Vec<(&str, crate::error::Span)> =
            methods.iter().map(|(n, s)| (n.as_str(), *s)).collect();
        check_sorted_named("method declaration", &pairs, errors);
    }

    // Free functions (no receiver) declared in the entry file must be
    // alphabetical. Imported items are exempt — they follow their own
    // file's ordering.
    let free_funcs: Vec<(&str, crate::error::Span)> = entry_items
        .iter()
        .filter_map(|item| {
            if let Item::Function(func) = item {
                if func.receiver.is_none() {
                    return Some((func.name.name.as_str(), func.name.span));
                }
            }
            None
        })
        .collect();
    check_sorted_named("function declaration", &free_funcs, errors);

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

    // `use` imports must come first and be alphabetical.
    let mut seen_non_use = false;
    for item in &module.items {
        match item {
            Item::Use(u) => {
                if seen_non_use {
                    errors.push(OnewayError::CheckError {
                        message: format!(
                            "`use {}` must appear before any type or function definitions",
                            u.name.name
                        ),
                        span: u.span,
                    });
                }
            }
            _ => seen_non_use = true,
        }
    }
    let use_names: Vec<(&str, crate::error::Span)> = module
        .items
        .iter()
        .filter_map(|i| {
            if let Item::Use(u) = i {
                Some((u.name.name.as_str(), u.span))
            } else {
                None
            }
        })
        .collect();
    check_sorted_named("`use` import", &use_names, errors);
}

fn check_sorted_named(
    kind: &str,
    items: &[(&str, crate::error::Span)],
    errors: &mut Vec<OnewayError>,
) {
    for window in items.windows(2) {
        let (prev, _) = window[0];
        let (next, span) = window[1];
        if next < prev {
            errors.push(OnewayError::CheckError {
                message: format!(
                    "{}s must be in alphabetical order — `{}` should come before `{}`",
                    kind, next, prev
                ),
                span,
            });
        }
    }
}

fn collect_symbols(module: &Module, errors: &mut Vec<OnewayError>) -> SymbolTable {
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

    // `use Foo` (or `use path/Foo`) imports the type `Foo` — register it as
    // known so references to it in the same file are not flagged as unknown.
    for item in &module.items {
        if let Item::Use(u) = item {
            let type_name = u.name.name.split('/').next_back().unwrap_or(&u.name.name);
            types.insert(type_name.to_string());
        }
    }

    for item in &module.items {
        if let Item::TypeDef(td) = item {
            let name = td.name.name.clone();
            let already_known = types.contains(&name) || generic_types.contains(&name);
            if already_known {
                errors.push(OnewayError::CheckError {
                    message: format!("duplicate type definition `{}`", name),
                    span: td.name.span,
                });
            } else if td.generic_params.is_empty() {
                types.insert(name);
            } else {
                generic_types.insert(name);
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

    let mut product_fields: HashMap<String, Vec<String>> = HashMap::new();
    for item in &module.items {
        if let Item::TypeDef(td) = item {
            if let TypeExpr::Product { fields, .. } = &td.body {
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
        }
    }

    let mut methods: HashMap<(String, String), MethodSig> = HashMap::new();
    for item in &module.items {
        if let Item::Function(func) = item {
            if let Some(recv) = &func.receiver {
                let return_ty = match &func.return_ty {
                    TypeExpr::Named { name, .. } => name.clone(),
                    _ => "<complex>".to_string(),
                };
                methods.insert(
                    (recv.name.clone(), func.name.name.clone()),
                    MethodSig {
                        arity: func.params.len(),
                        return_ty: return_ty.clone(),
                    },
                );
                // Register under each param type for commutative calling.
                // For constructors (name == "Self"), also register the TYPE NAME as the method
                // so that `param_val.TypeName()` (commutative constructor call) is recognized.
                // For product-type params (A * B), register each component separately.
                // `ctor_arity` is the number of remaining args when that component is the receiver.
                let mut components: Vec<(String, usize)> = Vec::new();
                for param in &func.params {
                    match &param.ty {
                        TypeExpr::Named { .. } => {
                            if let Some(n) = param.ty.simple_name() {
                                components.push((n.to_string(), 0));
                            }
                        }
                        TypeExpr::Product { fields, .. } => {
                            let remaining = fields.len().saturating_sub(1);
                            for field in fields {
                                if let Some(n) = field.simple_name() {
                                    components.push((n.to_string(), remaining));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                for (param_name, ctor_arity) in &components {
                    methods
                        .entry((param_name.clone(), func.name.name.clone()))
                        .or_insert(MethodSig {
                            arity: func.params.len(),
                            return_ty: return_ty.clone(),
                        });
                    if func.name.name == "Self" {
                        // e.g. "str".JsonValue() (arity 0) or Port(...).HttpServer(state) (arity 1)
                        methods
                            .entry((param_name.clone(), recv.name.clone()))
                            .or_insert(MethodSig {
                                arity: *ctor_arity,
                                return_ty: return_ty.clone(),
                            });
                    }
                }
            }
        }
    }

    let mut standalone_types: HashSet<String> = HashSet::new();
    for item in &module.items {
        if let Item::TypeDef(td) = item {
            standalone_types.insert(td.name.name.clone());
        }
    }

    SymbolTable {
        types,
        generic_types,
        variant_of,
        methods,
        product_fields,
        standalone_types,
    }
}

fn check_self_constructor_signature(
    func: &FunctionDef,
    receiver_name: &str,
    errors: &mut Vec<OnewayError>,
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
        errors.push(OnewayError::CheckError {
            message: format!(
                "constructor `{}` must return `{}`, `Result<{}, E>`, or `Option<{}>`",
                receiver_name, receiver_name, receiver_name, receiver_name
            ),
            span: func.return_ty.span(),
        });
    }
}

fn check_type_def(td: &TypeDef, symbols: &SymbolTable, errors: &mut Vec<OnewayError>) {
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
    errors: &mut Vec<OnewayError>,
    main_found: &mut bool,
) {
    if func.name.name == "main" {
        if *main_found {
            errors.push(OnewayError::CheckError {
                message: "duplicate `main` definition".to_string(),
                span: func.span,
            });
        }
        *main_found = true;

        if func.receiver.is_some() {
            errors.push(OnewayError::CheckError {
                message: "`main` is the entry point and must not have a receiver".to_string(),
                span: func.span,
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
            errors.push(OnewayError::CheckError {
                message: format!("unknown receiver type `{}`", recv.name),
                span: recv.span,
            });
        }
        if func.name.name == "Self" {
            check_self_constructor_signature(func, &recv.name, errors);
        }
    }

    if func.extern_rust.is_some() {
        return;
    }

    let scope = ExprScope::from_function(func);
    check_block(&func.body, &func.return_ty, &scope, symbols, errors);
}

fn check_type_expr(
    ty: &TypeExpr,
    symbols: &SymbolTable,
    generic_scope: &HashSet<String>,
    errors: &mut Vec<OnewayError>,
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
                // extern type alias body — the Rust path isn't an Oneway type
            } else if generic_scope.contains(name) {
                if !generics.is_empty() {
                    errors.push(OnewayError::CheckError {
                        message: format!(
                            "type parameter `{}` cannot be applied to type arguments",
                            name
                        ),
                        span: *span,
                    });
                }
            } else if !symbols.knows_type(name) {
                errors.push(OnewayError::CheckError {
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
    errors: &mut Vec<OnewayError>,
) {
    if block.exprs.is_empty() {
        errors.push(OnewayError::CheckError {
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
    if last_ty != return_ty_name && last_ty != "<unknown>" {
        errors.push(OnewayError::CheckError {
            message: format!(
                "function returns `{}` but last expression has type `{}`",
                return_ty_name, last_ty
            ),
            span: last.span(),
        });
    }
}

fn check_expr(
    expr: &Expr,
    scope: &ExprScope,
    symbols: &SymbolTable,
    errors: &mut Vec<OnewayError>,
) {
    match expr {
        Expr::Ident(ident) => {
            if is_capability_type(&ident.name) {
                if !scope.contains(&ident.name) {
                    errors.push(OnewayError::CheckError {
                        message: format!(
                            "capability `{}` must be received as a parameter — capabilities cannot be conjured",
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
                    errors.push(OnewayError::CheckError {
                        message: format!("unknown name `{}`", ident.name),
                        span: ident.span,
                    });
                }
            }
        }
        Expr::StringLit { .. } => {}
        Expr::IntLit { .. } | Expr::FloatLit { .. } | Expr::HexLit { .. } => {}
        Expr::JsonLit { .. } => {}
        Expr::Constructor { name, args, span } => {
            let is_variant = symbols.variant_of.contains_key(&name.name);
            if !symbols.knows_type(&name.name) && !is_variant {
                errors.push(OnewayError::CheckError {
                    message: format!("unknown type `{}` in constructor", name.name),
                    span: name.span,
                });
            }
            if args.is_empty() && !is_variant {
                let is_zero_data_builtin = ZERO_DATA_BUILTINS.contains(&name.name.as_str());
                let has_zero_arg_ctor = symbols
                    .methods
                    .get(&(name.name.clone(), "Self".to_string()))
                    .map(|sig| sig.arity == 0)
                    .unwrap_or(false);
                if !is_zero_data_builtin && !has_zero_arg_ctor {
                    errors.push(OnewayError::CheckError {
                        message: format!(
                            "constructor `{}()` is not allowed — empty constructors are disallowed",
                            name.name
                        ),
                        span: *span,
                    });
                }
            }
            for arg in args {
                check_expr(arg, scope, symbols, errors);
            }
        }
        Expr::MethodCall {
            receiver,
            method,
            type_args,
            args,
            span,
        } => {
            check_expr(receiver, scope, symbols, errors);
            for arg in args {
                check_expr(arg, scope, symbols, errors);
            }
            let empty_generic_scope: HashSet<String> = HashSet::new();
            for ta in type_args {
                check_type_expr(ta, symbols, &empty_generic_scope, errors);
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
            let known = is_known_method(&recv_ty_specific, &method.name, effective_arity)
                || symbols
                    .methods
                    .get(&(recv_ty_specific.clone(), method.name.clone()))
                    .map(|m| m.arity == effective_arity)
                    .unwrap_or(false)
                || (recv_ty_specific != recv_ty
                    && (is_known_method(&recv_ty, &method.name, effective_arity)
                        || symbols
                            .methods
                            .get(&(recv_ty.clone(), method.name.clone()))
                            .map(|m| m.arity == effective_arity)
                            .unwrap_or(false)));
            if !known {
                errors.push(OnewayError::CheckError {
                    message: format!(
                        "no method `{}` on type `{}` with {} argument(s)",
                        method.name, recv_ty, effective_arity
                    ),
                    span: *span,
                });
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
            for arm in arms {
                // Validate param_ty and return_ty as type expressions
                check_type_expr(&arm.param_ty, symbols, &generic_scope, errors);
                check_type_expr(&arm.return_ty, symbols, &generic_scope, errors);
                // Verify the variant belongs to the scrutinee's type
                if let TypeExpr::Named {
                    name: variant_name,
                    span: vspan,
                    ..
                } = &arm.param_ty
                {
                    if !scrutinee_ty.is_empty() && scrutinee_ty != "<unknown>" {
                        let pattern_enum = symbols.variant_of.get(variant_name.as_str());
                        if pattern_enum.map(|s| s.as_str()) != Some(scrutinee_ty.as_str()) {
                            errors.push(OnewayError::CheckError {
                                message: format!(
                                    "pattern `{}` is not a variant of `{}`",
                                    variant_name, scrutinee_ty
                                ),
                                span: *vspan,
                            });
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
                errors.push(OnewayError::CheckError {
                    message: "dispatch expression must have at least one arm".to_string(),
                    span: *span,
                });
            }
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
            // Case 1: product field access
            if let Some(fields) = symbols.product_fields.get(&recv_ty_for_lookup) {
                if fields.iter().any(|f| f == &field.name) {
                    return; // valid product field
                }
                errors.push(OnewayError::CheckError {
                    message: format!(
                        "type `{}` has no field `{}`",
                        recv_ty_for_lookup, field.name
                    ),
                    span: *span,
                });
                return;
            }
            // Case 2: first-class method reference (extern or Oneway-defined)
            if symbols
                .methods
                .contains_key(&(recv_ty_for_lookup.clone(), field.name.clone()))
            {
                return; // valid method reference used as a value
            }
            // Case 3: zero-arg built-in method used without parens (e.g. "hello".print)
            if is_known_method(&recv_ty_for_lookup, &field.name, 0) {
                return;
            }
            errors.push(OnewayError::CheckError {
                message: format!(
                    "field access `.{}` on `{}` — not a product field and no method `{}` found",
                    field.name, recv_ty_for_lookup, field.name
                ),
                span: *span,
            });
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
    }
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

fn is_known_method(receiver_ty: &str, method: &str, arg_count: usize) -> bool {
    if receiver_ty == "<unknown>" || receiver_ty == "Self" {
        return true;
    }
    if matches!(
        (receiver_ty, method, arg_count),
        ("String", "print", 0)
            | ("String", "print", 1)
            | ("Int", "print", 0)
            | ("Int", "print", 1)
            | ("Float", "print", 0)
            | ("Float", "print", 1)
            | ("Hex", "print", 0)
            | ("Hex", "print", 1)
            | ("Bool", "print", 0)
            | ("Bool", "print", 1)
    ) {
        return true;
    }
    if matches!(receiver_ty, "Int" | "Float")
        && matches!(
            method,
            "add" | "sub" | "mul" | "div" | "rem" | "eq" | "lt" | "gt" | "lte" | "gte"
        )
        && arg_count == 1
    {
        return true;
    }
    if receiver_ty == "Bool" && matches!(method, "not") && arg_count == 0 {
        return true;
    }
    if receiver_ty == "Bool" && matches!(method, "and" | "or") && arg_count == 1 {
        return true;
    }
    if receiver_ty == "String" && method == "concat" && arg_count == 1 {
        return true;
    }
    if receiver_ty == "List"
        && matches!(
            (method, arg_count),
            ("length", 0) | ("first", 0) | ("map", 1)
        )
    {
        return true;
    }
    if receiver_ty == "Map" {
        return matches!(
            (method, arg_count),
            ("empty", 0)
                | ("get", 1)
                | ("insert", 2)
                | ("keys", 0)
                | ("length", 0)
                | ("remove", 1)
                | ("values", 0)
        );
    }
    if receiver_ty == "Set" {
        return matches!(
            (method, arg_count),
            ("contains", 1) | ("empty", 0) | ("insert", 1) | ("length", 0) | ("remove", 1)
        );
    }
    false
}

fn expr_type_name_in_scope(expr: &Expr, symbols: &SymbolTable) -> String {
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
        Expr::HexLit { .. } => "Hex".to_string(),
        Expr::Constructor { name, .. } => {
            if let Some(parent) = symbols.variant_of.get(&name.name) {
                parent.clone()
            } else {
                name.name.clone()
            }
        }
        Expr::MethodCall {
            receiver, method, ..
        } => {
            let recv_ty = expr_type_name_in_scope(receiver, symbols);
            if let Some(sig) = symbols.methods.get(&(recv_ty.clone(), method.name.clone())) {
                return sig.return_ty.clone();
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
            if let Expr::Constructor { name, args, .. } = &**inner {
                if matches!(name.name.as_str(), "Ok" | "Some") && !args.is_empty() {
                    return expr_type_name_in_scope(&args[0], symbols);
                }
            }
            "<unknown>".to_string()
        }
        Expr::Lambda { return_ty, .. } => match return_ty {
            TypeExpr::Named { name, .. } => name.clone(),
            _ => "<unknown>".to_string(),
        },
        Expr::ProductValue { .. } => "<unknown>".to_string(),
        Expr::FieldAccess {
            receiver, field, ..
        } => {
            // If this is a zero-arg builtin method used without parens, return its
            // return type rather than the field name (e.g. "hello".print -> Unit).
            let recv_ty = expr_type_name_in_scope(receiver, symbols);
            let ret = method_return_type(&recv_ty, &field.name);
            if ret != "<unknown>" {
                return ret;
            }
            if let Some(sig) = symbols.methods.get(&(recv_ty, field.name.clone())) {
                return sig.return_ty.clone();
            }
            field.name.clone()
        }
        Expr::JsonLit { .. } => "JsonValue".to_string(),
    }
}

fn method_return_type(receiver_ty: &str, method: &str) -> String {
    match (receiver_ty, method) {
        ("String", "print")
        | ("Int", "print")
        | ("Float", "print")
        | ("Hex", "print")
        | ("Bool", "print") => "Unit".to_string(),
        ("Int", "add" | "sub" | "mul" | "div" | "rem") => "Int".to_string(),
        ("Float", "add" | "sub" | "mul" | "div" | "rem") => "Float".to_string(),
        ("Int", "eq" | "lt" | "gt" | "lte" | "gte") => "Bool".to_string(),
        ("Float", "eq" | "lt" | "gt" | "lte" | "gte") => "Bool".to_string(),
        ("Bool", "not" | "and" | "or") => "Bool".to_string(),
        ("String", "concat") => "String".to_string(),
        ("List", "length") => "Int".to_string(),
        ("List", "map") => "List".to_string(),
        ("List", "first") => "Option".to_string(),
        ("Map", "empty") => "Map".to_string(),
        ("Map", "get") => "Option".to_string(),
        ("Map", "insert") => "Map".to_string(),
        ("Map", "keys") => "List".to_string(),
        ("Map", "length") => "Int".to_string(),
        ("Map", "remove") => "Map".to_string(),
        ("Map", "values") => "List".to_string(),
        ("Set", "contains") => "Bool".to_string(),
        ("Set", "empty") => "Set".to_string(),
        ("Set", "insert") => "Set".to_string(),
        ("Set", "length") => "Int".to_string(),
        ("Set", "remove") => "Set".to_string(),
        _ => "<unknown>".to_string(),
    }
}
