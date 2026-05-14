use crate::ast::*;
use crate::error::OnewayError;
use std::collections::{HashMap, HashSet};

const BUILTIN_TYPES: &[&str] = &[
    "Float", "Hex", "Int", "Noop", "Off", "On", "Self", "Stderr", "Stdin", "Stdout", "String",
];

const BUILTIN_GENERIC_TYPES: &[&str] = &["List", "Map", "Option", "Result", "Set"];

pub struct SymbolTable {
    pub types: HashSet<String>,
    pub generic_types: HashSet<String>,
    pub variant_of: HashMap<String, String>,
    pub methods: HashMap<(String, String), MethodSig>,
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

    if !main_found {
        errors.push(OnewayError::CheckError {
            message: "no `main` entry point defined".to_string(),
            span: module.span,
        });
    }

    errors
}

fn collect_symbols(module: &Module, errors: &mut Vec<OnewayError>) -> SymbolTable {
    let mut types: HashSet<String> = BUILTIN_TYPES.iter().map(|s| s.to_string()).collect();
    let mut generic_types: HashSet<String> =
        BUILTIN_GENERIC_TYPES.iter().map(|s| s.to_string()).collect();
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
                        if !types.contains(&name_s) && !generic_types.contains(&name_s) {
                            types.insert(name_s.clone());
                            variant_of.insert(name_s, td.name.name.clone());
                        }
                    }
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
                        return_ty,
                    },
                );
            }
        }
    }

    SymbolTable {
        types,
        generic_types,
        variant_of,
        methods,
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
        TypeExpr::Named { name, generics, span } => {
            if name == "Self" {
                // allowed in method bodies / trait declarations; not validated here
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
            for v in variants {
                check_type_expr(v, symbols, generic_scope, errors);
            }
        }
        TypeExpr::Product { fields, .. } => {
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
        let mut names: Vec<String> = func
            .params
            .iter()
            .filter_map(|p| p.ty.simple_name().map(|s| s.to_string()))
            .collect();
        if let Some(recv) = &func.receiver {
            names.push(recv.name.clone());
        }
        Self { names }
    }

    fn contains(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
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
            args,
            span,
        } => {
            check_expr(receiver, scope, symbols, errors);
            for arg in args {
                check_expr(arg, scope, symbols, errors);
            }
            let recv_ty = expr_type_name_in_scope(receiver, symbols);
            let known = is_known_method(&recv_ty, &method.name, args.len())
                || symbols
                    .methods
                    .get(&(recv_ty.clone(), method.name.clone()))
                    .map(|m| m.arity == args.len())
                    .unwrap_or(false);
            if !known {
                errors.push(OnewayError::CheckError {
                    message: format!(
                        "no method `{}` on type `{}` with {} argument(s)",
                        method.name,
                        recv_ty,
                        args.len()
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
                    name,
                    span: pspan,
                    ..
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
        Expr::While { cond, body, .. } => {
            check_expr(cond, scope, symbols, errors);
            for expr in &body.exprs {
                check_expr(expr, scope, symbols, errors);
            }
        }
    }
}

fn is_known_method(receiver_ty: &str, method: &str, arg_count: usize) -> bool {
    if receiver_ty == "<unknown>" || receiver_ty == "Self" {
        return true;
    }
    matches!(
        (receiver_ty, method, arg_count),
        ("String", "print", 1)
            | ("Int", "print", 1)
            | ("Float", "print", 1)
            | ("Hex", "print", 1)
    )
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
            if let Some(sig) = symbols
                .methods
                .get(&(recv_ty.clone(), method.name.clone()))
            {
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
        Expr::While { .. } => "Noop".to_string(),
    }
}

fn method_return_type(receiver_ty: &str, method: &str) -> String {
    match (receiver_ty, method) {
        ("String", "print") | ("Int", "print") | ("Float", "print") | ("Hex", "print") => {
            "Noop".to_string()
        }
        _ => "<unknown>".to_string(),
    }
}
