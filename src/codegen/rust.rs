use crate::ast::*;
use std::collections::HashMap;
use std::fmt::Write;

pub fn generate(module: &Module) -> String {
    let mut cg = Codegen::from_module(module);
    let mut out = String::new();

    // Pass 0: emit `use` imports (commented for now — module system Phase 12+)
    for item in &module.items {
        if let Item::Use(u) = item {
            let _ = writeln!(out, "// use {}", u.name.name);
        }
    }

    // Pass 1: emit type definitions
    for item in &module.items {
        if let Item::TypeDef(td) = item {
            cg.emit_type_def(&mut out, td);
            out.push('\n');
        }
    }

    // Pass 2: group methods by receiver type and emit impl blocks
    let mut methods_by_receiver: HashMap<String, Vec<&FunctionDef>> = HashMap::new();
    let mut free_functions: Vec<&FunctionDef> = Vec::new();
    for item in &module.items {
        if let Item::Function(func) = item {
            if func.extern_rust.is_some() {
                continue;
            }
            if let Some(recv) = &func.receiver {
                methods_by_receiver
                    .entry(recv.name.clone())
                    .or_default()
                    .push(func);
            } else {
                free_functions.push(func);
            }
        }
    }

    let mut receivers: Vec<&String> = methods_by_receiver.keys().collect();
    receivers.sort();
    for recv in receivers {
        let methods = methods_by_receiver.get(recv).unwrap();
        let _ = writeln!(out, "impl {} {{", recv);
        for func in methods {
            cg.emit_method(&mut out, recv, func);
            out.push('\n');
        }
        let _ = writeln!(out, "}}");
        out.push('\n');
    }

    // Pass 3: emit free functions (e.g. main)
    for func in &free_functions {
        cg.emit_function(&mut out, func);
        out.push('\n');
    }

    out
}

struct Codegen {
    variant_of: HashMap<String, String>,
    current_receiver: Option<String>,
    extern_methods: HashMap<(String, String), String>,
    bool_declared: bool,
}

impl Codegen {
    fn from_module(module: &Module) -> Self {
        let mut variant_of = HashMap::new();
        let mut extern_methods = HashMap::new();
        for item in &module.items {
            match item {
                Item::TypeDef(td) => {
                    if let TypeExpr::Union { variants, .. } = &td.body {
                        if variants.iter().all(|t| {
                            matches!(
                                t,
                                TypeExpr::Named { generics, .. } if generics.is_empty()
                            )
                        }) {
                            for v in variants {
                                if let TypeExpr::Named { name, .. } = v {
                                    variant_of.insert(name.clone(), td.name.name.clone());
                                }
                            }
                        }
                    }
                }
                Item::Function(func) => {
                    if let (Some(recv), Some(rust_path)) =
                        (&func.receiver, &func.extern_rust)
                    {
                        extern_methods.insert(
                            (recv.name.clone(), func.name.name.clone()),
                            rust_path.clone(),
                        );
                    }
                }
                Item::Use(_) => {}
            }
        }
        let bool_declared = module.items.iter().any(|item| {
            if let Item::TypeDef(td) = item {
                if td.name.name == "Bool" {
                    if let TypeExpr::Union { variants, .. } = &td.body {
                        let names: Vec<&str> = variants
                            .iter()
                            .filter_map(|v| {
                                if let TypeExpr::Named { name, .. } = v {
                                    Some(name.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect();
                        return names.contains(&"False") && names.contains(&"True");
                    }
                }
            }
            false
        });

        Self {
            variant_of,
            current_receiver: None,
            extern_methods,
            bool_declared,
        }
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

        if td.name.name == "Bool" && self.bool_declared {
            let _ = writeln!(out, "// Bool is mapped to Rust's `bool` primitive.");
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
                let _ = writeln!(out, "#[allow(dead_code)]");
                let _ = writeln!(out, "pub struct {}(pub {});", td.name.name, rendered);
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

    fn emit_function(&mut self, out: &mut String, func: &FunctionDef) {
        let is_entry = func.receiver.is_none() && func.name.name == "main";
        self.current_receiver = None;
        if is_entry {
            let ret = render_type(&func.return_ty);
            if ret == "()" {
                out.push_str("fn main() {\n");
                self.emit_block_body(out, &func.body, true);
            } else {
                let _ = write!(out, "fn main() -> {} {{\n", ret);
                self.emit_block_body(out, &func.body, false);
            }
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

    fn emit_method(&mut self, out: &mut String, recv: &str, func: &FunctionDef) {
        self.current_receiver = Some(recv.to_string());
        let ret = render_type(&func.return_ty);
        let pascal = is_pascal_case(&func.name.name);
        if pascal {
            let _ = writeln!(out, "    #[allow(non_snake_case)]");
        }
        let _ = write!(out, "    pub fn {}(&self", func.name.name);
        for (i, param) in func.params.iter().enumerate() {
            let _ = write!(out, ", arg{}: {}", i, render_type(&param.ty));
        }
        let _ = write!(out, ") -> {} {{\n", ret);
        self.emit_block_body_indented(out, &func.body, false, 2);
        out.push_str("    }\n");
        self.current_receiver = None;
    }

    fn emit_block_body(&self, out: &mut String, block: &Block, main_unit: bool) {
        self.emit_block_body_indented(out, block, main_unit, 1);
    }

    fn emit_block_body_indented(
        &self,
        out: &mut String,
        block: &Block,
        main_unit: bool,
        indent: usize,
    ) {
        let pad: String = std::iter::repeat("    ").take(indent).collect();
        let last_idx = block.exprs.len().saturating_sub(1);
        for (i, expr) in block.exprs.iter().enumerate() {
            out.push_str(&pad);
            self.emit_expr(out, expr);
            if main_unit || i != last_idx {
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
                let _ = write!(out, "{:?}.to_string()", value);
            }
            Expr::IntLit { value, .. } => {
                let _ = write!(out, "{}i64", value);
            }
            Expr::FloatLit { value, .. } => {
                let _ = write!(out, "{}f64", value);
            }
            Expr::HexLit { value, .. } => {
                let _ = write!(out, "0x{:X}u64", value);
            }
            Expr::Constructor { name, args, .. } => {
                if is_primitive_constructor(&name.name) && args.len() == 1 {
                    self.emit_expr(out, &args[0]);
                } else if is_stdlib_variant(&name.name) {
                    if args.is_empty() {
                        out.push_str(&name.name);
                    } else {
                        let _ = write!(out, "{}(", name.name);
                        for (i, arg) in args.iter().enumerate() {
                            if i > 0 {
                                out.push_str(", ");
                            }
                            self.emit_expr(out, arg);
                        }
                        out.push(')');
                    }
                } else {
                    let _ = write!(out, "{}(", name.name);
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.emit_expr(out, arg);
                    }
                    out.push(')');
                }
            }
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => {
                if let Some(rust) = self.try_emit_builtin_method(receiver, method, args) {
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
            Expr::Try { inner, .. } => {
                self.emit_expr(out, inner);
                out.push('?');
            }
            Expr::While { cond, body, .. } => {
                out.push_str("while ");
                self.emit_expr(out, cond);
                out.push_str(" {\n");
                for expr in &body.exprs {
                    out.push_str("        ");
                    self.emit_expr(out, expr);
                    out.push_str(";\n");
                }
                out.push_str("    }");
            }
        }
    }

    fn try_emit_builtin_method(
        &self,
        receiver: &Expr,
        method: &Ident,
        args: &[Expr],
    ) -> Option<String> {
        if let Some(rust_path) = self.lookup_extern_method(receiver, &method.name) {
            return Some(self.emit_extern_call(&rust_path, receiver, args));
        }
        if method.name == "print" && args.len() == 1 {
            let mut s = String::from("println!(\"{}\", ");
            self.emit_expr(&mut s, receiver);
            s.push(')');
            return Some(s);
        }
        None
    }

    fn lookup_extern_method(&self, receiver: &Expr, method: &str) -> Option<String> {
        let recv_ty = static_type_of(receiver);
        self.extern_methods
            .get(&(recv_ty, method.to_string()))
            .cloned()
    }

    fn emit_extern_call(&self, rust_path: &str, receiver: &Expr, args: &[Expr]) -> String {
        let is_macro = rust_path.ends_with('!');
        let path = rust_path.trim_end_matches('!');
        let mut s = String::new();
        if is_macro {
            let _ = write!(s, "{}!(", path);
        } else {
            let _ = write!(s, "{}(", path);
        }
        self.emit_expr(&mut s, receiver);
        for arg in args {
            s.push_str(", ");
            self.emit_expr(&mut s, arg);
        }
        s.push(')');
        s
    }

    fn emit_pattern(&self, out: &mut String, pattern: &Pattern) {
        match pattern {
            Pattern::Variant { name, args, .. } => {
                if is_stdlib_variant(name) {
                    out.push_str(name);
                } else {
                    out.push_str(&self.rust_value(name));
                }
                if !args.is_empty() {
                    out.push('(');
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.emit_pattern(out, arg);
                    }
                    out.push(')');
                }
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
        if name == "Self" {
            return "self".to_string();
        }
        if let Some(current) = &self.current_receiver {
            if name == current {
                return "self".to_string();
            }
        }
        if self.bool_declared {
            if name == "True" {
                return "true".to_string();
            }
            if name == "False" {
                return "false".to_string();
            }
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
        TypeExpr::Function {
            params, return_ty, ..
        } => {
            let ps: Vec<String> = params.iter().map(render_type).collect();
            format!("fn({}) -> {}", ps.join(", "), render_type(return_ty))
        }
    }
}

fn render_named_type(name: &str, generics: &[TypeExpr]) -> String {
    let base = match name {
        "Noop" => "()".to_string(),
        "Int" => "i64".to_string(),
        "Float" => "f64".to_string(),
        "Hex" => "u64".to_string(),
        "Bytes" => "Vec<u8>".to_string(),
        "String" => "String".to_string(),
        "Bool" => "bool".to_string(),
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

fn is_primitive_constructor(name: &str) -> bool {
    matches!(name, "Int" | "Float" | "Hex" | "String")
}

fn is_stdlib_variant(name: &str) -> bool {
    matches!(name, "None" | "Some" | "Ok" | "Err")
}

fn is_pascal_case(name: &str) -> bool {
    name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
}

fn static_type_of(expr: &Expr) -> String {
    match expr {
        Expr::StringLit { .. } => "String".to_string(),
        Expr::IntLit { .. } => "Int".to_string(),
        Expr::FloatLit { .. } => "Float".to_string(),
        Expr::HexLit { .. } => "Hex".to_string(),
        Expr::Constructor { name, .. } => name.name.clone(),
        Expr::Ident(ident) => ident.name.clone(),
        Expr::MethodCall { .. } | Expr::Match { .. } | Expr::Try { .. } | Expr::While { .. } => {
            "<unknown>".to_string()
        }
    }
}
