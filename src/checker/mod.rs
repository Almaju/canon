use crate::ast::*;
use crate::error::{OnewayError, Span};

const BUILTIN_TYPES: &[&str] = &["Noop"];

pub fn check(module: &Module) -> Vec<OnewayError> {
    let mut errors = Vec::new();
    let mut main_found = false;

    for item in &module.items {
        let Item::Function(func) = item;
        check_function(func, &mut errors, &mut main_found);
    }

    if !main_found {
        errors.push(OnewayError::CheckError {
            message: "no `main` entry point defined".to_string(),
            span: module.span,
        });
    }

    errors
}

fn check_function(func: &FunctionDef, errors: &mut Vec<OnewayError>, main_found: &mut bool) {
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
        if !func.params.is_empty() {
            errors.push(OnewayError::CheckError {
                message: "`main` cannot take parameters yet (capability support comes later)"
                    .to_string(),
                span: func.params[0].span,
            });
        }
    }

    check_type_known(&func.return_ty, errors);
    check_block(&func.body, &func.return_ty, errors);
}

fn check_block(block: &Block, return_ty: &TypeExpr, errors: &mut Vec<OnewayError>) {
    if block.exprs.is_empty() {
        errors.push(OnewayError::CheckError {
            message: "function body must contain at least one expression".to_string(),
            span: block.span,
        });
        return;
    }

    for expr in &block.exprs {
        check_expr(expr, errors);
    }

    let last_ty = expr_type_name(block.exprs.last().unwrap());
    if last_ty != return_ty.name {
        errors.push(OnewayError::CheckError {
            message: format!(
                "function returns `{}` but last expression has type `{}`",
                return_ty.name, last_ty
            ),
            span: expr_span(block.exprs.last().unwrap()),
        });
    }
}

fn check_expr(expr: &Expr, errors: &mut Vec<OnewayError>) {
    let Expr::Ident(ident) = expr;
    if !BUILTIN_TYPES.contains(&ident.name.as_str()) {
        errors.push(OnewayError::CheckError {
            message: format!("unknown name `{}`", ident.name),
            span: ident.span,
        });
    }
}

fn check_type_known(ty: &TypeExpr, errors: &mut Vec<OnewayError>) {
    if !BUILTIN_TYPES.contains(&ty.name.as_str()) {
        errors.push(OnewayError::CheckError {
            message: format!("unknown type `{}`", ty.name),
            span: ty.span,
        });
    }
}

fn expr_type_name(expr: &Expr) -> String {
    let Expr::Ident(ident) = expr;
    ident.name.clone()
}

fn expr_span(expr: &Expr) -> Span {
    let Expr::Ident(ident) = expr;
    ident.span
}
