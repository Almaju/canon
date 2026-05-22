use crate::ast::*;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

const SUSPENDING_CAPABILITIES: &[&str] = &["Filesystem", "HttpClient", "Network"];

fn is_suspending_capability(name: &str) -> bool {
    SUSPENDING_CAPABILITIES.contains(&name)
}

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
    /// True when the currently-emitted method has a `mut` receiver, so a
    /// reference to the receiver name in the body resolves to plain `self`
    /// (not the cloned form used for immutable receivers).
    receiver_mut_in_scope: bool,
    extern_methods: HashMap<(String, String), ExternMethod>,
    bool_declared: bool,
    async_methods: HashSet<(String, String)>,
    async_free_fns: HashSet<String>,
    types_with_self: HashSet<String>,
    lambda_scopes: std::cell::RefCell<Vec<HashMap<String, String>>>,
    commutative_methods: HashMap<(String, String), String>,
    /// For each product TypeDef `T = A * B * ...`, the ordered list of
    /// component type names. Used to emit struct-literal constructors.
    product_components: HashMap<String, Vec<String>>,
    /// Names of TypeDefs declared in this module. Used to decide whether
    /// a union variant carries a payload (variant name == typedef name).
    known_typedefs: HashSet<String>,
    /// (union, variant) pairs where the variant payload must be `Box<…>`
    /// to break a structural cycle.
    boxed_variants: HashSet<(String, String)>,
}

#[derive(Clone)]
struct ExternMethod {
    path: String,
    is_async: bool,
    return_ty: TypeExpr,
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
                    if let (Some(recv), Some(extern_decl)) = (&func.receiver, &func.extern_rust) {
                        extern_methods.insert(
                            (recv.name.clone(), func.name.name.clone()),
                            ExternMethod {
                                path: extern_decl.path.clone(),
                                is_async: extern_decl.is_async,
                                return_ty: func.return_ty.clone(),
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

        let types_with_self: HashSet<String> = module
            .items
            .iter()
            .filter_map(|item| {
                if let Item::Function(func) = item {
                    if func.name.name == "Self" {
                        return func.receiver.as_ref().map(|r| r.name.clone());
                    }
                }
                None
            })
            .collect();

        let mut commutative_methods: HashMap<(String, String), String> = HashMap::new();
        for item in &module.items {
            if let Item::Function(func) = item {
                if let Some(recv) = &func.receiver {
                    if func.name.name != "Self" {
                        for param in &func.params {
                            if let Some(param_name) = param.ty.simple_name() {
                                commutative_methods.insert(
                                    (param_name.to_string(), func.name.name.clone()),
                                    recv.name.clone(),
                                );
                            }
                        }
                    }
                }
            }
        }

        let mut product_components: HashMap<String, Vec<String>> = HashMap::new();
        let mut typedef_index: HashMap<String, &TypeDef> = HashMap::new();
        let mut known_typedefs: HashSet<String> = HashSet::new();
        for item in &module.items {
            if let Item::TypeDef(td) = item {
                typedef_index.insert(td.name.name.clone(), td);
                known_typedefs.insert(td.name.name.clone());
                if let TypeExpr::Product { fields, .. } = &td.body {
                    if all_simple_named(fields) {
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
                        product_components.insert(td.name.name.clone(), names);
                    }
                }
            }
        }

        let mut boxed_variants: HashSet<(String, String)> = HashSet::new();
        for (union_name, union_td) in &typedef_index {
            if let TypeExpr::Union { variants, .. } = &union_td.body {
                for v in variants {
                    if let TypeExpr::Named {
                        name: variant_name, ..
                    } = v
                    {
                        if typedef_index.contains_key(variant_name)
                            && reaches_target(variant_name, union_name, &typedef_index)
                        {
                            boxed_variants
                                .insert((union_name.clone(), variant_name.clone()));
                        }
                    }
                }
            }
        }

        Self {
            variant_of,
            current_receiver: None,
            receiver_mut_in_scope: false,
            extern_methods,
            bool_declared,
            async_methods,
            async_free_fns,
            types_with_self,
            lambda_scopes: std::cell::RefCell::new(Vec::new()),
            commutative_methods,
            product_components,
            known_typedefs,
            boxed_variants,
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
        if td.name.name == "Bool" && self.bool_declared {
            let _ = writeln!(out, "// Bool is mapped to Rust's `bool` primitive.");
            return;
        }

        let generic_str = render_generic_params(&td.generic_params);

        match &td.body {
            TypeExpr::Union { variants, .. } if all_simple_named(variants) => {
                let _ = writeln!(out, "#[derive(Clone)]");
                let _ = writeln!(out, "#[allow(non_camel_case_types, dead_code)]");
                let _ = writeln!(out, "pub enum {}{} {{", td.name.name, generic_str);
                for v in variants {
                    if let TypeExpr::Named { name, .. } = v {
                        if self.known_typedefs.contains(name) {
                            let boxed = self
                                .boxed_variants
                                .contains(&(td.name.name.clone(), name.clone()));
                            if boxed {
                                let _ = writeln!(out, "    {}(Box<{}>),", name, name);
                            } else {
                                let _ = writeln!(out, "    {}({}),", name, name);
                            }
                        } else {
                            let _ = writeln!(out, "    {},", name);
                        }
                    }
                }
                let _ = writeln!(out, "}}");
            }
            TypeExpr::Product { fields, .. } if all_simple_named(fields) => {
                let _ = writeln!(out, "#[derive(Clone)]");
                let _ = writeln!(out, "#[allow(non_snake_case, dead_code)]");
                let _ = writeln!(out, "pub struct {}{} {{", td.name.name, generic_str);
                for f in fields {
                    if let TypeExpr::Named { name, .. } = f {
                        let _ = writeln!(out, "    pub {}: {},", lower_first(name), name);
                    }
                }
                let _ = writeln!(out, "}}");
            }
            TypeExpr::Named { name, generics, .. } => {
                if let Some(rust_path) = name.strip_prefix("__extern__") {
                    let _ = writeln!(
                        out,
                        "pub type {}{} = {};",
                        td.name.name, generic_str, rust_path
                    );
                } else {
                    let rendered = render_named_type(name, generics);
                    let field_vis = if self.types_with_self.contains(&td.name.name) {
                        ""
                    } else {
                        "pub "
                    };
                    let _ = writeln!(out, "#[derive(Clone)]");
                    let _ = writeln!(out, "#[allow(dead_code)]");
                    let _ = writeln!(
                        out,
                        "pub struct {}{}({}{});",
                        td.name.name, generic_str, field_vis, rendered
                    );
                }
            }
            TypeExpr::Repeat { ty, count, .. } => {
                let _ = writeln!(
                    out,
                    "pub type {}{} = [{}; {}];",
                    td.name.name,
                    generic_str,
                    render_type(ty),
                    count
                );
            }
            TypeExpr::Spread { ty, .. } => {
                let _ = writeln!(
                    out,
                    "pub type {}{} = Vec<{}>;",
                    td.name.name,
                    generic_str,
                    render_type(ty)
                );
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
        self.receiver_mut_in_scope = func.receiver_mut;
        let mut param_scope = HashMap::new();
        for (i, param) in func.params.iter().enumerate() {
            if let Some(name) = param.ty.simple_name() {
                param_scope.insert(name.to_string(), format!("arg{}", i));
            }
        }
        self.lambda_scopes.borrow_mut().push(param_scope);
        let ret = render_type(&func.return_ty);
        let pascal = is_pascal_case(&func.name.name);
        if pascal {
            let _ = writeln!(out, "    #[allow(non_snake_case)]");
        }
        let is_async = self
            .async_methods
            .contains(&(recv.to_string(), func.name.name.clone()));
        let async_kw = if is_async { "async " } else { "" };
        let generic_str = render_generic_params(&func.generic_params);
        let is_self_ctor = func.name.name == "Self";
        let emitted_name = if is_self_ctor {
            "r#Self"
        } else {
            func.name.name.as_str()
        };
        let _ = write!(
            out,
            "    pub {}fn {}{}(",
            async_kw, emitted_name, generic_str
        );
        let mut first = true;
        if !is_self_ctor {
            if func.receiver_mut {
                out.push_str("&mut self");
            } else {
                out.push_str("&self");
            }
            first = false;
        }
        for (i, param) in func.params.iter().enumerate() {
            if !first {
                out.push_str(", ");
            }
            let ty = render_type(&param.ty);
            if param.mutable {
                let _ = write!(out, "arg{}: &mut {}", i, ty);
            } else {
                let _ = write!(out, "arg{}: {}", i, ty);
            }
            first = false;
        }
        let _ = write!(out, ") -> {} {{\n", ret);
        self.emit_block_body_indented(out, &func.body, false, 2);
        out.push_str("    }\n");
        self.lambda_scopes.borrow_mut().pop();
        self.current_receiver = None;
        self.receiver_mut_in_scope = false;
    }

    /// Emit the inner payload of a union-variant constructor. The variant's
    /// own typedef determines the form: a product → struct literal with
    /// named fields; anything else → tuple-struct call.
    fn emit_payload_constructor(&self, out: &mut String, variant_name: &str, args: &[&Expr]) {
        if let Some(components) = self.product_components.get(variant_name) {
            let _ = write!(out, "{} {{ ", variant_name);
            for (i, (comp, arg)) in components.iter().zip(args.iter()).enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let _ = write!(out, "{}: ", lower_first(comp));
                self.emit_expr(out, arg);
            }
            out.push_str(" }");
        } else {
            let _ = write!(out, "{}(", variant_name);
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                self.emit_expr(out, arg);
            }
            out.push(')');
        }
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
                let flat = flatten_product_args(args);
                // User union variant with its own payload typedef:
                // emit `Parent::Variant(payload)`, boxing if the variant is
                // on a structural cycle.
                if !is_stdlib_variant(&name.name)
                    && self.known_typedefs.contains(&name.name)
                {
                    if let Some(parent) = self.variant_of.get(&name.name).cloned() {
                        let boxed = self
                            .boxed_variants
                            .contains(&(parent.clone(), name.name.clone()));
                        let _ = write!(out, "{}::{}(", parent, name.name);
                        if boxed {
                            out.push_str("Box::new(");
                        }
                        self.emit_payload_constructor(out, &name.name, &flat);
                        if boxed {
                            out.push(')');
                        }
                        out.push(')');
                        return;
                    }
                }
                if is_primitive_constructor(&name.name) && flat.len() == 1 {
                    self.emit_expr(out, flat[0]);
                } else if name.name == "List" {
                    out.push_str("vec![");
                    for (i, arg) in flat.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.emit_expr(out, arg);
                    }
                    out.push(']');
                } else if is_stdlib_variant(&name.name) {
                    if flat.is_empty() {
                        out.push_str(&name.name);
                    } else {
                        let _ = write!(out, "{}(", name.name);
                        for (i, arg) in flat.iter().enumerate() {
                            if i > 0 {
                                out.push_str(", ");
                            }
                            self.emit_expr(out, arg);
                        }
                        out.push(')');
                    }
                } else if let Some(components) = self.product_components.get(&name.name) {
                    // T = A * B * C → struct literal with named fields.
                    let _ = write!(out, "{} {{ ", name.name);
                    for (i, (comp_name, arg)) in components.iter().zip(flat.iter()).enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        let _ = write!(out, "{}: ", lower_first(comp_name));
                        self.emit_expr(out, arg);
                    }
                    out.push_str(" }");
                } else if self.types_with_self.contains(&name.name) {
                    let key = (name.name.clone(), "Self".to_string());
                    if let Some(em) = self.extern_methods.get(&key) {
                        let path = em.path.clone();
                        let is_async = em.is_async;
                        let _ = write!(out, "{}(", path);
                        for (i, arg) in flat.iter().enumerate() {
                            if i > 0 {
                                out.push_str(", ");
                            }
                            self.emit_expr(out, arg);
                        }
                        out.push(')');
                        if is_async {
                            out.push_str(".await");
                        }
                    } else {
                        let _ = write!(out, "{}::r#Self(", name.name);
                        for (i, arg) in flat.iter().enumerate() {
                            if i > 0 {
                                out.push_str(", ");
                            }
                            self.emit_expr(out, arg);
                        }
                        out.push(')');
                    }
                } else {
                    let _ = write!(out, "{}(", name.name);
                    for (i, arg) in flat.iter().enumerate() {
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
                type_args,
                args,
                ..
            } => {
                if let Some(rust) = self.try_emit_builtin_method(receiver, method, type_args, args)
                {
                    out.push_str(&rust);
                } else {
                    let recv_ty = static_type_of_with(receiver, Some(&self.extern_methods));
                    let canonical = self
                        .commutative_methods
                        .get(&(recv_ty.clone(), method.name.clone()))
                        .cloned();
                    let needs_swap = canonical
                        .as_ref()
                        .map(|c| c != &recv_ty && args.len() == 1)
                        .unwrap_or(false);

                    let flat = flatten_product_args(args);
                    if needs_swap {
                        // Commutative swap: emit args[0].method(receiver)
                        let canonical_recv = canonical.unwrap();
                        self.emit_expr(out, flat[0]);
                        let _ = write!(out, ".{}", method.name);
                        if !type_args.is_empty() {
                            out.push_str("::");
                            out.push_str(&render_type_args(type_args));
                        }
                        out.push('(');
                        self.emit_expr(out, receiver);
                        out.push(')');
                        if self.is_async_method(&canonical_recv, &method.name) {
                            out.push_str(".await");
                        }
                    } else {
                        self.emit_expr(out, receiver);
                        let _ = write!(out, ".{}", method.name);
                        if !type_args.is_empty() {
                            out.push_str("::");
                            out.push_str(&render_type_args(type_args));
                        }
                        out.push('(');
                        for (i, arg) in flat.iter().enumerate() {
                            if i > 0 {
                                out.push_str(", ");
                            }
                            self.emit_expr(out, arg);
                        }
                        out.push(')');
                        if self.is_async_method(&recv_ty, &method.name) {
                            out.push_str(".await");
                        }
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

            Expr::ProductValue { fields, .. } => {
                // A ProductValue should normally be unwrapped by the caller
                // (constructor / method call). If we hit one here it's the rare
                // standalone case — emit a tuple, which is the closest Rust
                // analogue.
                out.push('(');
                for (i, f) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    self.emit_expr(out, f);
                }
                out.push(')');
            }
            Expr::FieldAccess { receiver, field, .. } => {
                self.emit_expr(out, receiver);
                let _ = write!(out, ".{}", lower_first(&field.name));
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
        type_args: &[TypeExpr],
        args: &[Expr],
    ) -> Option<String> {
        if let Some(extern_method) = self.lookup_extern_method(receiver, &method.name) {
            let mut s = self.emit_extern_call(&extern_method.path, receiver, type_args, args);
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
        if static_type_of_with(receiver, Some(&self.extern_methods)) == "List" {
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
        let recv_ty = static_type_of_with(receiver, Some(&self.extern_methods));
        self.extern_methods
            .get(&(recv_ty, method.to_string()))
            .cloned()
    }

    fn emit_extern_call(
        &self,
        rust_path: &str,
        receiver: &Expr,
        type_args: &[TypeExpr],
        args: &[Expr],
    ) -> String {
        let turbofish = if type_args.is_empty() {
            String::new()
        } else {
            format!("::{}", render_type_args(type_args))
        };
        if let Some(method) = rust_path.strip_prefix('.') {
            let mut s = String::new();
            self.emit_expr(&mut s, receiver);
            let _ = write!(s, ".{}{}(", method, turbofish);
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
            let _ = write!(s, "{}!{}(", path, turbofish);
        } else {
            let _ = write!(s, "{}{}(", path, turbofish);
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
                if self.bool_declared && (name == "True" || name == "False") {
                    out.push_str(if name == "True" { "true" } else { "false" });
                } else if is_stdlib_variant(name) {
                    out.push_str(name);
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
                } else if let Some(parent) = self.variant_of.get(name).cloned() {
                    // User union variant. If the variant carries a payload
                    // (typedef of the same name), patterns destructure that
                    // payload — but with no payload args, write `Parent::V(_)`
                    // so the match arm still binds.
                    if self.known_typedefs.contains(name) {
                        if args.is_empty() {
                            let _ = write!(out, "{}::{}(_)", parent, name);
                        } else {
                            let _ = write!(out, "{}::{}(", parent, name);
                            for (i, arg) in args.iter().enumerate() {
                                if i > 0 {
                                    out.push_str(", ");
                                }
                                self.emit_pattern(out, arg);
                            }
                            out.push(')');
                        }
                    } else {
                        // No payload — emit the bare variant path.
                        let _ = write!(out, "{}::{}", parent, name);
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
                } else {
                    out.push_str(&self.rust_value(name));
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
            }
            Pattern::Wildcard { .. } => {
                out.push('_');
            }
        }
    }

    fn rust_value(&self, name: &str) -> String {
        if name == "Unit" {
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
                return if self.receiver_mut_in_scope {
                    "self".to_string()
                } else {
                    "self.clone()".to_string()
                };
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

/// If `args` is exactly `[ProductValue { fields }]`, treat the product's
/// fields as the call's positional arguments. Otherwise pass args through.
fn flatten_product_args(args: &[Expr]) -> Vec<&Expr> {
    if args.len() == 1 {
        if let Expr::ProductValue { fields, .. } = &args[0] {
            return fields.iter().collect();
        }
    }
    args.iter().collect()
}

/// Names of typedefs structurally referenced by `ty` (Named refs, recursing
/// into compound types).
fn collect_referenced_types(ty: &TypeExpr, out: &mut Vec<String>) {
    match ty {
        TypeExpr::Named { name, generics, .. } => {
            out.push(name.clone());
            for g in generics {
                collect_referenced_types(g, out);
            }
        }
        TypeExpr::Union { variants, .. } => {
            for v in variants {
                collect_referenced_types(v, out);
            }
        }
        TypeExpr::Product { fields, .. } => {
            for f in fields {
                collect_referenced_types(f, out);
            }
        }
        TypeExpr::Repeat { ty, .. } | TypeExpr::Spread { ty, .. } => {
            collect_referenced_types(ty, out);
        }
        TypeExpr::Function { .. } => {}
    }
}

/// True if `target` is transitively reachable from `start` by following
/// structural Named references through the module's typedefs. The start
/// node itself is not counted as a reach; we want true cycles.
fn reaches_target(
    start: &str,
    target: &str,
    typedefs: &HashMap<String, &TypeDef>,
) -> bool {
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(start.to_string());
    let mut stack: Vec<String> = Vec::new();
    if let Some(td) = typedefs.get(start) {
        let mut refs = Vec::new();
        collect_referenced_types(&td.body, &mut refs);
        stack.extend(refs);
    }
    while let Some(current) = stack.pop() {
        if current == target {
            return true;
        }
        if !visited.insert(current.clone()) {
            continue;
        }
        if let Some(td) = typedefs.get(&current) {
            let mut refs = Vec::new();
            collect_referenced_types(&td.body, &mut refs);
            stack.extend(refs);
        }
    }
    false
}

fn all_simple_named(items: &[TypeExpr]) -> bool {
    items
        .iter()
        .all(|t| matches!(t, TypeExpr::Named { generics, .. } if generics.is_empty()))
}

fn render_generic_params(params: &[GenericParam]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = params
        .iter()
        .map(|p| match &p.bound {
            Some(TypeExpr::Named { name, .. }) => {
                format!("{}: {}", p.name.name, render_trait_bound(name))
            }
            _ => p.name.name.clone(),
        })
        .collect();
    format!("<{}>", parts.join(", "))
}

fn render_trait_bound(name: &str) -> String {
    match name {
        "Deserialize" => "serde::de::DeserializeOwned".to_string(),
        "Serialize" => "serde::Serialize".to_string(),
        other => other.to_string(),
    }
}

fn render_type_args(type_args: &[TypeExpr]) -> String {
    if type_args.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = type_args.iter().map(render_type).collect();
    format!("<{}>", parts.join(", "))
}

fn render_type(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named { name, generics, .. } => render_named_type(name, generics),
        TypeExpr::Repeat { ty, count, .. } => format!("[{}; {}]", render_type(ty), count),
        TypeExpr::Spread { ty, .. } => format!("Vec<{}>", render_type(ty)),
        TypeExpr::Union { .. } => "Box<dyn std::error::Error + Send + Sync>".to_string(),
        TypeExpr::Product { .. } => "()".to_string(),
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
        "Unit" => "()".to_string(),
        "Never" => "std::convert::Infallible".to_string(),
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
    name.chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
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

fn static_type_of_with(
    expr: &Expr,
    extern_methods: Option<&HashMap<(String, String), ExternMethod>>,
) -> String {
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
            let recv_ty = static_type_of_with(receiver, extern_methods);
            let builtin = match (recv_ty.as_str(), method.name.as_str()) {
                ("List", "map") => Some("List".to_string()),
                ("List", "length") => Some("Int".to_string()),
                ("List", "first") => Some("Option".to_string()),
                ("Int" | "Float", "add" | "sub" | "mul" | "div" | "rem") => Some(recv_ty.clone()),
                ("Int" | "Float", "eq" | "lt" | "gt" | "lte" | "gte") => Some("Bool".to_string()),
                ("Bool", "not" | "and" | "or") => Some("Bool".to_string()),
                ("String", "concat") => Some("String".to_string()),
                _ => None,
            };
            if let Some(ty) = builtin {
                return ty;
            }
            if let Some(em) = extern_methods {
                if let Some(method_info) = em.get(&(recv_ty.clone(), method.name.clone())) {
                    if let Some(name) = method_info.return_ty.simple_name() {
                        return name.to_string();
                    }
                }
            }
            "<unknown>".to_string()
        }
        Expr::Try { inner, .. } => {
            if let Expr::Constructor { name, args, .. } = &**inner {
                if matches!(name.name.as_str(), "Ok" | "Some") && !args.is_empty() {
                    return static_type_of_with(&args[0], extern_methods);
                }
            }
            // For a Result<T, E>?, the unwrapped type is T.
            if let Expr::MethodCall {
                receiver,
                method,
                type_args,
                ..
            } = &**inner
            {
                if let Some(em) = extern_methods {
                    let recv_ty = static_type_of_with(receiver, extern_methods);
                    if let Some(info) = em.get(&(recv_ty, method.name.clone())) {
                        if let TypeExpr::Named { name, generics, .. } = &info.return_ty {
                            if (name == "Result" || name == "Option") && !generics.is_empty() {
                                // Prefer the call-site turbofish type if provided.
                                if !type_args.is_empty() {
                                    if let TypeExpr::Named { name, .. } = &type_args[0] {
                                        return name.clone();
                                    }
                                }
                                if let Some(inner_name) = generics[0].simple_name() {
                                    return inner_name.to_string();
                                }
                            }
                        }
                    }
                }
            }
            "<unknown>".to_string()
        }
        Expr::Match { .. } | Expr::Lambda { .. } | Expr::ProductValue { .. } => {
            "<unknown>".to_string()
        }
        Expr::FieldAccess { field, .. } => field.name.clone(),
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
            if body_calls_async_oneway(body, &async_methods, extern_methods) {
                async_methods.insert(key.clone());
                changed = true;
            }
        }
        for (name, body) in &free_bodies {
            if async_free_fns.contains(name) {
                continue;
            }
            if body_calls_async_oneway(body, &async_methods, extern_methods) {
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
    params.iter().any(|p| ty_mentions_suspending(&p.ty))
}

fn ty_mentions_suspending(ty: &TypeExpr) -> bool {
    match ty {
        TypeExpr::Named { name, generics, .. } if generics.is_empty() => {
            is_suspending_capability(name)
        }
        TypeExpr::Product { fields, .. } => fields.iter().any(ty_mentions_suspending),
        _ => false,
    }
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
            let recv_ty = static_type_of_with(receiver, Some(extern_methods));
            let key = (recv_ty, method.name.clone());
            if let Some(em) = extern_methods.get(&key) {
                if em.is_async {
                    return true;
                }
            }
            if expr_calls_async_extern(receiver, extern_methods) {
                return true;
            }
            args.iter()
                .any(|a| expr_calls_async_extern(a, extern_methods))
        }
        Expr::Constructor { args, .. } => args
            .iter()
            .any(|a| expr_calls_async_extern(a, extern_methods)),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            if expr_calls_async_extern(scrutinee, extern_methods) {
                return true;
            }
            arms.iter()
                .any(|arm| expr_calls_async_extern(&arm.body, extern_methods))
        }
        Expr::Try { inner, .. } => expr_calls_async_extern(inner, extern_methods),
        Expr::Lambda { body, .. } => body
            .exprs
            .iter()
            .any(|e| expr_calls_async_extern(e, extern_methods)),
        Expr::ProductValue { fields, .. } => fields
            .iter()
            .any(|f| expr_calls_async_extern(f, extern_methods)),
        Expr::FieldAccess { receiver, .. } => expr_calls_async_extern(receiver, extern_methods),
        _ => false,
    }
}

fn body_calls_async_oneway(
    body: &Block,
    async_methods: &HashSet<(String, String)>,
    extern_methods: &HashMap<(String, String), ExternMethod>,
) -> bool {
    body.exprs
        .iter()
        .any(|e| expr_calls_async_oneway(e, async_methods, extern_methods))
}

fn expr_calls_async_oneway(
    expr: &Expr,
    async_methods: &HashSet<(String, String)>,
    extern_methods: &HashMap<(String, String), ExternMethod>,
) -> bool {
    match expr {
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            let recv_ty = static_type_of_with(receiver, Some(extern_methods));
            let key = (recv_ty, method.name.clone());
            if async_methods.contains(&key) {
                return true;
            }
            if expr_calls_async_oneway(receiver, async_methods, extern_methods) {
                return true;
            }
            args.iter()
                .any(|a| expr_calls_async_oneway(a, async_methods, extern_methods))
        }
        Expr::Constructor { args, .. } => args
            .iter()
            .any(|a| expr_calls_async_oneway(a, async_methods, extern_methods)),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            if expr_calls_async_oneway(scrutinee, async_methods, extern_methods) {
                return true;
            }
            arms.iter()
                .any(|arm| expr_calls_async_oneway(&arm.body, async_methods, extern_methods))
        }
        Expr::Try { inner, .. } => expr_calls_async_oneway(inner, async_methods, extern_methods),
        Expr::Lambda { body, .. } => body
            .exprs
            .iter()
            .any(|e| expr_calls_async_oneway(e, async_methods, extern_methods)),
        Expr::ProductValue { fields, .. } => fields
            .iter()
            .any(|f| expr_calls_async_oneway(f, async_methods, extern_methods)),
        Expr::FieldAccess { receiver, .. } => {
            expr_calls_async_oneway(receiver, async_methods, extern_methods)
        }
        _ => false,
    }
}
