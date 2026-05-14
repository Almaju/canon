use crate::ast::*;
use std::fmt::Write;

pub fn generate(module: &Module) -> String {
    let mut out = String::new();
    for item in &module.items {
        let Item::Function(func) = item;
        emit_function(&mut out, func);
        out.push('\n');
    }
    out
}

fn emit_function(out: &mut String, func: &FunctionDef) {
    let is_entry = func.receiver.is_none() && func.name.name == "main";

    if is_entry {
        out.push_str("fn main() {\n");
        emit_block_body(out, &func.body, /* is_main */ true);
        out.push_str("}\n");
    } else {
        let _ = write!(
            out,
            "fn {}() -> {} {{\n",
            func.name.name,
            rust_type(&func.return_ty.name)
        );
        emit_block_body(out, &func.body, false);
        out.push_str("}\n");
    }
}

fn emit_block_body(out: &mut String, block: &Block, is_main: bool) {
    let last_idx = block.exprs.len().saturating_sub(1);
    for (i, expr) in block.exprs.iter().enumerate() {
        out.push_str("    ");
        emit_expr(out, expr);
        if is_main || i != last_idx {
            out.push(';');
        }
        out.push('\n');
    }
}

fn emit_expr(out: &mut String, expr: &Expr) {
    match expr {
        Expr::Ident(ident) => {
            out.push_str(&rust_value(&ident.name));
        }
    }
}

fn rust_type(name: &str) -> String {
    match name {
        "Noop" => "()".to_string(),
        other => other.to_string(),
    }
}

fn rust_value(name: &str) -> String {
    match name {
        "Noop" => "()".to_string(),
        other => other.to_string(),
    }
}
