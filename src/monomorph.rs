//! Generic-type instantiation by syntactic expansion.
//!
//! Runs on the merged module after loading, before checking and
//! codegen. Every concrete application of a generic declaration
//! (`Map<String, Int>` in type position, `Box<Int>(42)` in expression
//! position) is a root; each root mints a concrete `TypeDef` /
//! `FunctionDef` copy with the parameters substituted, named by the
//! application's canonical spelling (`Box<Int>` as a flat name), and
//! the copies are re-expanded until the worklist drains. Instantiated
//! copies are ordinary Canon: the checker checks each one fully and
//! codegen never sees a type parameter.
//!
//! Inside a generic body, a bare reference to another generic
//! declaration (the `Empty` variant, a sibling newtype) resolves its
//! arguments through the enclosing binding *by parameter name* — a
//! family shares its parameter names, and a reference whose parameter
//! the binding doesn't cover is an error.

use crate::ast::{
    substitute_type_params, type_expr_canonical, Block, Expr, FunctionDef, Ident, Item, MatchArm,
    Module, TypeDef, TypeExpr,
};
use crate::error::{CanonError, Span};
use std::collections::{HashMap, HashSet, VecDeque};

/// The canonical flat name of an instantiation: `Box<Int>`,
/// `Map<String, Int>`. Doubles as the worklist key.
fn mangle(head: &str, args: &[TypeExpr]) -> String {
    let rendered: Vec<String> = args.iter().map(type_expr_canonical).collect();
    format!("{}<{}>", head, rendered.join(", "))
}

struct Expander {
    /// Generic typedef schemas by name.
    type_schemas: HashMap<String, TypeDef>,
    /// Generic function schemas grouped by declared name (constructor
    /// families can hold several members under one name).
    func_schemas: HashMap<String, Vec<FunctionDef>>,
    /// Zero-data variants of generic unions (`Empty` in
    /// `Map<K, V> = Empty + Node<K, V>`) → the owning union's
    /// parameter names. Such a variant has no typedef of its own; its
    /// instantiated name still carries the union's arguments so each
    /// instantiation keeps distinct variants.
    zero_data_variants: HashMap<String, Vec<String>>,
    /// Instantiations already minted (by mangled name).
    done: HashSet<String>,
    /// Pending (head, concrete args) instantiations.
    queue: VecDeque<(String, Vec<TypeExpr>)>,
    /// Minted concrete items.
    minted: Vec<Item>,
    errors: Vec<CanonError>,
}

pub fn expand(module: &mut Module) -> Vec<CanonError> {
    let mut type_schemas = HashMap::new();
    let mut func_schemas: HashMap<String, Vec<FunctionDef>> = HashMap::new();
    let mut zero_data_variants = HashMap::new();
    for item in &module.items {
        match item {
            Item::TypeDef(td) if !td.generic_params.is_empty() => {
                let params: Vec<String> = td
                    .generic_params
                    .iter()
                    .map(|g| g.name.name.clone())
                    .collect();
                if let TypeExpr::Union { variants, .. } = &td.body {
                    for v in variants {
                        if let TypeExpr::Named { name, generics, .. } = v {
                            if generics.is_empty() {
                                zero_data_variants.insert(name.clone(), params.clone());
                            }
                        }
                    }
                }
                type_schemas.insert(td.name.name.clone(), td.clone());
            }
            Item::Function(f) if !f.generic_params.is_empty() => {
                // `resolve_new_syntax` normalizes a constructor to name
                // `Self` with the constructed type as receiver; the
                // call-site spelling — the name expansion must match —
                // is the receiver's type name.
                let surface = if f.name.name == "Self" {
                    f.receiver
                        .as_ref()
                        .map(|r| r.name.clone())
                        .unwrap_or_else(|| f.name.name.clone())
                } else {
                    f.name.name.clone()
                };
                func_schemas.entry(surface).or_default().push(f.clone());
            }
            _ => {}
        }
    }
    if type_schemas.is_empty() && func_schemas.is_empty() {
        return Vec::new();
    }

    let mut ex = Expander {
        type_schemas,
        func_schemas,
        zero_data_variants,
        done: HashSet::new(),
        queue: VecDeque::new(),
        minted: Vec::new(),
        errors: Vec::new(),
    };

    // Roots: every concrete generic application in a non-generic item.
    // Rewriting is in place — the application's spelling collapses to
    // its mangled flat name and enqueues the instantiation.
    let empty_binding = HashMap::new();
    for item in &mut module.items {
        match item {
            Item::TypeDef(td) if td.generic_params.is_empty() => {
                let mut body = td.body.clone();
                ex.rewrite_type(&mut body, &empty_binding);
                td.body = body;
            }
            Item::Function(f) if f.generic_params.is_empty() => {
                ex.rewrite_function(f, &empty_binding);
            }
            _ => {}
        }
    }

    while let Some((head, args)) = ex.queue.pop_front() {
        ex.instantiate(&head, &args);
    }

    module.items.append(&mut ex.minted);
    ex.errors
}

impl Expander {
    fn is_generic_decl(&self, name: &str) -> bool {
        self.type_schemas.contains_key(name)
            || self.func_schemas.contains_key(name)
            || self.zero_data_variants.contains_key(name)
    }

    /// Parameter names a bare reference to `name` needs bound: the
    /// declaration's own binders.
    fn decl_params(&self, name: &str) -> Option<Vec<String>> {
        if let Some(td) = self.type_schemas.get(name) {
            return Some(
                td.generic_params
                    .iter()
                    .map(|g| g.name.name.clone())
                    .collect(),
            );
        }
        if let Some(fs) = self.func_schemas.get(name) {
            return fs.first().map(|f| {
                f.generic_params
                    .iter()
                    .map(|g| g.name.name.clone())
                    .collect()
            });
        }
        self.zero_data_variants.get(name).cloned()
    }

    fn enqueue(&mut self, head: &str, args: &[TypeExpr]) -> String {
        let key = mangle(head, args);
        if self.done.insert(key.clone()) {
            self.queue.push_back((head.to_string(), args.to_vec()));
        }
        key
    }

    /// Resolve a bare reference to a generic declaration through the
    /// enclosing binding by parameter name. Returns the mangled
    /// instantiation name, or `None` (with an error pushed) when the
    /// binding doesn't cover the declaration's parameters.
    fn resolve_bare(
        &mut self,
        name: &str,
        binding: &HashMap<String, TypeExpr>,
        span: Span,
    ) -> Option<String> {
        let params = self.decl_params(name)?;
        let mut args = Vec::with_capacity(params.len());
        for p in &params {
            match binding.get(p) {
                Some(t) => args.push(t.clone()),
                None => {
                    self.errors.push(CanonError::CheckError {
                        message: format!(
                            "generic `{}` referenced without arguments and parameter `{}` is not bound here — apply it explicitly",
                            name, p
                        ),
                        span,
                    });
                    return None;
                }
            }
        }
        Some(self.enqueue(name, &args))
    }

    /// Rewrite a type expression under `binding`: substitute bound
    /// parameters, then collapse every concrete generic application
    /// into its mangled flat name, enqueueing the instantiation. Bare
    /// references to generic declarations resolve through the binding.
    fn rewrite_type(&mut self, ty: &mut TypeExpr, binding: &HashMap<String, TypeExpr>) {
        if !binding.is_empty() {
            *ty = substitute_type_params(ty, binding);
        }
        self.mangle_type(ty, binding);
    }

    fn mangle_type(&mut self, ty: &mut TypeExpr, binding: &HashMap<String, TypeExpr>) {
        match ty {
            TypeExpr::Named {
                name,
                generics,
                span,
            } => {
                for g in generics.iter_mut() {
                    self.mangle_type(g, binding);
                }
                if !generics.is_empty() && self.is_generic_decl(name) {
                    let all_concrete = generics
                        .iter()
                        .all(|g| !type_mentions_names(g, &binding_domain(binding)));
                    if all_concrete {
                        let mangled = self.enqueue(&name.clone(), generics);
                        *name = mangled;
                        generics.clear();
                    }
                } else if generics.is_empty() && self.is_generic_decl(name) && !binding.is_empty() {
                    if let Some(mangled) = self.resolve_bare(&name.clone(), binding, *span) {
                        *name = mangled;
                    }
                }
            }
            TypeExpr::Union { variants, .. } => {
                for v in variants {
                    self.mangle_type(v, binding);
                }
            }
            TypeExpr::Product { fields, .. } => {
                for f in fields {
                    self.mangle_type(f, binding);
                }
            }
            TypeExpr::Repeat { ty, .. } | TypeExpr::Spread { ty, .. } => {
                self.mangle_type(ty, binding);
            }
            TypeExpr::Function {
                params, return_ty, ..
            } => {
                for p in params {
                    self.mangle_type(p, binding);
                }
                self.mangle_type(return_ty, binding);
            }
        }
    }

    /// Rewrite a name that appears in expression position (a
    /// constructor call, method name, value reference, or field
    /// access). Under a binding, a substituted parameter renames to
    /// its concrete spelling and a generic declaration renames to its
    /// instantiation.
    fn rewrite_expr_name(
        &mut self,
        name: &mut String,
        binding: &HashMap<String, TypeExpr>,
        span: Span,
    ) {
        if let Some(t) = binding.get(name.as_str()) {
            *name = type_expr_canonical(t);
            return;
        }
        if !binding.is_empty() && self.is_generic_decl(name) {
            if let Some(mangled) = self.resolve_bare(&name.clone(), binding, span) {
                *name = mangled;
            }
        }
    }

    /// Fold explicit call-site type arguments into the call's name.
    /// Outside a binding this is the instantiation root; inside one,
    /// the arguments are substituted first.
    fn apply_expr_type_args(
        &mut self,
        name: &mut String,
        type_args: &mut Vec<TypeExpr>,
        binding: &HashMap<String, TypeExpr>,
    ) {
        if type_args.is_empty() {
            return;
        }
        if !self.is_generic_decl(name) {
            return; // left for the checker to reject
        }
        for t in type_args.iter_mut() {
            self.rewrite_type(t, binding);
        }
        if type_args
            .iter()
            .any(|t| type_mentions_names(t, &binding_domain(binding)))
        {
            return; // still parameterized — only reachable on malformed input
        }
        let mangled = self.enqueue(&name.clone(), type_args);
        *name = mangled;
        type_args.clear();
    }

    fn rewrite_function(&mut self, f: &mut FunctionDef, binding: &HashMap<String, TypeExpr>) {
        if let Some(recv) = &mut f.receiver {
            self.rewrite_expr_name(&mut recv.name, binding, recv.span);
        }
        for p in &mut f.params {
            let mut ty = p.ty.clone();
            self.rewrite_type(&mut ty, binding);
            p.ty = ty;
        }
        let mut ret = f.return_ty.clone();
        self.rewrite_type(&mut ret, binding);
        f.return_ty = ret;
        self.rewrite_block(&mut f.body, binding);
    }

    fn rewrite_block(&mut self, block: &mut Block, binding: &HashMap<String, TypeExpr>) {
        for e in &mut block.exprs {
            self.rewrite_expr(e, binding);
        }
    }

    fn rewrite_expr(&mut self, e: &mut Expr, binding: &HashMap<String, TypeExpr>) {
        match e {
            Expr::Ident(id) => self.rewrite_expr_name(&mut id.name, binding, id.span),
            Expr::Constructor {
                name,
                type_args,
                args,
                ..
            } => {
                self.apply_expr_type_args(&mut name.name, type_args, binding);
                self.rewrite_expr_name(&mut name.name, binding, name.span);
                for a in args {
                    self.rewrite_expr(a, binding);
                }
            }
            Expr::MethodCall {
                receiver,
                method,
                type_args,
                args,
                ..
            } => {
                self.rewrite_expr(receiver, binding);
                self.apply_expr_type_args(&mut method.name, type_args, binding);
                self.rewrite_expr_name(&mut method.name, binding, method.span);
                for a in args {
                    self.rewrite_expr(a, binding);
                }
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.rewrite_expr(scrutinee, binding);
                for arm in arms {
                    self.rewrite_arm(arm, binding);
                }
            }
            Expr::Try { inner, .. } => self.rewrite_expr(inner, binding),
            Expr::Lambda {
                params,
                return_ty,
                body,
                ..
            } => {
                for p in params {
                    let mut ty = p.ty.clone();
                    self.rewrite_type(&mut ty, binding);
                    p.ty = ty;
                }
                self.rewrite_type(return_ty, binding);
                self.rewrite_block(body, binding);
            }
            Expr::ProductValue { fields, .. } => {
                for f in fields {
                    self.rewrite_expr(f, binding);
                }
            }
            Expr::FieldAccess {
                receiver, field, ..
            } => {
                self.rewrite_expr(receiver, binding);
                self.rewrite_expr_name(&mut field.name, binding, field.span);
            }
            Expr::JsonLit { parts, .. } => {
                for p in parts {
                    if let crate::ast::JsonLitPart::Interp(inner) = p {
                        self.rewrite_expr(inner, binding);
                    }
                }
            }
            Expr::HtmlLit { parts, .. } => {
                for p in parts {
                    if let crate::ast::HtmlLitPart::Interp(inner) = p {
                        self.rewrite_expr(inner, binding);
                    }
                }
            }
            Expr::FormatLit { parts, .. } => {
                for p in parts {
                    if let crate::ast::FormatLitPart::Interp(inner) = p {
                        self.rewrite_expr(inner, binding);
                    }
                }
            }
            Expr::Await { inner, .. } => self.rewrite_expr(inner, binding),
            Expr::StringLit { .. } | Expr::IntLit { .. } | Expr::FloatLit { .. } => {}
        }
    }

    fn rewrite_arm(&mut self, arm: &mut MatchArm, binding: &HashMap<String, TypeExpr>) {
        self.rewrite_type(&mut arm.param_ty, binding);
        self.rewrite_type(&mut arm.return_ty, binding);
        self.rewrite_block(&mut arm.body, binding);
    }

    /// Mint the concrete copies for one instantiation: the typedef
    /// (when `head` names one) and every function-family member.
    fn instantiate(&mut self, head: &str, args: &[TypeExpr]) {
        let mangled = mangle(head, args);
        if let Some(schema) = self.type_schemas.get(head).cloned() {
            if schema.generic_params.len() != args.len() {
                self.errors.push(CanonError::CheckError {
                    message: format!(
                        "wrong number of type arguments for `{}`: expected {}, found {}",
                        head,
                        schema.generic_params.len(),
                        args.len()
                    ),
                    span: schema.name.span,
                });
                return;
            }
            let binding = make_binding(&schema.generic_params, args);
            let mut body = schema.body.clone();
            self.rewrite_type(&mut body, &binding);
            // Substitution can reorder same-head components
            // (`Box<A> * Box<B>` under A=String, B=Int), so the minted
            // body re-sorts into canonical order — the copy is compiler
            // output and must pass the same checks as source.
            sort_canonical(&mut body);
            self.minted.push(Item::TypeDef(TypeDef {
                name: Ident {
                    name: mangled.clone(),
                    span: schema.name.span,
                },
                generic_params: Vec::new(),
                body,
                span: schema.span,
            }));
        }
        if let Some(members) = self.func_schemas.get(head).cloned() {
            for schema in members {
                if schema.generic_params.len() != args.len() {
                    self.errors.push(CanonError::CheckError {
                        message: format!(
                            "wrong number of type arguments for `{}`: expected {}, found {}",
                            head,
                            schema.generic_params.len(),
                            args.len()
                        ),
                        span: schema.name.span,
                    });
                    continue;
                }
                let binding = make_binding(&schema.generic_params, args);
                let mut copy = schema.clone();
                copy.generic_params = Vec::new();
                // A `Self`-normalized constructor keeps its name — its
                // identity is the receiver, which `rewrite_function`
                // renames to the instantiation.
                if copy.name.name != "Self" {
                    copy.name = Ident {
                        name: mangled.clone(),
                        span: schema.name.span,
                    };
                }
                self.rewrite_function(&mut copy, &binding);
                self.minted.push(Item::Function(copy));
            }
        }
        // A zero-data variant instantiation mints nothing: it exists
        // only as a name inside its union's instantiated body.
    }
}

fn sort_canonical(ty: &mut TypeExpr) {
    match ty {
        TypeExpr::Union { variants, .. } => {
            variants.sort_by_key(type_expr_canonical);
        }
        TypeExpr::Product { fields, .. } => {
            fields.sort_by_key(type_expr_canonical);
        }
        _ => {}
    }
}

fn make_binding(
    params: &[crate::ast::GenericParam],
    args: &[TypeExpr],
) -> HashMap<String, TypeExpr> {
    params
        .iter()
        .zip(args.iter())
        .map(|(p, a)| (p.name.name.clone(), a.clone()))
        .collect()
}

fn binding_domain(binding: &HashMap<String, TypeExpr>) -> HashSet<&str> {
    binding.keys().map(|k| k.as_str()).collect()
}

/// Does `ty` mention any of `names` as a bare reference?
fn type_mentions_names(ty: &TypeExpr, names: &HashSet<&str>) -> bool {
    match ty {
        TypeExpr::Named { name, generics, .. } => {
            (generics.is_empty() && names.contains(name.as_str()))
                || generics.iter().any(|g| type_mentions_names(g, names))
        }
        TypeExpr::Union { variants, .. } => variants.iter().any(|v| type_mentions_names(v, names)),
        TypeExpr::Product { fields, .. } => fields.iter().any(|f| type_mentions_names(f, names)),
        TypeExpr::Repeat { ty, .. } | TypeExpr::Spread { ty, .. } => type_mentions_names(ty, names),
        TypeExpr::Function {
            params, return_ty, ..
        } => {
            params.iter().any(|p| type_mentions_names(p, names))
                || type_mentions_names(return_ty, names)
        }
    }
}
