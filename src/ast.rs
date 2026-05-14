use crate::error::Span;

#[derive(Debug, Clone)]
pub struct Module {
    pub items: Vec<Item>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Item {
    Function(FunctionDef),
}

#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub receiver: Option<Ident>,
    pub name: Ident,
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
pub struct TypeExpr {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub exprs: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Ident(Ident),
}

#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}
