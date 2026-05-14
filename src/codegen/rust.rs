use crate::ast::*;
use std::collections::HashMap;
use std::fmt::Write;

pub fn generate(module: &Module) -> String {
    let cg = Codegen::from_module(module);
    let mut out = String::new();
    for item in &module.items {
        match item {
            Item::Function(func) => {
                cg.emit_function(&mut out, func);
                out.push('\n');
            }
            Item::TypeDef(td) => {
                cg.emit_type_def(&mut out, td);
                out.push('\n');
            }
        }
    }
    out
}

struct Codegen {
    variant_of: HashMap<String, String>,
}

impl Codegen {
    fn from_module(module: &Module) -> Self {
        let mut variant_of = HashMap::new();
        for item in &module.items {
            if let Item::TypeDef(td) = item {
                if let TypeExpr::Union { variants, .. } = &td.body {
                    if variants.iter().all(|t| {
                        matches!(t, TypeExpr::Named { generics, .. } if generics.is_empty())
                    }) {
                        for v in variants {
                            if let TypeExpr::Named { name, .. } = v {
                                variant_of.insert(name.clone(), td.name.name.clone());
                            }
                        }
                    }
                }
            }
        }
        Self { variant_of }
    }

    fn emit_type_def(&self, out: &mut String, td: &TypeDef) {
        if !td.generic_params.is_empty() {
            let _ = writeln!(
                out,
                "// Skipping generic type `{}` for now (TODO).",
                td.name.name
            );
            return;
        }

        match &td.body {
            TypeExpr::Union { variants, .. } if all_simple_named(variants) => {
                let _ = writeln!(out, "#[allow(non_camel_case_types, dead_code)]");
                let _ = writeln!(out, "pub enum {} {{", td.name.name);
                for v in variants {
                    if let TypeExpr::Named { name, .. } = v {
                        let _ = writeln!(out, "    {},", name);
                    }
                }
                let _ = writeln!(out, "}}");
            }
            TypeExpr::Product { fields, .. } if all_simple_named(fields) => {
                let _ = writeln!(out, "#[allow(non_snake_case, dead_code)]");
                let _ = writeln!(out, "pub struct {} {{", td.name.name);
                for f in fields {
                    if let TypeExpr::Named { name, .. } = f {
                        let _ = writeln!(out, "    pub {}: {},", lower_first(name), name);
                    }
                }
                let _ = writeln!(out, "}}");
            }
            TypeExpr::Named { name, generics, .. } => {
                let rendered = render_named_type(name, generics);
                let _ = writeln!(out, "pub type {} = {};", td.name.name, rendered);
            }
            TypeExpr::Repeat { ty, count, .. } => {
                let _ = writeln!(
                    out,
                    "pub type {} = [{}; {}];",
                    td.name.name,
                    render_type(ty),
                    count
                );
            }
            TypeExpr::Spread { ty, .. } => {
                let _ = writeln!(out, "pub type {} = Vec<{}>;", td.name.name, render_type(ty));
            }
            _ => {
                let _ = writeln!(
                    out,
                    "// Skipping complex type `{}` for now (TODO).",
                    td.name.name
                );
            }
        }
    }

    fn emit_function(&self, out: &mut String, func: &FunctionDef) {
        let is_entry = func.receiver.is_none() && func.name.name == "main";
        if is_entry {
            out.push_str("fn main() {\n");
            self.emit_block_body(out, &func.body, /* is_main */ true);
            out.push_str("}\n");
        } else {
            let _ = write!(
                out,
                "fn {}() -> {} {{\n",
                func.name.name,
                render_type(&func.return_ty)
            );
            self.emit_block_body(out, &func.body, false);
            out.push_str("}\n");
        }
    }

    fn emit_block_body(&self, out: &mut String, block: &Block, is_main: bool) {
        let last_idx = block.exprs.len().saturating_sub(1);
        for (i, expr) in block.exprs.iter().enumerate() {
            out.push_str("    ");
            self.emit_expr(out, expr);
            if is_main || i != last_idx {
                out.push(';');
            }
            out.push('\n');
        }
    }

    fn emit_expr(&self, out: &mut String, expr: &Expr) {
        match expr {
            Expr::Ident(ident) => {
                out.push_str(&self.rust_value(&ident.name));
            }
            Expr::StringLit { value, .. } => {
                let _ = write!(out, "{:?}", value);
            }
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => {
                if let Some(rust) = try_emit_builtin_method(receiver, method, args) {
                    out.push_str(&rust);
                } else {
                    self.emit_expr(out, receiver);
                    let _ = write!(out, ".{}(", method.name);
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.emit_expr(out, arg);
                    }
                    out.push(')');
                }
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                out.push_str("match ");
                self.emit_expr(out, scrutinee);
                out.push_str(" {\n");
                for arm in arms {
                    out.push_str("        ");
                    self.emit_pattern(out, &arm.pattern);
                    out.push_str(" => ");
                    self.emit_expr(out, &arm.body);
                    out.push_str(",\n");
                }
                out.push_str("    }");
            }
        }
    }

    fn emit_pattern(&self, out: &mut String, pattern: &Pattern) {
        match pattern {
            Pattern::Variant { name, .. } => {
                out.push_str(&self.rust_value(name));
            }
            Pattern::Wildcard { .. } => {
                out.push('_');
            }
        }
    }

    fn rust_value(&self, name: &str) -> String {
        if name == "Noop" {
            return "()".to_string();
        }
        if let Some(parent) = self.variant_of.get(name) {
            return format!("{}::{}", parent, name);
        }
        name.to_string()
    }
}

fn all_simple_named(items: &[TypeExpr]) -> bool {
    items
        .iter()
        .all(|t| matches!(t, TypeExpr::Named { generics, .. } if generics.is_empty()))
}

fn render_type(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named { name, generics, .. } => render_named_type(name, generics),
        TypeExpr::Repeat { ty, count, .. } => format!("[{}; {}]", render_type(ty), count),
        TypeExpr::Spread { ty, .. } => format!("Vec<{}>", render_type(ty)),
        TypeExpr::Union { .. } | TypeExpr::Product { .. } => "()".to_string(),
    }
}

fn render_named_type(name: &str, generics: &[TypeExpr]) -> String {
    let base = match name {
        "Noop" => "()".to_string(),
        "Int" => "i64".to_string(),
        "Float" => "f64".to_string(),
        "Hex" => "u64".to_string(),
        "Bytes" => "Vec<u8>".to_string(),
        other => other.to_string(),
    };
    if generics.is_empty() {
        base
    } else {
        let inner: Vec<String> = generics.iter().map(render_type).collect();
        format!("{}<{}>", base, inner.join(", "))
    }
}

fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn try_emit_builtin_method(receiver: &Expr, method: &Ident, args: &[Expr]) -> Option<String> {
    if method.name == "print" && args.len() == 1 {
        if let Expr::StringLit { value, .. } = receiver {
            return Some(format!("println!({:?})", value));
        }
    }
    None
}
