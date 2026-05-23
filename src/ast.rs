use crate::error::Span;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct Module {
    pub items: Vec<Item>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Item {
    Function(FunctionDef),
    TypeDef(TypeDef),
    Use(UseDecl),
}

#[derive(Debug, Clone)]
pub struct UseDecl {
    pub name: Ident,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypeDef {
    pub name: Ident,
    pub generic_params: Vec<GenericParam>,
    pub body: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct GenericParam {
    pub name: Ident,
    pub bound: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub receiver: Option<Ident>,
    pub receiver_mut: bool,
    pub name: Ident,
    pub generic_params: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub return_ty: TypeExpr,
    pub body: Block,
    pub extern_rust: Option<ExternRust>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ExternRust {
    pub path: String,
    pub is_async: bool,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub ty: TypeExpr,
    pub mutable: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TypeExpr {
    Named {
        name: String,
        generics: Vec<TypeExpr>,
        span: Span,
    },
    Union {
        variants: Vec<TypeExpr>,
        span: Span,
    },
    Product {
        fields: Vec<TypeExpr>,
        span: Span,
    },
    Repeat {
        ty: Box<TypeExpr>,
        count: u64,
        span: Span,
    },
    Spread {
        ty: Box<TypeExpr>,
        span: Span,
    },
    Function {
        generic_params: Vec<GenericParam>,
        params: Vec<TypeExpr>,
        return_ty: Box<TypeExpr>,
        span: Span,
    },
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Named { span, .. } => *span,
            TypeExpr::Union { span, .. } => *span,
            TypeExpr::Product { span, .. } => *span,
            TypeExpr::Repeat { span, .. } => *span,
            TypeExpr::Spread { span, .. } => *span,
            TypeExpr::Function { span, .. } => *span,
        }
    }

    pub fn simple_name(&self) -> Option<&str> {
        if let TypeExpr::Named { name, generics, .. } = self {
            if generics.is_empty() {
                return Some(name);
            }
        }
        None
    }
}

#[derive(Debug, Clone)]
pub struct Block {
    pub exprs: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Ident(Ident),
    StringLit {
        value: String,
        span: Span,
    },
    IntLit {
        value: i64,
        span: Span,
    },
    FloatLit {
        value: f64,
        span: Span,
    },
    HexLit {
        value: u64,
        span: Span,
    },
    Constructor {
        name: Ident,
        args: Vec<Expr>,
        span: Span,
    },
    MethodCall {
        receiver: Box<Expr>,
        method: Ident,
        type_args: Vec<TypeExpr>,
        args: Vec<Expr>,
        span: Span,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    Try {
        inner: Box<Expr>,
        span: Span,
    },
    Lambda {
        params: Vec<Param>,
        return_ty: TypeExpr,
        body: Block,
        span: Span,
    },
    ProductValue {
        fields: Vec<Expr>,
        span: Span,
    },
    FieldAccess {
        receiver: Box<Expr>,
        field: Ident,
        span: Span,
    },
    JsonLit {
        value: String,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Ident(ident) => ident.span,
            Expr::StringLit { span, .. } => *span,
            Expr::IntLit { span, .. } => *span,
            Expr::FloatLit { span, .. } => *span,
            Expr::HexLit { span, .. } => *span,
            Expr::Constructor { span, .. } => *span,
            Expr::MethodCall { span, .. } => *span,
            Expr::Match { span, .. } => *span,
            Expr::Try { span, .. } => *span,
            Expr::Lambda { span, .. } => *span,
            Expr::ProductValue { span, .. } => *span,
            Expr::FieldAccess { span, .. } => *span,
            Expr::JsonLit { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub param_ty: TypeExpr, // variant type, e.g. Err<String>, Ok<Int>, Branch, Leaf
    pub return_ty: TypeExpr, // arm return type (same across all arms)
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// Extract the receiver type from the first component of a parameter list.
/// In the new syntax `name = (A * B * C) -> ...`, A is the receiver and B, C are params.
/// For a single param `name = (A) -> ...`, A is the receiver with no extra params.
/// Returns `(receiver_name, receiver_mut, remaining_params)`.
pub fn extract_receiver_from_params(params: Vec<Param>) -> (Option<Ident>, bool, Vec<Param>) {
    if params.is_empty() {
        return (None, false, params);
    }

    let mut param_iter = params.into_iter();
    let first_param = param_iter.next().unwrap();
    let outer_mut = first_param.mutable;
    let remaining_original: Vec<Param> = param_iter.collect();

    match first_param.ty {
        TypeExpr::Product { fields, .. } => {
            // `mut` written outside the parens of a product param marks the
            // first component (the receiver) as mutable.
            if let Some(first_field) = fields.first() {
                let recv = match first_field {
                    TypeExpr::Named { name, span, .. } => Some(Ident {
                        name: name.clone(),
                        span: *span,
                    }),
                    _ => None,
                };
                let mut remaining: Vec<Param> = fields[1..]
                    .iter()
                    .map(|f| Param {
                        ty: f.clone(),
                        mutable: false,
                        span: f.span(),
                    })
                    .collect();
                remaining.extend(remaining_original);
                (recv, outer_mut, remaining)
            } else {
                (None, false, remaining_original)
            }
        }
        TypeExpr::Named { ref name, span, .. } => {
            let recv = Some(Ident {
                name: name.clone(),
                span,
            });
            (recv, outer_mut, remaining_original)
        }
        _ => {
            let mut result = vec![first_param];
            result.extend(remaining_original);
            (None, false, result)
        }
    }
}

/// Post-parse transformation: resolve PascalCase function definitions.
/// - If the name matches a TypeDef in the same module → it's a validated constructor
///   (set receiver = type name, rename to "Self")
/// - Otherwise → it's a trait implementation
///   (extract first param as receiver)
pub fn resolve_new_syntax(module: &mut Module) {
    let type_names: HashSet<String> = module
        .items
        .iter()
        .filter_map(|item| {
            if let Item::TypeDef(td) = item {
                // Skip trait definitions (function types) — they're not constructible types
                if matches!(td.body, TypeExpr::Function { .. }) {
                    return None;
                }
                Some(td.name.name.clone())
            } else {
                None
            }
        })
        .collect();

    for item in &mut module.items {
        if let Item::Function(func) = item {
            if func.receiver.is_none() && func.name.name != "main" {
                let is_pascal = func
                    .name
                    .name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false);
                if is_pascal {
                    if type_names.contains(&func.name.name) {
                        // Constructor (including zero-arg): set receiver to type name, rename to "Self"
                        let recv_name = func.name.name.clone();
                        let recv_span = func.name.span;
                        func.receiver = Some(Ident {
                            name: recv_name,
                            span: recv_span,
                        });
                        func.name = Ident {
                            name: "Self".to_string(),
                            span: func.name.span,
                        };
                    } else if !func.params.is_empty() {
                        // Trait impl: extract first component as receiver
                        let old_params = std::mem::take(&mut func.params);
                        let (receiver, recv_mut, new_params) =
                            extract_receiver_from_params(old_params);
                        func.receiver = receiver;
                        func.receiver_mut = recv_mut;
                        func.params = new_params;
                    }
                }
            }
        }
    }
}
