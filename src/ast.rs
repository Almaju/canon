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
    /// Declared in the anonymous-arrow form `(A) -> B { … }`
    /// (the language spec, § Types-Only Canon): the constructor of its output type, with
    /// `name` synthesized from the constructed type. The flag exists so
    /// the formatter round-trips the arrow form instead of inventing a
    /// `B = ` prefix.
    pub anonymous: bool,
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

/// One piece of an HTML literal expression — see `Expr::HtmlLit`.
#[derive(Debug, Clone)]
pub enum HtmlLitPart {
    /// A raw chunk of HTML text (e.g. `<div>` or `</li></ul>`).
    /// Inlined verbatim into the output.
    Static(String),
    /// An interpolated Canon expression (`{expr}` in the literal). Its
    /// runtime value is converted to HTML via `.ToHtml()` — escaping
    /// for `String`/`Int`, identity for `Html` — and concatenated into
    /// the surrounding HTML text.
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
        /// Written in the pipe form `value -> Name(rest…)` — the third
        /// spelling of the commutative call (`Name(value, rest…)` /
        /// `value.Name(rest…)`), mirroring the declaration arrow
        /// `(A) -> B { … }` at the value level. Semantics are identical
        /// to the dot form; the flag only preserves the surface
        /// spelling for the formatter.
        piped: bool,
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
    /// An HTML element literal: `<div>hello {Model}</div>`.
    ///
    /// Lexed as raw text (an HTML literal starts at a `<` immediately
    /// followed by a lowercase tag name — a position where `<` is never
    /// valid Canon) and compiled-out at parse time into an alternating
    /// list of `Static` (raw HTML text fragments) and `Interp`
    /// (arbitrary Canon expressions in `{…}` holes whose runtime values
    /// are `.ToHtml()`-converted and concatenated into the surrounding
    /// markup). When `parts` is a single `Static`, the literal is fully
    /// constant and codegen lowers it to one string-literal load;
    /// otherwise it emits a `String.concat` chain over the parts.
    HtmlLit {
        parts: Vec<HtmlLitPart>,
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
            Expr::HtmlLit { span, .. } => *span,
            Expr::Await { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub param_ty: TypeExpr, // variant type, e.g. Err<String>, Ok<Int>, Branch, Leaf
    /// `Some` for a literal-pattern arm (`* ("/notes") -> …` /
    /// `* (404) -> …`) dispatching on a `String` / `Int` scrutinee.
    /// When set, `param_ty` holds the scrutinee's primitive type name
    /// (`String` / `Int`) so type-shaped consumers stay well-formed;
    /// the literal is what the arm actually matches. A `None` arm in a
    /// literal dispatch is the mandatory catch-all.
    pub literal: Option<ArmLiteral>,
    pub return_ty: TypeExpr, // arm return type (same across all arms)
    pub body: Block,
    pub span: Span,
}

/// A literal pattern in a dispatch arm. Equality-dispatch is limited to
/// the two primitive kinds with unambiguous literal syntax.
#[derive(Debug, Clone, PartialEq)]
pub enum ArmLiteral {
    Int(i64),
    Str(String),
}

#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// Which WASI world's primary export shape a function's return type
/// matches, if any. Used by both the parser (to suppress receiver
/// extraction for entry-shaped functions) and the checker (for the
/// entry-detection rule documented in the entry-point rule (docs/src/spec/functions.md)).
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
/// Shape registry (matches the table in the language spec (docs/src/spec/)):
///
/// | Return type                                 | World |
/// |---------------------------------------------|-------|
/// | `Program`, `Unit`, `ExitCode`               | Cli   |
/// | `Result<Program, _>` (and `Unit`/`ExitCode`)| Cli   |
/// | `Response`                                  | Http  |
/// | `Result<Response, _>`                       | Http  |
///
/// `Program` (`= Unit`, from `canon/std`) is the canonical CLI world
/// type — the entry is `Unit => Program`, mirroring the HTTP entry's
/// `Request => Response`. `Unit`/`ExitCode` stay accepted so the legacy
/// `main` and the `canon test`-synthesized entry still classify.
///
/// The unwrapping recurses through `Result` so wrapped and unwrapped
/// shapes both classify.
pub fn entry_world_of(ty: &TypeExpr) -> Option<EntryWorld> {
    match ty {
        TypeExpr::Named { name, generics, .. } if generics.is_empty() => match name.as_str() {
            "Program" | "Unit" | "ExitCode" => Some(EntryWorld::Cli),
            "Response" => Some(EntryWorld::Http),
            _ => None,
        },
        TypeExpr::Named { name, generics, .. } if name == "Result" && !generics.is_empty() => {
            entry_world_of(&generics[0])
        }
        _ => None,
    }
}

/// The Elm-architecture entry triple that makes a program a web app
/// (see the web target, docs/src/reference/web-target.md): a free `init = () -> Model`, an
/// `update = (Model * String) -> Model`, and a `view = (Model) -> Html`.
/// `Model` is the user's own type; `Html` comes from `canon/std/web`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebEntry {
    /// The model type name — `view`'s receiver as written in source.
    pub model: String,
}

/// Detects the web-app entry triple among `items`. Unlike the HTTP
/// world, the web world can't key on a return type alone — every view
/// helper returns `Html` — so detection keys on the conventional
/// names `init` / `update` / `view` plus their shapes:
///
///   - `view`: a method (receiver = the model type) with no extra
///     params returning `Html`,
///   - `init`: a free zero-param function,
///   - `update`: a method on the same receiver type as `view` with
///     exactly one `String` param (the message).
///
/// Returns `None` unless all three line up.
pub fn find_web_entry(items: &[Item]) -> Option<WebEntry> {
    let funcs = |name: &'static str| {
        items.iter().filter_map(move |item| match item {
            Item::Function(f) if f.name.name == name && f.extern_wasm.is_none() => Some(f),
            _ => None,
        })
    };
    let view = funcs("view").find(|f| {
        f.receiver.is_some()
            && f.params.is_empty()
            && matches!(&f.return_ty, TypeExpr::Named { name, generics, .. }
                        if name == "Html" && generics.is_empty())
    })?;
    let model = view.receiver.as_ref()?.name.clone();
    funcs("init").find(|f| {
        f.receiver.is_none()
            && f.params.is_empty()
            && matches!(&f.return_ty, TypeExpr::Named { name, .. } if *name == model)
    })?;
    funcs("update").find(|f| {
        f.receiver.as_ref().map(|r| r.name.as_str()) == Some(model.as_str())
            && f.params.len() == 1
            && matches!(&f.params[0].ty, TypeExpr::Named { name, generics, .. }
                        if name == "String" && generics.is_empty())
            && matches!(&f.return_ty, TypeExpr::Named { name, .. } if *name == model)
    })?;
    Some(WebEntry { model })
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

/// Canonical spelling of a type expression, ignoring spans — the
/// language's type-equality story is *syntactic* (alphabetical order
/// gives every type exactly one spelling), so two definitions are the
/// same type iff their canonical spellings match. Used to merge
/// structurally identical duplicate type definitions across files
/// (`Length = Int` declared by both map.can and set.can is one type,
/// not a clash — the language spec, § Types-Only Canon).
pub fn type_expr_canonical(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named { name, generics, .. } => {
            if generics.is_empty() {
                name.clone()
            } else {
                let gs: Vec<String> = generics.iter().map(type_expr_canonical).collect();
                format!("{}<{}>", name, gs.join(", "))
            }
        }
        TypeExpr::Union { variants, .. } => {
            let vs: Vec<String> = variants.iter().map(type_expr_canonical).collect();
            vs.join(" + ")
        }
        TypeExpr::Product { fields, .. } => {
            let fs: Vec<String> = fields.iter().map(type_expr_canonical).collect();
            fs.join(" * ")
        }
        TypeExpr::Repeat { ty, count, .. } => format!("{}^{}", type_expr_canonical(ty), count),
        TypeExpr::Spread { ty, .. } => format!("{}^*", type_expr_canonical(ty)),
        TypeExpr::Function {
            generic_params,
            params,
            return_ty,
            ..
        } => {
            let gs = if generic_params.is_empty() {
                String::new()
            } else {
                let names: Vec<String> =
                    generic_params.iter().map(|g| g.name.name.clone()).collect();
                format!("<{}>", names.join(", "))
            };
            let ps: Vec<String> = params.iter().map(type_expr_canonical).collect();
            format!(
                "{}({}) -> {}",
                gs,
                ps.join(" * "),
                type_expr_canonical(return_ty)
            )
        }
    }
}

/// The PascalCase pipe/vocabulary spelling of a compiler builtin
/// method → its canonical (camelCase) implementation name. This is the
/// types-only surface for operations the compiler owns: `x -> Print`,
/// `1 -> Sum(2)`, `"a" -> Joined("b")`, `list -> Mapped(f)`. The alias
/// only resolves at the *builtin* layer — a user/stdlib function of the
/// same name is found first (func_table lookup precedes the builtin
/// fallback in both checker and codegen), so `map -> Length` still hits
/// the stdlib `Length` on `Map` while `"hi" -> Length` falls through to
/// the `String` builtin. camelCase spellings keep working during
/// migration; they simply don't pass through here.
pub fn builtin_method_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        // Effects
        "Print" => "print",
        // Int / Float arithmetic — result-type nouns
        "Sum" => "add",
        "Difference" => "sub",
        "Product" => "mul",
        "Quotient" => "div",
        "Remainder" => "rem",
        // Comparison — boolean predicates
        "Eq" => "eq",
        "Ne" => "ne",
        "Lt" => "lt",
        "Le" => "le",
        "Gt" => "gt",
        "Ge" => "ge",
        // String / List
        "Joined" => "concat",
        "ByteAt" => "byteAt",
        "Length" => "length",
        "Substring" => "substring",
        // List
        "Mapped" => "map",
        "First" => "first",
        "At" => "get",
        "Appended" => "append",
        // Concurrency combinators
        "Parallel" => "parallel",
        "Race" => "race",
        _ => return None,
    })
}

/// The PascalCase pipe spelling `canon fmt` emits for a builtin method
/// — the inverse of `builtin_method_alias`. `concat` prints as
/// `Joined`, `add` as `Sum`, `print` as `Print`. A name with no mapping
/// (already-PascalCase user/stdlib methods, and the `String`/`Json`
/// conversion methods) is emitted unchanged.
pub fn builtin_pipe_name(name: &str) -> &str {
    match name {
        "print" => "Print",
        "add" => "Sum",
        "sub" => "Difference",
        "mul" => "Product",
        "div" => "Quotient",
        "rem" | "mod" => "Remainder",
        "eq" => "Eq",
        "ne" => "Ne",
        "lt" => "Lt",
        "le" => "Le",
        "gt" => "Gt",
        "ge" => "Ge",
        "concat" => "Joined",
        "byteAt" => "ByteAt",
        "length" => "Length",
        "substring" | "slice" => "Substring",
        "map" => "Mapped",
        "first" => "First",
        "get" => "At",
        "append" => "Appended",
        "parallel" => "Parallel",
        "race" => "Race",
        other => other,
    }
}

/// The type an arrow *constructs*: its return type with the standard
/// containers peeled — `Result<Url, InvalidUrl>` constructs `Url`,
/// `Option<Value>` constructs `Value`, `Future<T>` constructs `T`.
/// `None` when the return type doesn't name a type (a bare function
/// type, a product, …), in which case an anonymous declaration has no
/// derivable identity and is rejected at parse time.
pub fn constructed_type_name(ty: &TypeExpr) -> Option<String> {
    match ty {
        TypeExpr::Named { name, generics, .. } => {
            if matches!(name.as_str(), "Result" | "Option" | "Future") && !generics.is_empty() {
                constructed_type_name(&generics[0])
            } else {
                Some(name.clone())
            }
        }
        _ => None,
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
            // A lone `Unit` input is the nullary case: `Unit => AddForm` ≡
            // `() => AddForm`, and `Unit => Program` ≡ the entry. `Unit`
            // is the single-value type — it carries no data and is
            // auto-supplied at call sites — so strip it to zero params and
            // let the zero-arg-constructor machinery take over unchanged
            // (`AddForm()` still calls it, the entry still renames to
            // `main` below).
            if func.anonymous && func.params.len() == 1 {
                if let TypeExpr::Named { name, generics, .. } = &func.params[0].ty {
                    if name == "Unit" && generics.is_empty() {
                        func.params.clear();
                    }
                }
            }
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
                        // A multi-input constructor parses its input product
                        // as ONE Product param (`Inserted = (Map * String *
                        // Value) -> …`). Trait impls split that product via
                        // `extract_receiver_from_params`; constructors keep
                        // no runtime receiver, so flatten it into one param
                        // per component here — downstream (wasm param
                        // lowering, local-scope registration, commutative
                        // keying) only understands `Named` params.
                        if func.params.len() == 1 {
                            if let TypeExpr::Product { fields, .. } = &func.params[0].ty {
                                func.params = fields
                                    .iter()
                                    .map(|f| Param {
                                        ty: f.clone(),
                                        mutable: false,
                                        span: f.span(),
                                    })
                                    .collect();
                            }
                        }
                    } else if func.anonymous
                        && entry_world_of(&func.return_ty) == Some(EntryWorld::Http)
                    {
                        // `(Request) -> Response { … }` — an anonymous HTTP
                        // entry. Mirror the parser's named-entry guard: keep
                        // it a free function so entry selection sees it,
                        // instead of extracting `Request` as a receiver.
                    } else if func.anonymous
                        && func.params.is_empty()
                        && entry_world_of(&func.return_ty) == Some(EntryWorld::Cli)
                    {
                        // `() => Unit { … }` — an anonymous CLI entry. The
                        // entry needs no name; it's selected by its
                        // world-shaped return like the HTTP handler. Rename
                        // to the canonical `main` so entry selection, the
                        // ordering exemption, and codegen's `$start`
                        // inlining all recognize it with zero other changes.
                        func.name = Ident {
                            name: "main".to_string(),
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
