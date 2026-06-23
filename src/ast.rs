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
    /// A file-level `bindings "<urn>"` directive. Declares that all
    /// camelCase function-type aliases in this file are actually
    /// external bindings backed by `<urn>` in the WebAssembly Component
    /// canonical ABI. The loader rewrites those aliases into
    /// `FunctionDef`s with `extern_wasm` populated; without the
    /// directive, a bare `name = (P) -> R` stays a function-type alias
    /// (the existing language semantics).
    ///
    /// Emitted at the top of every `canon install`-generated file as a
    /// human-readable index of "what is this file bindings for?" so a
    /// reader sees `bindings "wasi:clocks/timezone@…"` and immediately
    /// knows where to look in the source WIT.
    Bindings(BindingsDecl),
}

/// A `bindings "<urn>"` directive at the top of a generated bindings
/// file. See [`Item::Bindings`].
#[derive(Debug, Clone, PartialEq)]
pub struct BindingsDecl {
    /// The WIT interface URN, e.g.
    /// `"wasi:clocks/timezone@0.3.0-rc-2026-03-15"`. Stored verbatim;
    /// the loader appends `#<fn-kebab>` per function.
    pub urn: String,
    pub span: Span,
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
    pub extern_wasm: Option<ExternWasm>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ExternWasm {
    /// WASI import path, e.g. `"wasi:filesystem/types@0.3.0-rc-2026-03-15#read-via-stream"`
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

/// One piece of a JSON literal expression — see `Expr::JsonLit`.
#[derive(Debug, Clone)]
pub enum JsonLitPart {
    /// A pre-encoded chunk of JSON text (e.g. `{"k":` or `,"k2":"hi"}`).
    /// Inlined verbatim into the output.
    Static(String),
    /// An interpolated Canon expression. Its runtime value is converted
    /// to JSON via `.ToJson()` and concatenated into the surrounding
    /// JSON text.
    Interp(Box<Expr>),
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
    /// A JSON object or array literal: `{"k": value, ...}` / `[v, ...]`.
    ///
    /// Compiled-out at parse time into an alternating list of `Static`
    /// (pre-encoded JSON text fragments) and `Interp` (arbitrary Canon
    /// expressions whose runtime values are `.ToJson()`-converted and
    /// concatenated into the surrounding scaffolding). When `parts`
    /// contains a single `Static`, the literal is fully constant and
    /// codegen lowers it to a single string-literal load; otherwise it
    /// emits a `String.concat` chain over the parts.
    JsonLit {
        parts: Vec<JsonLitPart>,
        span: Span,
    },
    /// Inserted by the checker when a `Future<T>` expression is used in a position
    /// that expects `T`. Never produced by the parser.
    Await {
        inner: Box<Expr>,
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
            Expr::Await { span, .. } => *span,
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

/// Which WASI world's primary export shape a function's return type
/// matches, if any. Used by both the parser (to suppress receiver
/// extraction for entry-shaped functions) and the checker (for the
/// entry-detection rule documented in `WASI-HTTP-HANDLER.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryWorld {
    /// `wasi:cli/command`, exporting `wasi:cli/run.run`.
    Cli,
    /// `wasi:http/service`, exporting `wasi:http/handler.handle`.
    Http,
}

/// Returns the WASI world whose primary export shape this return type
/// matches, or `None` if it's not a world-shape type.
///
/// Shape registry (matches the table in DESIGN.md §Entry Point):
///
/// | Return type                              | World |
/// |------------------------------------------|-------|
/// | `Unit`, `ExitCode`                       | Cli   |
/// | `Result<Unit, _>`, `Result<ExitCode, _>` | Cli   |
/// | `Response`                               | Http  |
/// | `Result<Response, _>`                    | Http  |
///
/// The unwrapping recurses through `Result` so wrapped and unwrapped
/// shapes both classify.
pub fn entry_world_of(ty: &TypeExpr) -> Option<EntryWorld> {
    match ty {
        TypeExpr::Named { name, generics, .. } if generics.is_empty() => match name.as_str() {
            "Unit" | "ExitCode" => Some(EntryWorld::Cli),
            "Response" => Some(EntryWorld::Http),
            _ => None,
        },
        TypeExpr::Named { name, generics, .. } if name == "Result" && !generics.is_empty() => {
            entry_world_of(&generics[0])
        }
        _ => None,
    }
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
                    .is_some_and(char::is_uppercase);
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
