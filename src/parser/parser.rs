use crate::ast::*;
use crate::error::{OnewayError, Result, Span};
use crate::lexer::{Token, TokenKind};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub fn parse(&mut self) -> Result<Module> {
        let start = self.current_span();
        let mut items = Vec::new();

        self.skip_newlines();
        while !self.is_at_end() {
            items.push(self.parse_item()?);
            self.skip_newlines();
        }

        let end = self.previous_span();
        Ok(Module {
            items,
            span: span_join(start, end),
        })
    }

    fn parse_item(&mut self) -> Result<Item> {
        let start_span = self.current_span();

        if self.check(TokenKind::KwUse) {
            self.advance();
            let name_tok = self.expect(TokenKind::Ident, "expected module name after `use`")?;
            return Ok(Item::Use(UseDecl {
                name: Ident {
                    name: name_tok.lexeme.clone(),
                    span: name_tok.span,
                },
                span: span_join(start_span, name_tok.span),
            }));
        }

        let extern_decl = if self.check(TokenKind::KwExtern) {
            self.advance();
            let lang_tok = self.expect(TokenKind::Ident, "expected language after `extern`")?;
            if lang_tok.lexeme != "Rust" {
                return Err(OnewayError::ParseError {
                    message: format!(
                        "only `extern Rust` is supported (got `extern {}`)",
                        lang_tok.lexeme
                    ),
                    span: lang_tok.span,
                });
            }
            let mut is_async = false;
            if self.check(TokenKind::Dot) {
                self.advance();
                let qualifier_tok =
                    self.expect(TokenKind::Ident, "expected `async` after `extern Rust.`")?;
                if qualifier_tok.lexeme != "async" {
                    return Err(OnewayError::ParseError {
                        message: format!(
                            "only `extern Rust.async` is supported (got `extern Rust.{}`)",
                            qualifier_tok.lexeme
                        ),
                        span: qualifier_tok.span,
                    });
                }
                is_async = true;
            }
            self.expect(TokenKind::LParen, "expected `(` after `extern Rust`")?;
            let path_tok = self.expect(
                TokenKind::StringLit,
                "expected a Rust path string after `extern Rust(`",
            )?;
            self.expect(TokenKind::RParen, "expected `)` after Rust path")?;
            self.skip_newlines();
            Some(ExternRust {
                path: path_tok.lexeme,
                is_async,
            })
        } else {
            None
        };

        let first = self.expect(TokenKind::Ident, "expected a top-level definition")?;
        let first_ident = Ident {
            name: first.lexeme.clone(),
            span: first.span,
        };

        if let Some(extern_decl) = extern_decl {
            return self.parse_extern_item(start_span, first_ident, extern_decl);
        }

        if self.check(TokenKind::Dot) {
            self.advance();
            let name_tok = self.expect(TokenKind::Ident, "expected function name after `.`")?;
            let name = Ident {
                name: name_tok.lexeme.clone(),
                span: name_tok.span,
            };
            return self.parse_function_after_name(Some(first_ident), name, start_span);
        }

        let pre_eq_generics = if self.check(TokenKind::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::Eq, "expected `=` in top-level definition")?;

        if self.check(TokenKind::LParen) || self.check(TokenKind::Lt) {
            if !pre_eq_generics.is_empty() {
                return Err(OnewayError::ParseError {
                    message:
                        "generic parameters on function definitions go after `=`, not before"
                            .to_string(),
                    span: first_ident.span,
                });
            }
            return self.parse_function_after_eq(None, first_ident, start_span);
        }

        let body = self.parse_type_expr()?;
        let end_span = self.previous_span();
        Ok(Item::TypeDef(TypeDef {
            name: first_ident,
            generic_params: pre_eq_generics,
            body,
            span: span_join(start_span, end_span),
        }))
    }

    fn parse_function_after_name(
        &mut self,
        receiver: Option<Ident>,
        name: Ident,
        start_span: Span,
    ) -> Result<Item> {
        self.expect(TokenKind::Eq, "expected `=` after function name")?;
        self.parse_function_after_eq(receiver, name, start_span)
    }

    fn parse_extern_item(
        &mut self,
        start_span: Span,
        first_ident: Ident,
        extern_decl: ExternRust,
    ) -> Result<Item> {
        if self.check(TokenKind::Dot) {
            self.advance();
            let name_tok = self.expect(TokenKind::Ident, "expected method name after `.`")?;
            let name = Ident {
                name: name_tok.lexeme.clone(),
                span: name_tok.span,
            };
            self.expect(TokenKind::Eq, "expected `=` after extern method name")?;
            let generic_params = if self.check(TokenKind::Lt) {
                self.parse_generic_params()?
            } else {
                Vec::new()
            };
            self.expect(TokenKind::LParen, "expected `(` to begin parameter list")?;
            let mut params = Vec::new();
            if !self.check(TokenKind::RParen) {
                loop {
                    params.push(self.parse_param()?);
                    if self.check(TokenKind::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
            self.expect(TokenKind::RParen, "expected `)` to close parameter list")?;
            self.expect(TokenKind::Arrow, "expected `->` before return type")?;
            let return_ty = self.parse_type_expr()?;
            let end_span = self.previous_span();
            let empty_body_span = Span::new(end_span.end, end_span.end, end_span.line, end_span.column);
            return Ok(Item::Function(FunctionDef {
                receiver: Some(first_ident),
                name,
                generic_params,
                params,
                return_ty,
                body: Block {
                    exprs: Vec::new(),
                    span: empty_body_span,
                },
                extern_rust: Some(extern_decl),
                span: span_join(start_span, end_span),
            }));
        }

        if extern_decl.is_async {
            return Err(OnewayError::ParseError {
                message: "`extern Rust.async` is only valid on method declarations, not on types"
                    .to_string(),
                span: first_ident.span,
            });
        }

        if !self.check(TokenKind::Eq) {
            let end_span = first_ident.span;
            return Ok(Item::TypeDef(TypeDef {
                name: first_ident.clone(),
                generic_params: Vec::new(),
                body: TypeExpr::Named {
                    name: format!("__extern__{}", extern_decl.path),
                    generics: Vec::new(),
                    span: end_span,
                },
                span: span_join(start_span, end_span),
            }));
        }

        self.advance();
        let body = self.parse_type_expr()?;
        let end_span = self.previous_span();
        Ok(Item::TypeDef(TypeDef {
            name: first_ident,
            generic_params: Vec::new(),
            body,
            span: span_join(start_span, end_span),
        }))
    }

    fn parse_function_after_eq(
        &mut self,
        receiver: Option<Ident>,
        name: Ident,
        start_span: Span,
    ) -> Result<Item> {
        let generic_params = if self.check(TokenKind::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::LParen, "expected `(` to begin parameter list")?;
        let mut params = Vec::new();
        if !self.check(TokenKind::RParen) {
            loop {
                params.push(self.parse_param()?);
                if self.check(TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect(TokenKind::RParen, "expected `)` to close parameter list")?;
        self.expect(TokenKind::Arrow, "expected `->` before return type")?;
        let return_ty = self.parse_type_expr()?;

        if self.check(TokenKind::LBrace) {
            let body = self.parse_block()?;
            let end_span = self.previous_span();
            return Ok(Item::Function(FunctionDef {
                receiver,
                name,
                generic_params,
                params,
                return_ty,
                body,
                extern_rust: None,
                span: span_join(start_span, end_span),
            }));
        }

        if receiver.is_some() {
            return Err(OnewayError::ParseError {
                message: "method definition requires a body `{ ... }`".to_string(),
                span: self.peek().span,
            });
        }

        let end_span = self.previous_span();
        let func_ty_span = span_join(start_span, end_span);
        let function_ty = TypeExpr::Function {
            generic_params,
            params: params.into_iter().map(|p| p.ty).collect(),
            return_ty: Box::new(return_ty),
            span: func_ty_span,
        };
        Ok(Item::TypeDef(TypeDef {
            name,
            generic_params: Vec::new(),
            body: function_ty,
            span: func_ty_span,
        }))
    }

    fn parse_generic_params(&mut self) -> Result<Vec<GenericParam>> {
        self.expect(TokenKind::Lt, "expected `<` to begin generic parameters")?;
        let mut params = Vec::new();
        if !self.check(TokenKind::Gt) {
            loop {
                let start = self.current_span();
                let name_tok =
                    self.expect(TokenKind::Ident, "expected generic parameter name")?;
                let name = Ident {
                    name: name_tok.lexeme.clone(),
                    span: name_tok.span,
                };
                let bound = if self.check(TokenKind::Colon) {
                    self.advance();
                    Some(self.parse_type_expr()?)
                } else {
                    None
                };
                let end = self.previous_span();
                params.push(GenericParam {
                    name,
                    bound,
                    span: span_join(start, end),
                });
                if self.check(TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect(TokenKind::Gt, "expected `>` to close generic parameters")?;
        Ok(params)
    }

    fn parse_param(&mut self) -> Result<Param> {
        let start = self.current_span();
        let mutable = if self.check(TokenKind::KwMut) {
            self.advance();
            true
        } else {
            false
        };
        let ty = self.parse_type_expr()?;
        let end = self.previous_span();
        Ok(Param {
            ty,
            mutable,
            span: span_join(start, end),
        })
    }

    fn parse_type_expr(&mut self) -> Result<TypeExpr> {
        self.parse_type_union()
    }

    fn parse_type_union(&mut self) -> Result<TypeExpr> {
        let start = self.current_span();
        let first = self.parse_type_product()?;
        if !self.check(TokenKind::Pipe) {
            return Ok(first);
        }
        let mut variants = vec![first];
        while self.check(TokenKind::Pipe) {
            self.advance();
            self.skip_newlines();
            variants.push(self.parse_type_product()?);
        }
        let end = self.previous_span();
        Ok(TypeExpr::Union {
            variants,
            span: span_join(start, end),
        })
    }

    fn parse_type_product(&mut self) -> Result<TypeExpr> {
        let start = self.current_span();
        let first = self.parse_type_spread_or_postfix()?;
        if !self.check(TokenKind::Amp) {
            return Ok(first);
        }
        let mut fields = vec![first];
        while self.check(TokenKind::Amp) {
            self.advance();
            self.skip_newlines();
            fields.push(self.parse_type_spread_or_postfix()?);
        }
        let end = self.previous_span();
        Ok(TypeExpr::Product {
            fields,
            span: span_join(start, end),
        })
    }

    fn parse_type_spread_or_postfix(&mut self) -> Result<TypeExpr> {
        let start = self.current_span();
        if self.check(TokenKind::Ellipsis) {
            self.advance();
            let ty = self.parse_type_postfix_atom()?;
            let end = self.previous_span();
            return Ok(TypeExpr::Spread {
                ty: Box::new(ty),
                span: span_join(start, end),
            });
        }
        self.parse_type_postfix_atom()
    }

    fn parse_type_postfix_atom(&mut self) -> Result<TypeExpr> {
        let start = self.current_span();
        let atom = self.parse_type_atom()?;
        if !self.check(TokenKind::LBracket) {
            return Ok(atom);
        }
        self.advance();
        let count_tok = self.expect(TokenKind::IntLit, "expected an integer count in `[N]`")?;
        let count: u64 = count_tok.lexeme.parse().map_err(|_| OnewayError::ParseError {
            message: format!("invalid integer `{}` in repetition count", count_tok.lexeme),
            span: count_tok.span,
        })?;
        self.expect(TokenKind::RBracket, "expected `]` after repetition count")?;
        let end = self.previous_span();
        Ok(TypeExpr::Repeat {
            ty: Box::new(atom),
            count,
            span: span_join(start, end),
        })
    }

    fn parse_type_atom(&mut self) -> Result<TypeExpr> {
        let start = self.current_span();
        let name_tok = if self.check(TokenKind::KwSelf) {
            self.advance().clone()
        } else {
            self.expect(TokenKind::Ident, "expected a type name")?
        };
        let name = name_tok.lexeme.clone();

        let mut generics = Vec::new();
        if self.check(TokenKind::Lt) {
            self.advance();
            if !self.check(TokenKind::Gt) {
                loop {
                    generics.push(self.parse_type_expr()?);
                    if self.check(TokenKind::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
            self.expect(TokenKind::Gt, "expected `>` to close generic application")?;
        }

        let end = self.previous_span();
        Ok(TypeExpr::Named {
            name,
            generics,
            span: span_join(start, end),
        })
    }

    fn parse_block(&mut self) -> Result<Block> {
        let lbrace = self.expect(TokenKind::LBrace, "expected `{` to begin block")?;
        let start = lbrace.span;

        let mut exprs = Vec::new();
        self.skip_newlines();
        while !self.check(TokenKind::RBrace) && !self.is_at_end() {
            exprs.push(self.parse_expr()?);
            self.skip_newlines();
        }
        let rbrace = self.expect(TokenKind::RBrace, "expected `}` to close block")?;

        Ok(Block {
            exprs,
            span: span_join(start, rbrace.span),
        })
    }

    fn parse_expr(&mut self) -> Result<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            let next = self.peek_past_newlines();
            if matches!(next, TokenKind::Dot | TokenKind::Question) {
                self.skip_newlines();
            }
            if self.check(TokenKind::Dot) {
                self.advance();
                let method_tok =
                    self.expect(TokenKind::Ident, "expected method name after `.`")?;
                let method = Ident {
                    name: method_tok.lexeme.clone(),
                    span: method_tok.span,
                };
                self.expect(TokenKind::LParen, "expected `(` after method name")?;
                let mut args = Vec::new();
                if !self.check(TokenKind::RParen) {
                    loop {
                        args.push(self.parse_expr()?);
                        if self.check(TokenKind::Comma) {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                }
                let rparen =
                    self.expect(TokenKind::RParen, "expected `)` to close method call")?;
                let start_span = expr.span();
                expr = Expr::MethodCall {
                    receiver: Box::new(expr),
                    method,
                    args,
                    span: span_join(start_span, rparen.span),
                };
            } else if self.check(TokenKind::Question) {
                let q = self.advance().clone();
                let start_span = expr.span();
                expr = Expr::Try {
                    inner: Box::new(expr),
                    span: span_join(start_span, q.span),
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::KwMatch => self.parse_match(),
            TokenKind::KwWhile => self.parse_while(),
            TokenKind::LParen => self.parse_lambda(),
            TokenKind::Ident | TokenKind::KwSelf => {
                self.advance();
                if self.check(TokenKind::LParen) {
                    self.advance();
                    let mut args = Vec::new();
                    if !self.check(TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.check(TokenKind::Comma) {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                    }
                    let rparen =
                        self.expect(TokenKind::RParen, "expected `)` to close constructor")?;
                    return Ok(Expr::Constructor {
                        name: Ident {
                            name: tok.lexeme,
                            span: tok.span,
                        },
                        args,
                        span: span_join(tok.span, rparen.span),
                    });
                }
                Ok(Expr::Ident(Ident {
                    name: tok.lexeme,
                    span: tok.span,
                }))
            }
            TokenKind::StringLit => {
                self.advance();
                Ok(Expr::StringLit {
                    value: tok.lexeme,
                    span: tok.span,
                })
            }
            TokenKind::IntLit => {
                self.advance();
                let value: i64 = tok.lexeme.parse().map_err(|_| OnewayError::ParseError {
                    message: format!("invalid integer literal `{}`", tok.lexeme),
                    span: tok.span,
                })?;
                Ok(Expr::IntLit {
                    value,
                    span: tok.span,
                })
            }
            TokenKind::FloatLit => {
                self.advance();
                let value: f64 = tok.lexeme.parse().map_err(|_| OnewayError::ParseError {
                    message: format!("invalid float literal `{}`", tok.lexeme),
                    span: tok.span,
                })?;
                Ok(Expr::FloatLit {
                    value,
                    span: tok.span,
                })
            }
            TokenKind::HexLit => {
                self.advance();
                let stripped = tok.lexeme.trim_start_matches("0x");
                let value = u64::from_str_radix(stripped, 16).map_err(|_| {
                    OnewayError::ParseError {
                        message: format!("invalid hex literal `{}`", tok.lexeme),
                        span: tok.span,
                    }
                })?;
                Ok(Expr::HexLit {
                    value,
                    span: tok.span,
                })
            }
            _ => Err(OnewayError::ParseError {
                message: format!("expected an expression (got {})", tok.kind),
                span: tok.span,
            }),
        }
    }

    fn parse_lambda(&mut self) -> Result<Expr> {
        let lparen = self.expect(TokenKind::LParen, "expected `(` to begin lambda")?;
        let mut params = Vec::new();
        if !self.check(TokenKind::RParen) {
            loop {
                params.push(self.parse_param()?);
                if self.check(TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect(TokenKind::RParen, "expected `)` to close lambda parameter list")?;
        self.expect(TokenKind::Arrow, "expected `->` after lambda parameter list")?;
        let return_ty = self.parse_type_expr()?;
        let body = self.parse_block()?;
        Ok(Expr::Lambda {
            params,
            return_ty,
            body,
            span: span_join(lparen.span, self.previous_span()),
        })
    }

    fn parse_while(&mut self) -> Result<Expr> {
        let kw = self.expect(TokenKind::KwWhile, "expected `while`")?;
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(Expr::While {
            cond: Box::new(cond),
            body,
            span: span_join(kw.span, self.previous_span()),
        })
    }

    fn parse_match(&mut self) -> Result<Expr> {
        let kw = self.expect(TokenKind::KwMatch, "expected `match`")?;
        let scrutinee = self.parse_expr()?;
        self.expect(TokenKind::LBrace, "expected `{` to begin match arms")?;
        self.skip_newlines();

        let mut arms = Vec::new();
        while !self.check(TokenKind::RBrace) && !self.is_at_end() {
            arms.push(self.parse_match_arm()?);
            if self.check(TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        let rbrace = self.expect(TokenKind::RBrace, "expected `}` to close match")?;

        Ok(Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms,
            span: span_join(kw.span, rbrace.span),
        })
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm> {
        let pattern = self.parse_pattern()?;
        self.expect(TokenKind::FatArrow, "expected `=>` after match pattern")?;
        let body = self.parse_expr()?;
        let arm_span = span_join(pattern.span(), body.span());
        Ok(MatchArm {
            pattern,
            body,
            span: arm_span,
        })
    }

    fn parse_pattern(&mut self) -> Result<Pattern> {
        let tok = self.expect(TokenKind::Ident, "expected a pattern (variant name or `_`)")?;
        if tok.lexeme == "_" {
            return Ok(Pattern::Wildcard { span: tok.span });
        }
        let mut args = Vec::new();
        let mut end_span = tok.span;
        if self.check(TokenKind::LParen) {
            self.advance();
            if !self.check(TokenKind::RParen) {
                loop {
                    args.push(self.parse_pattern()?);
                    if self.check(TokenKind::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
            let rparen = self.expect(TokenKind::RParen, "expected `)` to close pattern")?;
            end_span = rparen.span;
        }
        Ok(Pattern::Variant {
            name: tok.lexeme,
            args,
            span: span_join(tok.span, end_span),
        })
    }

    fn skip_newlines(&mut self) {
        while self.check(TokenKind::Newline) {
            self.advance();
        }
    }

    fn peek_past_newlines(&self) -> TokenKind {
        let mut i = self.pos;
        while i < self.tokens.len() && self.tokens[i].kind == TokenKind::Newline {
            i += 1;
        }
        if i < self.tokens.len() {
            self.tokens[i].kind
        } else {
            TokenKind::Eof
        }
    }

    fn check(&self, kind: TokenKind) -> bool {
        self.peek().kind == kind
    }

    fn advance(&mut self) -> &Token {
        if !self.is_at_end() {
            self.pos += 1;
        }
        &self.tokens[self.pos - 1]
    }

    fn expect(&mut self, kind: TokenKind, msg: &str) -> Result<Token> {
        if self.check(kind) {
            Ok(self.advance().clone())
        } else {
            let actual = self.peek().clone();
            Err(OnewayError::ParseError {
                message: format!("{} (got {})", msg, actual.kind),
                span: actual.span,
            })
        }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn previous_span(&self) -> Span {
        if self.pos == 0 {
            self.tokens[0].span
        } else {
            self.tokens[self.pos - 1].span
        }
    }

    fn current_span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Eof)
    }
}

fn span_join(a: Span, b: Span) -> Span {
    Span::new(a.start.min(b.start), a.end.max(b.end), a.line, a.column)
}
