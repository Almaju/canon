use crate::error::Span;

#[derive(Debug, Clone)]
pub struct Module {
    pub items: Vec<Item>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Item {
    Function(FunctionDef),
    TypeDef(TypeDef),
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
    pub name: Ident,
    pub generic_params: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub return_ty: TypeExpr,
    pub body: Block,
    pub span: Span,
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
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Named { span, .. } => *span,
            TypeExpr::Union { span, .. } => *span,
            TypeExpr::Product { span, .. } => *span,
            TypeExpr::Repeat { span, .. } => *span,
            TypeExpr::Spread { span, .. } => *span,
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
    MethodCall {
        receiver: Box<Expr>,
        method: Ident,
        args: Vec<Expr>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Ident(ident) => ident.span,
            Expr::StringLit { span, .. } => *span,
            Expr::MethodCall { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}
