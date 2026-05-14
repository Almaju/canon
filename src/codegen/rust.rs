use crate::ast::*;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

const SUSPENDING_CAPABILITIES: &[&str] = &["Filesystem", "Network"];

fn is_suspending_capability(name: &str) -> bool {
    SUSPENDING_CAPABILITIES.contains(&name)
}

const CAPABILITY_TYPES: &[&str] = &[
    "Clock", "Filesystem", "Network", "Random", "Stderr", "Stdin", "Stdout",
];

fn is_capability_type(name: &str) -> bool {
    CAPABILITY_TYPES.contains(&name)
}

fn is_capability_receiver(expr: &Expr) -> bool {
    if let Expr::Ident(ident) = expr {
        return is_capability_type(&ident.name);
    }
    false
}

pub struct GeneratedRust {
    pub source: String,
    pub is_async: bool,
}

pub fn generate(module: &Module) -> String {
    generate_with_meta(module).source
}

pub fn generate_with_meta(module: &Module) -> GeneratedRust {
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

    let is_async = cg.async_free_fns.contains("main");
    GeneratedRust {
        source: out,
        is_async,
    }
}

struct Codegen {
    variant_of: HashMap<String, String>,
    current_receiver: Option<String>,
    extern_methods: HashMap<(String, String), ExternMethod>,
    bool_declared: bool,
    async_methods: HashSet<(String, String)>,
    async_free_fns: HashSet<String>,
    lambda_scopes: std::cell::RefCell<Vec<HashMap<String, String>>>,
}

#[derive(Clone)]
struct ExternMethod {
    path: String,
    is_async: bool,
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
                    if let (Some(recv), Some(extern_decl)) =
                        (&func.receiver, &func.extern_rust)
                    {
                        extern_methods.insert(
                            (recv.name.clone(), func.name.name.clone()),
                            ExternMethod {
                                path: extern_decl.path.clone(),
                                is_async: extern_decl.is_async,
                            },
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

        let (async_methods, async_free_fns) = compute_async_sets(module, &extern_methods);

        Self {
            variant_of,
            current_receiver: None,
            extern_methods,
            bool_declared,
            async_methods,
            async_free_fns,
            lambda_scopes: std::cell::RefCell::new(Vec::new()),
        }
    }

    fn is_async_method(&self, recv_ty: &str, method: &str) -> bool {
        let key = (recv_ty.to_string(), method.to_string());
        if let Some(em) = self.extern_methods.get(&key) {
            if em.is_async {
                return true;
            }
        }
        self.async_methods.contains(&key)
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
                if let Some(rust_path) = name.strip_prefix("__extern__") {
                    let _ = writeln!(out, "pub type {} = {};", td.name.name, rust_path);
                } else {
                    let rendered = render_named_type(name, generics);
                    let _ = writeln!(out, "#[allow(dead_code)]");
                    let _ = writeln!(out, "pub struct {}(pub {});", td.name.name, rendered);
                }
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
        let is_async = self.async_free_fns.contains(&func.name.name);
        let async_kw = if is_async { "async " } else { "" };
        if is_entry {
            if is_async {
                out.push_str("#[tokio::main]\n");
            }
            let ret = render_type(&func.return_ty);
            if ret == "()" {
                let _ = write!(out, "{}fn main() {{\n", async_kw);
                self.emit_block_body(out, &func.body, true);
            } else {
                let _ = write!(out, "{}fn main() -> {} {{\n", async_kw, ret);
                self.emit_block_body(out, &func.body, false);
            }
            out.push_str("}\n");
        } else {
            let _ = write!(
                out,
                "{}fn {}() -> {} {{\n",
                async_kw,
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
        let is_async = self
            .async_methods
            .contains(&(recv.to_string(), func.name.name.clone()));
        let async_kw = if is_async { "async " } else { "" };
        let _ = write!(out, "    pub {}fn {}(&self", async_kw, func.name.name);
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
                } else if name.name == "List" {
                    out.push_str("vec![");
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.emit_expr(out, arg);
                    }
                    out.push(']');
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
                    let recv_ty = static_type_of(receiver);
                    if self.is_async_method(&recv_ty, &method.name) {
                        out.push_str(".await");
                    }
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
            Expr::Lambda {
                params,
                return_ty,
                body,
                ..
            } => {
                let mut scope = HashMap::new();
                out.push('|');
                for (i, param) in params.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    let arg = format!("__a{}", i);
                    let _ = write!(out, "{}: {}", arg, render_type(&param.ty));
                    if let Some(name) = param.ty.simple_name() {
                        scope.insert(name.to_string(), arg);
                    }
                }
                let _ = write!(out, "| -> {} {{ ", render_type(return_ty));
                self.lambda_scopes.borrow_mut().push(scope);
                let last_idx = body.exprs.len().saturating_sub(1);
                for (i, expr) in body.exprs.iter().enumerate() {
                    self.emit_expr(out, expr);
                    if i != last_idx {
                        out.push_str("; ");
                    }
                }
                self.lambda_scopes.borrow_mut().pop();
                out.push_str(" }");
            }
        }
    }

    fn try_emit_builtin_method(
        &self,
        receiver: &Expr,
        method: &Ident,
        args: &[Expr],
    ) -> Option<String> {
        if let Some(extern_method) = self.lookup_extern_method(receiver, &method.name) {
            let mut s = self.emit_extern_call(&extern_method.path, receiver, args);
            if extern_method.is_async {
                s.push_str(".await");
            }
            return Some(s);
        }
        if method.name == "print" && args.len() == 1 {
            let mut s = String::from("println!(\"{}\", ");
            self.emit_expr(&mut s, receiver);
            s.push(')');
            return Some(s);
        }
        if let Some(op) = binary_operator_for(&method.name) {
            if args.len() == 1 {
                let mut s = String::from("(");
                self.emit_expr(&mut s, receiver);
                let _ = write!(s, " {} ", op);
                self.emit_expr(&mut s, &args[0]);
                s.push(')');
                return Some(s);
            }
        }
        if method.name == "not" && args.is_empty() {
            let mut s = String::from("(!");
            self.emit_expr(&mut s, receiver);
            s.push(')');
            return Some(s);
        }
        if method.name == "concat" && args.len() == 1 {
            let mut s = String::from("(");
            self.emit_expr(&mut s, receiver);
            s.push_str(" + &");
            self.emit_expr(&mut s, &args[0]);
            s.push(')');
            return Some(s);
        }
        if static_type_of(receiver) == "List" {
            if method.name == "length" && args.is_empty() {
                let mut s = String::from("(");
                self.emit_expr(&mut s, receiver);
                s.push_str(".len() as i64)");
                return Some(s);
            }
            if method.name == "first" && args.is_empty() {
                let mut s = String::new();
                self.emit_expr(&mut s, receiver);
                s.push_str(".first().cloned()");
                return Some(s);
            }
            if method.name == "map" && args.len() == 1 {
                let mut s = String::new();
                self.emit_expr(&mut s, receiver);
                s.push_str(".into_iter().map(");
                self.emit_expr(&mut s, &args[0]);
                s.push_str(").collect::<Vec<_>>()");
                return Some(s);
            }
        }
        None
    }

    fn lookup_extern_method(&self, receiver: &Expr, method: &str) -> Option<ExternMethod> {
        let recv_ty = static_type_of(receiver);
        self.extern_methods
            .get(&(recv_ty, method.to_string()))
            .cloned()
    }

    fn emit_extern_call(&self, rust_path: &str, receiver: &Expr, args: &[Expr]) -> String {
        if let Some(method) = rust_path.strip_prefix('.') {
            let mut s = String::new();
            self.emit_expr(&mut s, receiver);
            let _ = write!(s, ".{}(", method);
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                self.emit_expr(&mut s, arg);
            }
            s.push(')');
            return s;
        }
        let is_macro = rust_path.ends_with('!');
        let path = rust_path.trim_end_matches('!');
        let mut s = String::new();
        if is_macro {
            let _ = write!(s, "{}!(", path);
        } else {
            let _ = write!(s, "{}(", path);
        }
        let receiver_is_capability = is_capability_receiver(receiver);
        let mut first = true;
        if !receiver_is_capability {
            self.emit_expr(&mut s, receiver);
            first = false;
        }
        for arg in args {
            if !first {
                s.push_str(", ");
            }
            self.emit_expr(&mut s, arg);
            first = false;
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
        for scope in self.lambda_scopes.borrow().iter().rev() {
            if let Some(arg) = scope.get(name) {
                return arg.clone();
            }
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
        "List" => "Vec".to_string(),
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

fn binary_operator_for(method: &str) -> Option<&'static str> {
    match method {
        "add" => Some("+"),
        "sub" => Some("-"),
        "mul" => Some("*"),
        "div" => Some("/"),
        "rem" => Some("%"),
        "eq" => Some("=="),
        "lt" => Some("<"),
        "gt" => Some(">"),
        "lte" => Some("<="),
        "gte" => Some(">="),
        "and" => Some("&&"),
        "or" => Some("||"),
        _ => None,
    }
}

fn static_type_of(expr: &Expr) -> String {
    match expr {
        Expr::StringLit { .. } => "String".to_string(),
        Expr::IntLit { .. } => "Int".to_string(),
        Expr::FloatLit { .. } => "Float".to_string(),
        Expr::HexLit { .. } => "Hex".to_string(),
        Expr::Constructor { name, .. } => name.name.clone(),
        Expr::Ident(ident) => ident.name.clone(),
        Expr::MethodCall {
            receiver, method, ..
        } => {
            let recv_ty = static_type_of(receiver);
            match (recv_ty.as_str(), method.name.as_str()) {
                ("List", "map") => "List".to_string(),
                ("List", "length") => "Int".to_string(),
                ("List", "first") => "Option".to_string(),
                ("Int" | "Float", "add" | "sub" | "mul" | "div" | "rem") => recv_ty,
                ("Int" | "Float", "eq" | "lt" | "gt" | "lte" | "gte") => "Bool".to_string(),
                ("Bool", "not" | "and" | "or") => "Bool".to_string(),
                ("String", "concat") => "String".to_string(),
                _ => "<unknown>".to_string(),
            }
        }
        Expr::Try { inner, .. } => {
            if let Expr::Constructor { name, args, .. } = &**inner {
                if matches!(name.name.as_str(), "Ok" | "Some") && !args.is_empty() {
                    return static_type_of(&args[0]);
                }
            }
            "<unknown>".to_string()
        }
        Expr::Match { .. } | Expr::While { .. } | Expr::Lambda { .. } => "<unknown>".to_string(),
    }
}

fn compute_async_sets(
    module: &Module,
    extern_methods: &HashMap<(String, String), ExternMethod>,
) -> (HashSet<(String, String)>, HashSet<String>) {
    let mut method_bodies: HashMap<(String, String), &Block> = HashMap::new();
    let mut free_bodies: HashMap<String, &Block> = HashMap::new();
    let mut method_params: HashMap<(String, String), &Vec<Param>> = HashMap::new();
    let mut free_params: HashMap<String, &Vec<Param>> = HashMap::new();

    for item in &module.items {
        if let Item::Function(func) = item {
            if func.extern_rust.is_some() {
                continue;
            }
            if let Some(recv) = &func.receiver {
                let key = (recv.name.clone(), func.name.name.clone());
                method_bodies.insert(key.clone(), &func.body);
                method_params.insert(key, &func.params);
            } else {
                free_bodies.insert(func.name.name.clone(), &func.body);
                free_params.insert(func.name.name.clone(), &func.params);
            }
        }
    }

    let mut async_methods: HashSet<(String, String)> = HashSet::new();
    let mut async_free_fns: HashSet<String> = HashSet::new();

    for (key, params) in &method_params {
        if has_suspending_param(params) {
            async_methods.insert(key.clone());
        } else if body_calls_async_extern(method_bodies[key], extern_methods) {
            async_methods.insert(key.clone());
        }
    }
    for (name, params) in &free_params {
        if has_suspending_param(params) {
            async_free_fns.insert(name.clone());
        } else if body_calls_async_extern(free_bodies[name], extern_methods) {
            async_free_fns.insert(name.clone());
        }
    }

    loop {
        let mut changed = false;
        for (key, body) in &method_bodies {
            if async_methods.contains(key) {
                continue;
            }
            if body_calls_async_oneway(body, &async_methods) {
                async_methods.insert(key.clone());
                changed = true;
            }
        }
        for (name, body) in &free_bodies {
            if async_free_fns.contains(name) {
                continue;
            }
            if body_calls_async_oneway(body, &async_methods) {
                async_free_fns.insert(name.clone());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    (async_methods, async_free_fns)
}

fn has_suspending_param(params: &[Param]) -> bool {
    params.iter().any(|p| {
        p.ty.simple_name()
            .map(is_suspending_capability)
            .unwrap_or(false)
    })
}

fn body_calls_async_extern(
    body: &Block,
    extern_methods: &HashMap<(String, String), ExternMethod>,
) -> bool {
    body.exprs
        .iter()
        .any(|e| expr_calls_async_extern(e, extern_methods))
}

fn expr_calls_async_extern(
    expr: &Expr,
    extern_methods: &HashMap<(String, String), ExternMethod>,
) -> bool {
    match expr {
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            let recv_ty = static_type_of(receiver);
            let key = (recv_ty, method.name.clone());
            if let Some(em) = extern_methods.get(&key) {
                if em.is_async {
                    return true;
                }
            }
            if expr_calls_async_extern(receiver, extern_methods) {
                return true;
            }
            args.iter().any(|a| expr_calls_async_extern(a, extern_methods))
        }
        Expr::Constructor { args, .. } => args
            .iter()
            .any(|a| expr_calls_async_extern(a, extern_methods)),
        Expr::Match { scrutinee, arms, .. } => {
            if expr_calls_async_extern(scrutinee, extern_methods) {
                return true;
            }
            arms.iter()
                .any(|arm| expr_calls_async_extern(&arm.body, extern_methods))
        }
        Expr::Try { inner, .. } => expr_calls_async_extern(inner, extern_methods),
        Expr::While { cond, body, .. } => {
            if expr_calls_async_extern(cond, extern_methods) {
                return true;
            }
            body.exprs
                .iter()
                .any(|e| expr_calls_async_extern(e, extern_methods))
        }
        Expr::Lambda { body, .. } => body
            .exprs
            .iter()
            .any(|e| expr_calls_async_extern(e, extern_methods)),
        _ => false,
    }
}

fn body_calls_async_oneway(
    body: &Block,
    async_methods: &HashSet<(String, String)>,
) -> bool {
    body.exprs
        .iter()
        .any(|e| expr_calls_async_oneway(e, async_methods))
}

fn expr_calls_async_oneway(
    expr: &Expr,
    async_methods: &HashSet<(String, String)>,
) -> bool {
    match expr {
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            let recv_ty = static_type_of(receiver);
            let key = (recv_ty, method.name.clone());
            if async_methods.contains(&key) {
                return true;
            }
            if expr_calls_async_oneway(receiver, async_methods) {
                return true;
            }
            args.iter().any(|a| expr_calls_async_oneway(a, async_methods))
        }
        Expr::Constructor { args, .. } => {
            args.iter().any(|a| expr_calls_async_oneway(a, async_methods))
        }
        Expr::Match { scrutinee, arms, .. } => {
            if expr_calls_async_oneway(scrutinee, async_methods) {
                return true;
            }
            arms.iter()
                .any(|arm| expr_calls_async_oneway(&arm.body, async_methods))
        }
        Expr::Try { inner, .. } => expr_calls_async_oneway(inner, async_methods),
        Expr::While { cond, body, .. } => {
            if expr_calls_async_oneway(cond, async_methods) {
                return true;
            }
            body.exprs
                .iter()
                .any(|e| expr_calls_async_oneway(e, async_methods))
        }
        Expr::Lambda { body, .. } => body
            .exprs
            .iter()
            .any(|e| expr_calls_async_oneway(e, async_methods)),
        _ => false,
    }
}
