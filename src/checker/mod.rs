use crate::ast::*;
use crate::error::OnewayError;
use std::collections::{HashMap, HashSet};

const BUILTIN_TYPES: &[&str] = &[
    "Clock",
    "Deserialize",
    "Filesystem",
    "Float",
    "Hex",
    "HttpClient",
    "Int",
    "Json",
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
    "Unit",
];

const CAPABILITY_TYPES: &[&str] = &[
    "Clock",
    "Filesystem",
    "HttpClient",
    "Json",
    "Network",
    "Random",
    "Stderr",
    "Stdin",
    "Stdout",
];

fn is_capability_type(name: &str) -> bool {
    CAPABILITY_TYPES.contains(&name)
}

const BUILTIN_GENERIC_TYPES: &[&str] = &["List", "Map", "Option", "Result", "Set"];

pub struct SymbolTable {
    pub types: HashSet<String>,
    pub generic_types: HashSet<String>,
    pub variant_of: HashMap<String, String>,
    pub methods: HashMap<(String, String), MethodSig>,
    /// For each product TypeDef `T = A * B * ...`, the names of its
    /// component types (in declaration order). Used to validate
    /// `value.Field` access.
    pub product_fields: HashMap<String, Vec<String>>,
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
    for item in &module.items {
        if let Item::Function(func) = item {
            if let Some(recv) = &func.receiver {
                methods_per_receiver
                    .entry(recv.name.clone())
                    .or_default()
                    .push((func.name.name.clone(), func.name.span));
            }
        }
    }
    for (_recv, methods) in &methods_per_receiver {
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
                // Register under each param type for commutative calling
                for param in &func.params {
                    if let Some(param_name) = param.ty.simple_name() {
                        methods
                            .entry((param_name.to_string(), func.name.name.clone()))
                            .or_insert(MethodSig {
                                arity: func.params.len(),
                                return_ty: return_ty.clone(),
                            });
                    }
                }
            }
        }
    }

    SymbolTable {
        types,
        generic_types,
        variant_of,
        methods,
        product_fields,
    }
}

fn check_self_constructor_signature(
    func: &FunctionDef,
    receiver_name: &str,
    errors: &mut Vec<OnewayError>,
) {
    let valid = match &func.return_ty {
        TypeExpr::Named { name, generics, .. } => {
            if name == receiver_name && generics.is_empty() {
                true
            } else if (name == "Result" || name == "Option") && !generics.is_empty() {
                matches!(
                    &generics[0],
                    TypeExpr::Named { name, generics, .. }
                        if name == receiver_name && generics.is_empty()
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
        Expr::Constructor { name, args, span } => {
            let is_variant = symbols.variant_of.contains_key(&name.name);
            if !symbols.knows_type(&name.name) && !is_variant {
                errors.push(OnewayError::CheckError {
                    message: format!("unknown type `{}` in constructor", name.name),
                    span: name.span,
                });
            }
            if args.is_empty() && !is_variant {
                errors.push(OnewayError::CheckError {
                    message: format!(
                        "constructor `{}()` is not allowed — empty constructors are disallowed",
                        name.name
                    ),
                    span: *span,
                });
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
            let effective_arity = effective_call_arity(args);
            let known = is_known_method(&recv_ty, &method.name, effective_arity)
                || symbols
                    .methods
                    .get(&(recv_ty.clone(), method.name.clone()))
                    .map(|m| m.arity == effective_arity)
                    .unwrap_or(false);
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
            for arm in arms {
                if let Pattern::Variant {
                    name, span: pspan, ..
                } = &arm.pattern
                {
                    let pattern_enum = symbols.variant_of.get(name);
                    if pattern_enum.map(|s| s.as_str()) != Some(scrutinee_ty.as_str())
                        && !scrutinee_ty.is_empty()
                        && scrutinee_ty != "<unknown>"
                    {
                        errors.push(OnewayError::CheckError {
                            message: format!(
                                "pattern `{}` is not a variant of `{}`",
                                name, scrutinee_ty
                            ),
                            span: *pspan,
                        });
                    }
                }
                check_expr(&arm.body, scope, symbols, errors);
            }
            if arms.is_empty() {
                errors.push(OnewayError::CheckError {
                    message: "match expression must have at least one arm".to_string(),
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
            if recv_ty == "<unknown>" {
                return;
            }
            match symbols.product_fields.get(&recv_ty) {
                Some(fields) if fields.iter().any(|f| f == &field.name) => {}
                Some(_) => {
                    errors.push(OnewayError::CheckError {
                        message: format!(
                            "type `{}` has no field `{}`",
                            recv_ty, field.name
                        ),
                        span: *span,
                    });
                }
                None => {
                    errors.push(OnewayError::CheckError {
                        message: format!(
                            "field access `.{}` requires a product type — `{}` is not a product",
                            field.name, recv_ty
                        ),
                        span: *span,
                    });
                }
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
        ("String", "print", 1)
            | ("Int", "print", 1)
            | ("Float", "print", 1)
            | ("Hex", "print", 1)
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
    if receiver_ty == "List" {
        if matches!(
            (method, arg_count),
            ("length", 0) | ("first", 0) | ("map", 1)
        ) {
            return true;
        }
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
            .map(|arm| expr_type_name_in_scope(&arm.body, symbols))
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
        Expr::FieldAccess { field, .. } => field.name.clone(),
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
        _ => "<unknown>".to_string(),
    }
}
