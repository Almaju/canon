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
    /// Non-generic typedef bodies as written (`Scores = Map<String,
    /// Int>`), for walking a value's type through its aliases.
    aliases: HashMap<String, TypeExpr>,
    /// Minted flat names → the application each spells, so a rewritten
    /// name reads back as a structured type.
    applications: HashMap<String, (String, Vec<TypeExpr>)>,
    /// Inside the declaration or arm being rewritten, the written name
    /// of each generic-typed input → its instantiated flat name (`Map`
    /// → `Map<String, Int>` for a `(Map<String, Int>) => …` input).
    locals: HashMap<String, String>,
    /// Message types: the input a command takes beside the value it
    /// changes (`Insert` in `Insert<K, V> * Map<K, V> => Map<K, V>`). A
    /// pipe into one keeps the value's type, and its type arguments come
    /// from the arguments alone.
    messages: HashSet<String>,
    /// The result type of each call inference resolved to a family
    /// member, by the call name's span — what the member returns, which
    /// may wrap the name the call spells (`Option<Value<String>>`).
    call_results: HashMap<(u32, usize, usize), TypeExpr>,
    /// Instantiations already minted (by mangled name, or by member key
    /// for a member reached only through inference).
    done: HashSet<String>,
    /// Pending instantiations.
    queue: VecDeque<Pending>,
    /// Minted concrete items.
    minted: Vec<Item>,
    errors: Vec<CanonError>,
}

/// A queued instantiation: a type application (`Map<String, Int>`,
/// which mints the typedef and every member constructing exactly that
/// type), or one family member reached only by inference at a call site
/// (`<K, V>(Map<K, V>) => Length`, keyed by its surface and index).
enum Pending {
    Type(String, Vec<TypeExpr>),
    Member(String, usize, Vec<TypeExpr>),
}

/// Expand every generic application in `module`. Minted items are
/// prepended — compiler output, outside the entry file's ordering — and
/// their count is returned with the errors so the caller can shift its
/// entry-items boundary.
pub fn expand(module: &mut Module) -> (Vec<CanonError>, usize) {
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

    // A member's parameters are bound at a call site by its inputs (the
    // value piped in and the arguments), or — when it constructs exactly
    // its type's application (`<K, V>(Unit) => Store<K, V>`) — by that
    // application. A parameter neither reaches can never be bound, and
    // would survive into codegen as an unresolved name.
    let mut seed_errors: Vec<CanonError> = Vec::new();
    for members in func_schemas.values() {
        for schema in members {
            let mut mentioned = HashSet::new();
            for p in &schema.params {
                mentioned_names(&p.ty, &mut mentioned);
            }
            if is_identity(schema, &type_schemas) {
                continue;
            }
            let unbound: Vec<String> = schema
                .generic_params
                .iter()
                .map(|g| g.name.name.clone())
                .filter(|p| !mentioned.contains(p))
                .collect();
            if unbound.is_empty() {
                continue;
            }
            let surface = crate::checker::decl_key(schema);
            seed_errors.push(CanonError::CheckError {
                message: format!(
                    "`{}` declares type parameter(s) `{}` that no input carries: a call \
                     site binds a parameter from the values it passes, so nothing can bind \
                     these — take them in an input, or construct `{}<{}>`",
                    surface,
                    unbound.join("`, `"),
                    surface,
                    unbound.join(", ")
                ),
                span: schema.name.span,
            });
        }
    }
    if type_schemas.is_empty() && func_schemas.is_empty() {
        return (seed_errors, 0);
    }

    let aliases: HashMap<String, TypeExpr> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::TypeDef(td) if td.generic_params.is_empty() => {
                Some((td.name.name.clone(), td.body.clone()))
            }
            _ => None,
        })
        .collect();

    let mut ex = Expander {
        constraint_impls,
        type_schemas,
        func_schemas,
        zero_data_variants,
        aliases,
        applications: HashMap::new(),
        locals: HashMap::new(),
        messages: module.items.iter().filter_map(message_of).collect(),
        call_results: HashMap::new(),
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

    while let Some(pending) = ex.queue.pop_front() {
        match pending {
            Pending::Type(head, args) => ex.instantiate(&head, &args),
            Pending::Member(surface, index, args) => ex.instantiate_member(&surface, index, &args),
        }
    }

    // A schema's diagnostic repeats in every instantiation; report it once.
    let mut seen = HashSet::new();
    ex.errors.retain(|e| {
        let s = e.span();
        seen.insert((e.message().to_string(), s.file, s.start, s.end))
    });
    let minted = ex.minted.len();
    module.items.splice(0..0, ex.minted);
    (ex.errors, minted)
}

/// The message a command takes: of a declaration's two inputs, the one
/// that is not the type it constructs (heads compared, so a generic
/// command counts).
fn message_of(item: &Item) -> Option<String> {
    let Item::Function(f) = item else {
        return None;
    };
    let constructed = head_of(constructed(&f.return_ty)?)?;
    let heads: Vec<&str> = f
        .receiver
        .iter()
        .filter(|_| f.name.name != "Self")
        .map(|r| r.name.as_str())
        .chain(f.params.iter().filter_map(|p| head_of(&p.ty)))
        .collect();
    match heads.as_slice() {
        [a, b] if *a == constructed && *b != constructed => Some(b.to_string()),
        [a, b] if *b == constructed && *a != constructed => Some(a.to_string()),
        _ => None,
    }
}

/// Whether a family member constructs exactly its type's application
/// with its own parameters in order (`<K, V>(…) => Store<K, V>`), so a
/// type application binds it.
fn is_identity(schema: &FunctionDef, type_schemas: &HashMap<String, TypeDef>) -> bool {
    let surface = crate::checker::decl_key(schema);
    if !type_schemas.contains_key(&surface) {
        return false;
    }
    let Some(TypeExpr::Named { name, generics, .. }) = constructed(&schema.return_ty) else {
        return false;
    };
    *name == surface
        && generics.len() == schema.generic_params.len()
        && generics.iter().zip(&schema.generic_params).all(|(g, p)| {
            matches!(g, TypeExpr::Named { name, generics, .. } if generics.is_empty() && *name == p.name.name)
        })
}

/// The type an arrow constructs, containers peeled (the structured
/// counterpart of `ast::constructed_type_name`).
fn constructed(ty: &TypeExpr) -> Option<&TypeExpr> {
    match ty {
        TypeExpr::Named { name, generics, .. }
            if matches!(name.as_str(), "Result" | "Option" | "Future") && !generics.is_empty() =>
        {
            constructed(&generics[0])
        }
        TypeExpr::Named { .. } => Some(ty),
        _ => None,
    }
}

/// Every name a type expression mentions, at any depth.
fn mentioned_names(ty: &TypeExpr, out: &mut HashSet<String>) {
    match ty {
        TypeExpr::Named { name, generics, .. } => {
            out.insert(name.clone());
            for g in generics {
                mentioned_names(g, out);
            }
        }
        TypeExpr::Union { variants, .. } => variants.iter().for_each(|v| mentioned_names(v, out)),
        TypeExpr::Product { fields, .. } => fields.iter().for_each(|f| mentioned_names(f, out)),
        TypeExpr::Repeat { ty, .. } => mentioned_names(ty, out),
        TypeExpr::Function {
            params, return_ty, ..
        } => {
            params.iter().for_each(|p| mentioned_names(p, out));
            mentioned_names(return_ty, out);
        }
    }
}

fn named(name: &str, generics: Vec<TypeExpr>) -> TypeExpr {
    TypeExpr::Named {
        name: name.to_string(),
        generics,
        span: Span::default(),
    }
}

impl Expander {
    /// A name that resolves through a binding: a generic type or one
    /// of its zero-data variants. A family whose surface is not a
    /// generic type (`<K, V>(Map<K, V>) => Length`) is reached only by
    /// inference.
    fn is_generic_decl(&self, name: &str) -> bool {
        self.type_schemas.contains_key(name) || self.zero_data_variants.contains_key(name)
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
        self.zero_data_variants.get(name).cloned()
    }

    fn enqueue(&mut self, head: &str, args: &[TypeExpr]) -> String {
        let key = mangle(head, args);
        self.applications
            .insert(key.clone(), (head.to_string(), args.to_vec()));
        if self.done.insert(key.clone()) {
            self.queue
                .push_back(Pending::Type(head.to_string(), args.to_vec()));
        }
        key
    }

    /// Queue one inference-only family member under `args`, returning
    /// the name its call site spells: the member's constructed type
    /// rewritten under the binding (`Keys<String>`, or `Length` for a
    /// non-generic result).
    fn enqueue_member(&mut self, surface: &str, index: usize, args: &[TypeExpr]) -> String {
        let schema = self.func_schemas[surface][index].clone();
        let binding = make_binding(&schema.generic_params, args);
        let mut target = constructed(&schema.return_ty)
            .cloned()
            .unwrap_or_else(|| named(surface, Vec::new()));
        self.rewrite_type(&mut target, &binding);
        let key = format!("{surface}#{index}{}", mangle("", args));
        if self.done.insert(key) {
            self.queue
                .push_back(Pending::Member(surface.to_string(), index, args.to_vec()));
        }
        match target {
            TypeExpr::Named { name, .. } => name,
            _ => surface.to_string(),
        }
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
            TypeExpr::Repeat { ty, .. } => {
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
        let saved = self.locals.clone();
        for p in &mut f.params {
            self.rewrite_type(&mut p.ty, binding);
            self.bind_local(&p.ty);
        }
        self.rewrite_type(&mut f.return_ty, binding);
        self.rewrite_block(&mut f.body, binding);
        self.locals = saved;
    }

    /// An input of instantiated type is still written by its head
    /// (`Map` for a `Map<String, Int>` input); record the spelling.
    fn bind_local(&mut self, ty: &TypeExpr) {
        if let TypeExpr::Named { name, .. } = ty {
            if let Some((head, _)) = self.applications.get(name) {
                self.locals.insert(head.clone(), name.clone());
            }
        }
    }

    fn rewrite_block(&mut self, block: &mut Block, binding: &HashMap<String, TypeExpr>) {
        for e in &mut block.exprs {
            self.rewrite_expr(e, binding);
        }
    }

    fn rewrite_expr(&mut self, e: &mut Expr, binding: &HashMap<String, TypeExpr>) {
        match e {
            Expr::Ident(id) => match self.locals.get(&id.name) {
                Some(local) => id.name = local.clone(),
                None => self.rewrite_expr_name(&mut id.name, binding, id.span),
            },
            Expr::Constructor {
                name,
                type_args,
                args,
                ..
            } => {
                for a in args.iter_mut() {
                    self.rewrite_expr(a, binding);
                }
                let handed = self.handed_types(None, args);
                self.resolve_call(&mut name.name, type_args, handed, binding, name.span);
            }
            Expr::MethodCall {
                receiver,
                method,
                type_args,
                args,
                ..
            } => {
                self.rewrite_expr(receiver, binding);
                for a in args.iter_mut() {
                    self.rewrite_expr(a, binding);
                }
                let receiver = (!self.messages.contains(&method.name)).then_some(&**receiver);
                let handed = self.handed_types(receiver, args);
                self.resolve_call(&mut method.name, type_args, handed, binding, method.span);
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.rewrite_expr(scrutinee, binding);
                let variants = self
                    .static_type(scrutinee)
                    .map(|t| self.variants_of(&t))
                    .unwrap_or_default();
                for arm in arms {
                    if let Some(v) = variants
                        .iter()
                        .find(|v| head_of(v) == head_of(&arm.param_ty))
                    {
                        complete_arm(&mut arm.param_ty, v, &|n| self.is_generic_decl(n));
                    }
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
                let saved = self.locals.clone();
                for p in params {
                    self.rewrite_type(&mut p.ty, binding);
                    self.bind_local(&p.ty);
                }
                self.rewrite_type(return_ty, binding);
                self.rewrite_block(body, binding);
                self.locals = saved;
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
                // A field of instantiated type is still written by its
                // head (`Node.Key` for a `Key<String>` field).
                let fields = self.static_type(receiver).and_then(|t| self.body_of(&t));
                if let Some(TypeExpr::Product { fields, .. }) = fields {
                    if let Some(f) = fields.iter().find(|f| {
                        matches!(f, TypeExpr::Named { generics, .. } if !generics.is_empty())
                            && head_of(f) == Some(field.name.as_str())
                    }) {
                        field.name = type_expr_canonical(f);
                    }
                }
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
        let saved = self.locals.clone();
        match &arm.param_ty {
            TypeExpr::Named { name, generics, .. }
                if matches!(name.as_str(), "Some" | "Ok" | "Err") && generics.len() == 1 =>
            {
                self.bind_local(&generics[0].clone());
            }
            ty => self.bind_local(&ty.clone()),
        }
        self.rewrite_block(&mut arm.body, binding);
        self.locals = saved;
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
                if !is_identity(&schema, &self.type_schemas) {
                    continue;
                }
                if schema.generic_params.len() != args.len() {
                    self.arity_error(
                        head,
                        schema.generic_params.len(),
                        args.len(),
                        schema.name.span,
                    );
                    continue;
                }
                self.mint_member(&schema, args, Some(&mangled));
            }
        }
        // A zero-data variant instantiation mints nothing: it exists
        // only as a name inside its union's instantiated body.
    }

    fn instantiate_member(&mut self, surface: &str, index: usize, args: &[TypeExpr]) {
        let schema = self.func_schemas[surface][index].clone();
        self.check_bounds(surface, args);
        let name = match constructed(&schema.return_ty) {
            Some(TypeExpr::Named { name, .. }) if self.is_generic_decl(name) => {
                let mut target = constructed(&schema.return_ty).cloned();
                if let Some(t) = &mut target {
                    self.rewrite_type(t, &make_binding(&schema.generic_params, args));
                }
                match target {
                    Some(TypeExpr::Named { name, .. }) => Some(name),
                    _ => None,
                }
            }
            _ => None,
        };
        self.mint_member(&schema, args, name.as_deref());
    }

    /// One concrete copy of a family member. A `Self`-normalized
    /// constructor keeps its name — its identity is the receiver, which
    /// `rewrite_function` renames to the instantiation; a named one takes
    /// `name` when its constructed type is generic.
    fn mint_member(&mut self, schema: &FunctionDef, args: &[TypeExpr], name: Option<&str>) {
        let binding = make_binding(&schema.generic_params, args);
        let mut copy = schema.clone();
        copy.generic_params = Vec::new();
        if copy.name.name != "Self" {
            if let Some(name) = name {
                copy.name = Ident {
                    name: name.to_string(),
                    span: schema.name.span,
                };
            }
        }
        self.rewrite_function(&mut copy, &binding);
        self.minted.push(Item::Function(copy));
    }

    // ── Inference ────────────────────────────────────────────────────
    //
    // A call that spells no type arguments takes them from the values it
    // is handed: the value piped in and the arguments. The written
    // arguments are for what nothing else gives — a root like
    // `Map<String, Int>()`.

    /// A rewritten type read back structured: minted flat names expand
    /// to the application they spell, recursively.
    fn structured(&self, ty: &TypeExpr) -> TypeExpr {
        match ty {
            TypeExpr::Named { name, generics, .. } => {
                if let Some((head, args)) = self.applications.get(name) {
                    return named(head, args.iter().map(|a| self.structured(a)).collect());
                }
                named(name, generics.iter().map(|g| self.structured(g)).collect())
            }
            other => other.clone(),
        }
    }

    /// One alias step: the type a newtype or alias names, its
    /// parameters substituted (`Rest<String, Int>` → `Map<String, Int>`).
    fn unalias(&self, ty: &TypeExpr) -> Option<TypeExpr> {
        let TypeExpr::Named { name, generics, .. } = ty else {
            return None;
        };
        let body = if let Some(td) = self.type_schemas.get(name) {
            if td.generic_params.len() != generics.len() {
                return None;
            }
            substitute(&td.body, &make_binding(&td.generic_params, generics))
        } else {
            self.aliases.get(name)?.clone()
        };
        matches!(body, TypeExpr::Named { .. }).then(|| self.structured(&body))
    }

    /// A type's body with its parameters substituted, when it is a
    /// product or union (`Node<String, Int>` → its three fields).
    fn body_of(&self, ty: &TypeExpr) -> Option<TypeExpr> {
        let mut cur = self.structured(ty);
        for _ in 0..20 {
            let TypeExpr::Named { name, generics, .. } = &cur else {
                return None;
            };
            let body = match self.type_schemas.get(name) {
                Some(td) if td.generic_params.len() == generics.len() => {
                    substitute(&td.body, &make_binding(&td.generic_params, generics))
                }
                _ => self.aliases.get(name)?.clone(),
            };
            match body {
                TypeExpr::Named { .. } => cur = self.structured(&body),
                other => return Some(other),
            }
        }
        None
    }

    /// The static type of a rewritten expression, as far as inference
    /// needs one: a name is its own type (the only names are type names),
    /// a literal its primitive, a field its declared type.
    fn static_type(&self, e: &Expr) -> Option<TypeExpr> {
        match e {
            Expr::Ident(id) => Some(self.structured(&named(&id.name, Vec::new()))),
            Expr::StringLit { .. } | Expr::FormatLit { .. } => Some(named("String", Vec::new())),
            Expr::IntLit { .. } => Some(named("Int", Vec::new())),
            Expr::FloatLit { .. } => Some(named("Float", Vec::new())),
            Expr::Constructor { name, .. } | Expr::MethodCall { method: name, .. }
                if self.call_results.contains_key(&(
                    name.span.file,
                    name.span.start,
                    name.span.end,
                )) =>
            {
                let s = name.span;
                Some(self.call_results[&(s.file, s.start, s.end)].clone())
            }
            Expr::Constructor { name, .. } if crate::ast::is_type_name(&name.name) => {
                Some(self.structured(&named(&name.name, Vec::new())))
            }
            Expr::MethodCall {
                receiver, method, ..
            } => match method.name.as_str() {
                "Sum" | "Difference" | "Product" | "Quotient" | "Remainder" | "Joined" => {
                    self.static_type(receiver)
                }
                name if head_of(&named(name, Vec::new()))
                    .is_some_and(|h| self.messages.contains(h)) =>
                {
                    self.static_type(receiver)
                }
                name if crate::ast::is_type_name(name) => {
                    Some(self.structured(&named(name, Vec::new())))
                }
                _ => None,
            },
            Expr::FieldAccess {
                receiver, field, ..
            } => {
                let recv = self.static_type(receiver)?;
                let TypeExpr::Product { fields, .. } = self.body_of(&recv)? else {
                    return None;
                };
                let wanted = self.structured(&named(&field.name, Vec::new()));
                fields.into_iter().find(|f| {
                    type_expr_canonical(f) == type_expr_canonical(&wanted)
                        || head_of(f) == head_of(&wanted)
                })
            }
            Expr::Try { inner, .. } => match self.static_type(inner)? {
                TypeExpr::Named { name, generics, .. }
                    if matches!(name.as_str(), "Result" | "Option") && !generics.is_empty() =>
                {
                    Some(generics[0].clone())
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Bind `params` in `pattern` so it describes `concrete`, walking the
    /// concrete side through its aliases. `false` leaves `binding` as it
    /// was.
    fn unify(
        &self,
        pattern: &TypeExpr,
        concrete: &TypeExpr,
        params: &[String],
        binding: &mut HashMap<String, TypeExpr>,
        loose: bool,
    ) -> bool {
        let TypeExpr::Named {
            name: p_name,
            generics: p_args,
            ..
        } = pattern
        else {
            return false;
        };
        if p_args.is_empty() && params.contains(p_name) {
            return match binding.get(p_name) {
                Some(bound) => type_expr_canonical(bound) == type_expr_canonical(concrete),
                None => {
                    binding.insert(p_name.clone(), concrete.clone());
                    true
                }
            };
        }
        let mut cur = concrete.clone();
        for _ in 0..20 {
            if let TypeExpr::Named { name, generics, .. } = &cur {
                if name == p_name && generics.len() == p_args.len() {
                    let mut trial = binding.clone();
                    if p_args
                        .iter()
                        .zip(generics)
                        .all(|(p, c)| self.unify(p, c, params, &mut trial, loose))
                    {
                        *binding = trial;
                        return true;
                    }
                }
            }
            match self.unalias(&cur) {
                Some(next) => cur = next,
                None => break,
            }
        }
        // Loosely, the pattern walks its own aliases too: a `Key<K>`
        // input (`Key<K> = K`) takes a plain `String`.
        loose
            && self
                .unalias_pattern(pattern)
                .is_some_and(|p| self.unify(&p, concrete, params, binding, loose))
    }

    /// One alias step of a pattern, its own parameters left in place.
    fn unalias_pattern(&self, pattern: &TypeExpr) -> Option<TypeExpr> {
        let TypeExpr::Named { name, generics, .. } = pattern else {
            return None;
        };
        let body = match self.type_schemas.get(name) {
            Some(td) if td.generic_params.len() == generics.len() => {
                substitute(&td.body, &make_binding(&td.generic_params, generics))
            }
            _ => self.aliases.get(name)?.clone(),
        };
        matches!(body, TypeExpr::Named { .. }).then_some(body)
    }

    /// Bind `params` from the handed values through `patterns`, `Some`
    /// once every parameter is bound. Values land by their own type first — a pattern naming the
    /// value's type, not a bare parameter — and only then loosely, where
    /// a bare parameter or an alias of the value's type takes it. With
    /// `every`, a value that lands nowhere rejects the member: a call
    /// hands a member exactly its inputs.
    fn bind_from(
        &self,
        params: &[String],
        patterns: &[TypeExpr],
        values: &[TypeExpr],
        every: bool,
    ) -> Option<Vec<TypeExpr>> {
        let mut binding = HashMap::new();
        let mut used = vec![false; patterns.len()];
        let mut landed = vec![false; values.len()];
        for loose in [false, true] {
            for (v, value) in values.iter().enumerate() {
                if landed[v] {
                    continue;
                }
                for (i, pattern) in patterns.iter().enumerate() {
                    let bare = matches!(pattern, TypeExpr::Named { name, generics, .. }
                        if generics.is_empty() && params.contains(name));
                    if used[i] || (bare && !loose) {
                        continue;
                    }
                    if self.unify(pattern, value, params, &mut binding, loose) {
                        used[i] = true;
                        landed[v] = true;
                        break;
                    }
                }
            }
        }
        if every && landed.contains(&false) {
            return None;
        }
        params.iter().map(|p| binding.get(p).cloned()).collect()
    }

    /// The instantiated name a call to `name` without type arguments
    /// resolves to, when the handed values bind a family member's
    /// parameters (`map -> Length` → the `Map<K, V>` member).
    fn infer_member(&mut self, name: &str, values: &[TypeExpr]) -> Option<(String, TypeExpr)> {
        let members = self.func_schemas.get(name)?.clone();
        for (index, schema) in members.iter().enumerate() {
            let params: Vec<String> = schema
                .generic_params
                .iter()
                .map(|g| g.name.name.clone())
                .collect();
            let patterns: Vec<TypeExpr> = schema.params.iter().map(|p| p.ty.clone()).collect();
            let Some(args) = self.bind_from(&params, &patterns, values, true) else {
                continue;
            };
            let result = substitute(
                &schema.return_ty,
                &make_binding(&schema.generic_params, &args),
            );
            let resolved = if is_identity(schema, &self.type_schemas) {
                self.enqueue(name, &args)
            } else {
                self.enqueue_member(name, index, &args)
            };
            return Some((resolved, result));
        }
        None
    }

    /// The instantiation a construction of generic type `name` without
    /// type arguments builds, when the handed values bind its parameters
    /// through its body (`Key("a")` → `Key<String>`).
    fn infer_construction(&mut self, name: &str, values: &[TypeExpr]) -> Option<String> {
        let schema = self.type_schemas.get(name)?;
        let params: Vec<String> = schema
            .generic_params
            .iter()
            .map(|g| g.name.name.clone())
            .collect();
        // A value already of this type relabels as itself; otherwise the
        // values fill the body.
        let itself = named(name, params.iter().map(|p| named(p, Vec::new())).collect());
        let mut patterns = vec![itself];
        match &schema.body {
            TypeExpr::Product { fields, .. } => patterns.extend(fields.iter().cloned()),
            body @ TypeExpr::Named { .. } => patterns.push(body.clone()),
            _ => return None,
        }
        let args = self.bind_from(&params, &patterns, values, false)?;
        Some(self.enqueue(name, &args))
    }

    /// The values a call hands over, typed: the receiver, then each
    /// argument (a product argument flattened).
    fn handed_types(&self, receiver: Option<&Expr>, args: &[Expr]) -> Vec<TypeExpr> {
        let flat: Vec<&Expr> = match args {
            [Expr::ProductValue { fields, .. }] => fields.iter().collect(),
            _ => args.iter().collect(),
        };
        receiver
            .into_iter()
            .chain(flat)
            .filter_map(|e| self.static_type(e))
            .collect()
    }

    /// Resolve a call's name. Explicit type arguments are the root
    /// spelling; without them a family member the handed values bind
    /// wins, then the enclosing binding, then a construction the values
    /// bind.
    fn resolve_call(
        &mut self,
        name: &mut String,
        type_args: &mut Vec<TypeExpr>,
        handed: Vec<TypeExpr>,
        binding: &HashMap<String, TypeExpr>,
        span: Span,
    ) {
        if !type_args.is_empty() {
            let written: Vec<String> = type_args.iter().map(type_expr_canonical).collect();
            let head = name.clone();
            self.apply_expr_type_args(name, type_args, binding);
            if !handed.is_empty() && !binding.contains_key(&head) {
                let mut probe = self.clone_for_probe();
                let inferred = probe
                    .infer_member(&head, &handed)
                    .map(|(n, _)| n)
                    .or_else(|| probe.infer_construction(&head, &handed));
                if inferred.as_deref() == Some(name.as_str()) {
                    self.errors.push(CanonError::CheckError {
                        message: format!(
                            "`{head}<{}>`: the values handed to `{head}` give its type \
                             arguments — write `{head}`",
                            written.join(", ")
                        ),
                        span,
                    });
                }
            }
            return;
        }
        if binding.contains_key(name.as_str()) {
            self.rewrite_expr_name(name, binding, span);
            return;
        }
        if let Some((resolved, result)) = self.infer_member(name, &handed) {
            *name = resolved;
            self.call_results
                .insert((span.file, span.start, span.end), result);
            return;
        }
        // A declaration merged across files keeps one file's parameter
        // names, so the binding covers it only when those names match.
        let covered = self
            .decl_params(name)
            .is_some_and(|ps| ps.iter().all(|p| binding.contains_key(p)));
        if covered {
            self.rewrite_expr_name(name, binding, span);
            return;
        }
        if let Some(resolved) = self.infer_construction(name, &handed) {
            *name = resolved;
            return;
        }
        self.rewrite_expr_name(name, binding, span);
    }

    /// A throwaway copy for asking what inference *would* resolve,
    /// without queueing anything.
    fn clone_for_probe(&self) -> Expander {
        Expander {
            constraint_impls: HashSet::new(),
            type_schemas: self.type_schemas.clone(),
            func_schemas: self.func_schemas.clone(),
            zero_data_variants: self.zero_data_variants.clone(),
            aliases: self.aliases.clone(),
            applications: self.applications.clone(),
            locals: HashMap::new(),
            messages: self.messages.clone(),
            call_results: HashMap::new(),
            done: HashSet::new(),
            queue: VecDeque::new(),
            minted: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// The variants a dispatch on `ty` tests, instantiated
    /// (`Option<Value<String>>` → `None`, `Some<Value<String>>`).
    fn variants_of(&self, ty: &TypeExpr) -> Vec<TypeExpr> {
        let ty = self.structured(ty);
        if let TypeExpr::Named { name, generics, .. } = &ty {
            match (name.as_str(), generics.as_slice()) {
                ("Option", [t]) => {
                    return vec![named("None", Vec::new()), named("Some", vec![t.clone()])]
                }
                ("Result", [t, e]) => {
                    return vec![named("Err", vec![e.clone()]), named("Ok", vec![t.clone()])]
                }
                _ => {}
            }
        }
        match self.body_of(&ty) {
            Some(TypeExpr::Union { variants, .. }) => {
                variants.iter().map(|v| self.structured(v)).collect()
            }
            _ => Vec::new(),
        }
    }
}

/// Fill the bare generic names of a written arm type from the variant it
/// tests (`Some<Value>` against `Some<Value<String>>`).
fn complete_arm(written: &mut TypeExpr, variant: &TypeExpr, generic: &dyn Fn(&str) -> bool) {
    let (
        TypeExpr::Named {
            name: w_name,
            generics: w_args,
            ..
        },
        TypeExpr::Named {
            name: v_name,
            generics: v_args,
            ..
        },
    ) = (&mut *written, variant)
    else {
        return;
    };
    if w_name != v_name {
        return;
    }
    if w_args.is_empty() && !v_args.is_empty() && generic(w_name) {
        *w_args = v_args.clone();
        return;
    }
    if w_args.len() == v_args.len() {
        for (w, v) in w_args.iter_mut().zip(v_args) {
            complete_arm(w, v, generic);
        }
    }
}

fn head_of(ty: &TypeExpr) -> Option<&str> {
    match ty {
        TypeExpr::Named { name, .. } => Some(name.split('<').next().unwrap_or(name)),
        _ => None,
    }
}

fn substitute(ty: &TypeExpr, binding: &HashMap<String, TypeExpr>) -> TypeExpr {
    match ty {
        TypeExpr::Named { name, generics, .. } => {
            if generics.is_empty() {
                if let Some(t) = binding.get(name) {
                    return t.clone();
                }
            }
            named(
                name,
                generics.iter().map(|g| substitute(g, binding)).collect(),
            )
        }
        TypeExpr::Union { variants, span } => TypeExpr::Union {
            variants: variants.iter().map(|v| substitute(v, binding)).collect(),
            span: *span,
        },
        TypeExpr::Product { fields, span } => TypeExpr::Product {
            fields: fields.iter().map(|f| substitute(f, binding)).collect(),
            span: *span,
        },
        other => other.clone(),
    }
}

fn sort_canonical(ty: &mut TypeExpr) {
    // A package-qualified name sorts as its plain name would.
    let key = |t: &TypeExpr| crate::ast::plain_name(&type_expr_canonical(t)).to_string();
    match ty {
        TypeExpr::Union { variants, .. } => variants.sort_by_cached_key(key),
        TypeExpr::Product { fields, .. } => fields.sort_by_cached_key(key),
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
