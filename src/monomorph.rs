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
    type_expr_canonical, Block, Expr, FunctionDef, Ident, Item, MatchArm, Module, TypeDef, TypeExpr,
};
use crate::error::{CanonError, Span};
use std::collections::{HashMap, HashSet, VecDeque};

/// The canonical flat name of an instantiation: `Box<Int>`,
/// `Map<String, Int>` — exactly the canonical spelling of the applied
/// type, so the instantiation key can never drift from the spelling
/// the checker compares. Doubles as the worklist key.
fn mangle(head: &str, args: &[TypeExpr]) -> String {
    type_expr_canonical(&TypeExpr::Named {
        name: head.to_string(),
        generics: args.to_vec(),
        span: Span::default(),
    })
}

/// The schema head of a minted instantiation name (`Map<String, Int>` →
/// `Map`), `None` for a source-declared name — `<` cannot appear in
/// one. The single reader of `mangle`'s format; every "is this item
/// compiler-minted?" question goes through here.
pub fn instantiation_head(name: &str) -> Option<&str> {
    name.split_once('<').map(|(head, _)| head)
}

struct Expander {
    /// `(constraint, type)` pairs — see the index built in `expand`.
    constraint_impls: HashSet<(String, String)>,
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
                // The call-site spelling a family is reached by — the
                // receiver's type name for a `Self`-normalized
                // constructor — shared with ordering and dead-code.
                let surface = crate::checker::decl_key(f);
                func_schemas.entry(surface).or_default().push(f.clone());
            }
            _ => {}
        }
    }
    // A generic function is reached through the type it constructs:
    // `instantiate` is driven by type applications, and a call site
    // names the instantiation by spelling that type
    // (`-> Inserted<String, Int>(…)`). So its parameters have to appear
    // in the constructed type — otherwise nothing can ever bind them,
    // the schema is never expanded, and the parameters survive into
    // codegen as an unresolved name. Left unchecked that builds an
    // invalid module from a program the checker accepted.
    //
    // Inference from the argument types would lift this; Milestone A
    // deliberately deferred it (#201), so for now it is an error rather
    // than a silent trap.
    // Which types satisfy a constraint. `<T: Ord>` is read as "some
    // `Ord` constructor accepts a `T`" — the same by-type routing every
    // call site uses, so a bound needs no new mechanism: it asks whether
    // the family the bound names has a member taking this type.
    let mut constraint_impls: HashSet<(String, String)> = HashSet::new();
    for item in &module.items {
        let Item::Function(f) = item else { continue };
        let key = crate::checker::decl_key(f);
        let note = |ty: &TypeExpr, set: &mut HashSet<(String, String)>| {
            if let TypeExpr::Named { name, .. } = ty {
                set.insert((key.clone(), name.clone()));
            }
        };
        for p in &f.params {
            match &p.ty {
                TypeExpr::Product { fields, .. } => {
                    for field in fields {
                        note(field, &mut constraint_impls);
                    }
                }
                TypeExpr::Repeat { ty, .. } => note(ty, &mut constraint_impls),
                other => note(other, &mut constraint_impls),
            }
        }
    }

    let mut seed_errors: Vec<CanonError> = Vec::new();
    for (surface, members) in &func_schemas {
        if type_schemas.contains_key(surface) {
            continue;
        }
        for schema in members {
            let params: Vec<String> = schema
                .generic_params
                .iter()
                .map(|g| g.name.name.clone())
                .collect();
            seed_errors.push(CanonError::CheckError {
                message: format!(
                    "`{}` declares type parameter(s) `{}` that its constructed type `{}` \
                     does not carry: a call site names the instantiation through that type, \
                     so nothing can bind them — declare `{}<{}>`",
                    surface,
                    params.join("`, `"),
                    surface,
                    surface,
                    params.join(", ")
                ),
                span: schema.name.span,
            });
        }
    }
    if type_schemas.is_empty() && func_schemas.is_empty() {
        return seed_errors;
    }

    let mut ex = Expander {
        constraint_impls,
        type_schemas,
        func_schemas,
        zero_data_variants,
        done: HashSet::new(),
        queue: VecDeque::new(),
        minted: Vec::new(),
        errors: seed_errors,
    };

    // Roots: every concrete generic application in a non-generic item.
    // Rewriting is in place — the application's spelling collapses to
    // its mangled flat name and enqueues the instantiation.
    let empty_binding = HashMap::new();
    for item in &mut module.items {
        match item {
            Item::TypeDef(td) if td.generic_params.is_empty() => {
                ex.rewrite_type(&mut td.body, &empty_binding);
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

    /// Rewrite a type expression under `binding` in place: a bound
    /// parameter is replaced by its (already-concrete) argument, and
    /// every concrete generic application collapses into its mangled
    /// flat name, enqueueing the instantiation. Bare references to
    /// generic declarations resolve through the binding.
    fn rewrite_type(&mut self, ty: &mut TypeExpr, binding: &HashMap<String, TypeExpr>) {
        match ty {
            TypeExpr::Named {
                name,
                generics,
                span,
            } => {
                if generics.is_empty() {
                    if let Some(t) = binding.get(name.as_str()) {
                        *ty = t.clone();
                        return;
                    }
                }
                for g in generics.iter_mut() {
                    self.rewrite_type(g, binding);
                }
                if self.is_generic_decl(name) {
                    if !generics.is_empty() {
                        let head = std::mem::take(name);
                        *name = self.enqueue(&head, generics);
                        generics.clear();
                    } else if !binding.is_empty() {
                        let head = std::mem::take(name);
                        match self.resolve_bare(&head, binding, *span) {
                            Some(mangled) => *name = mangled,
                            None => *name = head,
                        }
                    }
                }
            }
            TypeExpr::Union { variants, .. } => {
                for v in variants {
                    self.rewrite_type(v, binding);
                }
            }
            TypeExpr::Product { fields, .. } => {
                for f in fields {
                    self.rewrite_type(f, binding);
                }
            }
            TypeExpr::Repeat { ty, .. } | TypeExpr::Spread { ty, .. } => {
                self.rewrite_type(ty, binding);
            }
            TypeExpr::Function {
                generic_params,
                params,
                return_ty,
                ..
            } => {
                // A nested function type's own binders shadow the
                // enclosing binding.
                let narrowed;
                let inner = if generic_params.is_empty() {
                    binding
                } else {
                    let mut m = binding.clone();
                    for g in generic_params.iter() {
                        m.remove(&g.name.name);
                    }
                    narrowed = m;
                    &narrowed
                };
                for p in params {
                    self.rewrite_type(p, inner);
                }
                self.rewrite_type(return_ty, inner);
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
            let head = std::mem::take(name);
            match self.resolve_bare(&head, binding, span) {
                Some(mangled) => *name = mangled,
                None => *name = head,
            }
        }
    }

    fn arity_error(&mut self, head: &str, expected: usize, found: usize, span: Span) {
        self.errors.push(CanonError::CheckError {
            message: format!(
                "wrong number of type arguments for `{}`: expected {}, found {}",
                head, expected, found
            ),
            span,
        });
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
        let head = std::mem::take(name);
        *name = self.enqueue(&head, type_args);
        type_args.clear();
    }

    fn rewrite_function(&mut self, f: &mut FunctionDef, binding: &HashMap<String, TypeExpr>) {
        if let Some(recv) = &mut f.receiver {
            self.rewrite_expr_name(&mut recv.name, binding, recv.span);
        }
        for p in &mut f.params {
            self.rewrite_type(&mut p.ty, binding);
        }
        self.rewrite_type(&mut f.return_ty, binding);
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
                    self.rewrite_type(&mut p.ty, binding);
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
    /// Verify each bound at the point the parameter is bound to a
    /// concrete type. Instantiation is where a constraint can finally be
    /// decided — the schema itself says nothing about which types will
    /// arrive — so the error lands on the application that chose them.
    fn check_bounds(&mut self, head: &str, args: &[TypeExpr]) {
        // A name can be both a type schema and a constructor family
        // (`Shown<T> = String` plus `<T: Ord>(…) => Shown<T>`), and the
        // bound may be written on either. Check every parameter list
        // declared under this name, not the first one found.
        let mut param_lists: Vec<Vec<crate::ast::GenericParam>> = Vec::new();
        if let Some(td) = self.type_schemas.get(head) {
            param_lists.push(td.generic_params.clone());
        }
        if let Some(members) = self.func_schemas.get(head) {
            param_lists.extend(members.iter().map(|f| f.generic_params.clone()));
        }
        let mut reported: HashSet<(String, String)> = HashSet::new();
        for (param, arg) in param_lists.iter().flatten().zip(
            param_lists
                .iter()
                .flat_map(|list| args.iter().take(list.len())),
        ) {
            let Some(TypeExpr::Named { name: bound, .. }) = &param.bound else {
                continue;
            };
            let TypeExpr::Named { name: arg_name, .. } = arg else {
                continue;
            };
            if self
                .constraint_impls
                .contains(&(bound.clone(), arg_name.clone()))
            {
                continue;
            }
            if !reported.insert((bound.clone(), arg_name.clone())) {
                continue;
            }
            self.errors.push(CanonError::CheckError {
                message: format!(
                    "`{}` does not satisfy `{}`: no `{}` constructor accepts a `{}`",
                    arg_name, bound, bound, arg_name
                ),
                span: param.span,
            });
        }
    }

    fn instantiate(&mut self, head: &str, args: &[TypeExpr]) {
        let mangled = mangle(head, args);
        self.check_bounds(head, args);
        if let Some(schema) = self.type_schemas.get(head).cloned() {
            if schema.generic_params.len() != args.len() {
                self.arity_error(
                    head,
                    schema.generic_params.len(),
                    args.len(),
                    schema.name.span,
                );
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
                    self.arity_error(
                        head,
                        schema.generic_params.len(),
                        args.len(),
                        schema.name.span,
                    );
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
            variants.sort_by_cached_key(type_expr_canonical);
        }
        TypeExpr::Product { fields, .. } => {
            fields.sort_by_cached_key(type_expr_canonical);
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
