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

    SymbolTable {
        types,
        generic_types,
        variant_of,
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
    let last_ty = expr_type_name(last);
    let return_ty_name = match return_ty {
        TypeExpr::Named { name, .. } => name.clone(),
        _ => "<complex>".to_string(),
    };
    if last_ty != return_ty_name {
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
            let recv_ty = expr_type_name(receiver);
            if !is_known_method(&recv_ty, &method.name, args.len()) {
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
    }
}

fn is_known_method(receiver_ty: &str, method: &str, arg_count: usize) -> bool {
    matches!((receiver_ty, method, arg_count), ("String", "print", 1))
}

fn expr_type_name(expr: &Expr) -> String {
    match expr {
        Expr::Ident(ident) => ident.name.clone(),
        Expr::StringLit { .. } => "String".to_string(),
        Expr::MethodCall {
            receiver, method, ..
        } => {
            let recv_ty = expr_type_name(receiver);
            method_return_type(&recv_ty, &method.name)
        }
    }
}

fn method_return_type(receiver_ty: &str, method: &str) -> String {
    match (receiver_ty, method) {
        ("String", "print") => "Noop".to_string(),
        _ => "<unknown>".to_string(),
    }
}
