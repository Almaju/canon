use crate::ast::extract_receiver_from_params;
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
            let first = self.expect(TokenKind::Ident, "expected module name after `use`")?;
            let mut path = first.lexeme.clone();
            let mut end_span = first.span;
            while self.check(TokenKind::Slash) {
                self.advance(); // consume /
                let seg = self.expect(
                    TokenKind::Ident,
                    "expected identifier after `/` in use path",
                )?;
                path.push('/');
                path.push_str(&seg.lexeme);
                end_span = seg.span;
            }
            return Ok(Item::Use(UseDecl {
                name: Ident {
                    name: path,
                    span: span_join(start_span, end_span),
                },
                span: span_join(start_span, end_span),
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

        let pre_eq_generics = if self.check(TokenKind::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::Eq, "expected `=` in top-level definition")?;

        if self.check(TokenKind::LParen) || self.check(TokenKind::Lt) {
            if !pre_eq_generics.is_empty() {
                return Err(OnewayError::ParseError {
                    message: "generic parameters on function definitions go after `=`, not before"
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

    fn parse_extern_item(
        &mut self,
        start_span: Span,
        first_ident: Ident,
        extern_decl: ExternRust,
    ) -> Result<Item> {
        // Check for = (function or type definition)
        if !self.check(TokenKind::Eq) {
            // No = → bare type: extern Rust("...") TypeName
            if extern_decl.is_async {
                return Err(OnewayError::ParseError {
                    message:
                        "`extern Rust.async` is only valid on function declarations, not on types"
                            .to_string(),
                    span: first_ident.span,
                });
            }
            // Parse optional generic params: e.g. HttpRouter<S>
            let generic_params = if self.check(TokenKind::Lt) {
                self.parse_generic_params()?
            } else {
                Vec::new()
            };
            let end_span = self.previous_span();
            return Ok(Item::TypeDef(TypeDef {
                name: first_ident.clone(),
                generic_params,
                body: TypeExpr::Named {
                    name: format!("__extern__{}", extern_decl.path),
                    generics: Vec::new(),
                    span: end_span,
                },
                span: span_join(start_span, end_span),
            }));
        }

        self.advance(); // consume =

        // After =, if ( or < → function signature (new syntax)
        if self.check(TokenKind::LParen) || self.check(TokenKind::Lt) {
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

            // Extract receiver for camelCase, defer PascalCase to post-parse
            let (receiver, recv_mut, final_params) = if Self::is_pascal_case_str(&first_ident.name)
            {
                (None, false, params)
            } else {
                extract_receiver_from_params(params)
            };

            let empty_body_span =
                Span::new(end_span.end, end_span.end, end_span.line, end_span.column);
            return Ok(Item::Function(FunctionDef {
                receiver,
                receiver_mut: recv_mut,
                name: first_ident,
                generic_params,
                params: final_params,
                return_ty,
                body: Block {
                    exprs: Vec::new(),
                    span: empty_body_span,
                },
                extern_rust: Some(extern_decl),
                span: span_join(start_span, end_span),
            }));
        }

        // After =, not a function → type definition
        if extern_decl.is_async {
            return Err(OnewayError::ParseError {
                message: "`extern Rust.async` is only valid on function declarations, not on types"
                    .to_string(),
                span: first_ident.span,
            });
        }
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

            // New syntax: extract receiver from first param component
            let (final_receiver, recv_mut, final_params) = if receiver.is_some() {
                // Old dot syntax — keep as-is
                (receiver, false, params)
            } else if name.name == "main" || params.is_empty() {
                // main or no-param function: no receiver
                (None, false, params)
            } else if Self::is_pascal_case_str(&name.name) {
                // PascalCase: defer to post-parse resolve_new_syntax
                (None, false, params)
            } else {
                // camelCase with params: extract first component as receiver
                extract_receiver_from_params(params)
            };

            return Ok(Item::Function(FunctionDef {
                receiver: final_receiver,
                receiver_mut: recv_mut,
                name,
                generic_params,
                params: final_params,
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
                let name_tok = self.expect(TokenKind::Ident, "expected generic parameter name")?;
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
        // Allow the `+` to appear on the next line so that multi-line union
        // type definitions (emitted by the formatter) round-trip correctly.
        if self.peek_past_newlines() != TokenKind::Plus {
            return Ok(first);
        }
        let mut variants = vec![first];
        while self.peek_past_newlines() == TokenKind::Plus {
            self.skip_newlines();
            self.advance(); // consume `+`
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
        let first = self.parse_type_postfix()?;
        if !self.check(TokenKind::Star) {
            return Ok(first);
        }
        let mut fields = vec![first];
        while self.check(TokenKind::Star) {
            self.advance();
            self.skip_newlines();
            fields.push(self.parse_type_postfix()?);
        }
        let end = self.previous_span();
        Ok(TypeExpr::Product {
            fields,
            span: span_join(start, end),
        })
    }

    fn parse_type_postfix(&mut self) -> Result<TypeExpr> {
        let start = self.current_span();
        let atom = self.parse_type_atom()?;
        if !self.check(TokenKind::Caret) {
            return Ok(atom);
        }
        self.advance();
        // ^* means unbounded repetition (Kleene star)
        if self.check(TokenKind::Star) {
            self.advance();
            let end = self.previous_span();
            return Ok(TypeExpr::Spread {
                ty: Box::new(atom),
                span: span_join(start, end),
            });
        }
        // ^N means fixed repetition
        let count_tok = self.expect(TokenKind::IntLit, "expected `*` or an integer after `^`")?;
        let count: u64 = count_tok
            .lexeme
            .parse()
            .map_err(|_| OnewayError::ParseError {
                message: format!("invalid integer `{}` in repetition count", count_tok.lexeme),
                span: count_tok.span,
            })?;
        let end = self.previous_span();
        Ok(TypeExpr::Repeat {
            ty: Box::new(atom),
            count,
            span: span_join(start, end),
        })
    }

    fn parse_type_atom(&mut self) -> Result<TypeExpr> {
        let start = self.current_span();

        // Function type: `(Params) -> Ret` or `<T>(Params) -> Ret`
        if self.check(TokenKind::Lt) || self.check(TokenKind::LParen) {
            let generic_params = if self.check(TokenKind::Lt) {
                self.parse_generic_params()?
            } else {
                Vec::new()
            };
            self.expect(TokenKind::LParen, "expected `(` to begin function type")?;
            let mut params = Vec::new();
            if !self.check(TokenKind::RParen) {
                loop {
                    params.push(self.parse_type_expr()?);
                    if self.check(TokenKind::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
            self.expect(TokenKind::RParen, "expected `)` to close function type")?;
            self.expect(TokenKind::Arrow, "expected `->` in function type")?;
            let return_ty = self.parse_type_postfix()?;
            let end = self.previous_span();
            return Ok(TypeExpr::Function {
                generic_params,
                params,
                return_ty: Box::new(return_ty),
                span: span_join(start, end),
            });
        }

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

    /// An argument-position expression: a normal expression possibly joined
    /// with `*` into a value-level product. Used inside `(...)` arg lists.
    fn parse_arg_expr(&mut self) -> Result<Expr> {
        let first = self.parse_expr()?;
        if !self.check(TokenKind::Star) {
            return Ok(first);
        }
        let start_span = first.span();
        let mut fields = vec![first];
        while self.check(TokenKind::Star) {
            self.advance();
            fields.push(self.parse_expr()?);
        }
        let end_span = self.previous_span();
        Ok(Expr::ProductValue {
            fields,
            span: span_join(start_span, end_span),
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
                // Dispatch syntax: value.( arms ) — desugars to match
                if self.check(TokenKind::LParen) {
                    self.advance();
                    let mut arms = Vec::new();
                    self.skip_newlines();
                    while !self.check(TokenKind::RParen) && !self.is_at_end() {
                        // Consume optional `*` separator before each arm (including the first)
                        if self.check(TokenKind::Star) {
                            self.advance();
                            self.skip_newlines();
                        }
                        if self.check(TokenKind::RParen) || self.is_at_end() {
                            break;
                        }
                        arms.push(self.parse_match_arm()?);
                        self.skip_newlines();
                    }
                    let rparen =
                        self.expect(TokenKind::RParen, "expected `)` to close dispatch")?;
                    let start_span = expr.span();
                    expr = Expr::Match {
                        scrutinee: Box::new(expr),
                        arms,
                        span: span_join(start_span, rparen.span),
                    };
                } else {
                    let name_tok =
                        self.expect(TokenKind::Ident, "expected method or field name after `.`")?;
                    let ident = Ident {
                        name: name_tok.lexeme.clone(),
                        span: name_tok.span,
                    };
                    // `value.X` with no `(` or `::` after is field access; with
                    // `(` or `::` it's a method call.
                    if !self.check(TokenKind::LParen) && !self.check(TokenKind::ColonColon) {
                        let start_span = expr.span();
                        expr = Expr::FieldAccess {
                            receiver: Box::new(expr),
                            field: ident,
                            span: span_join(start_span, name_tok.span),
                        };
                        continue;
                    }
                    let mut type_args = Vec::new();
                    if self.check(TokenKind::ColonColon) {
                        self.advance();
                        self.expect(TokenKind::Lt, "expected `<` after `::` in turbofish")?;
                        if !self.check(TokenKind::Gt) {
                            loop {
                                type_args.push(self.parse_type_expr()?);
                                if self.check(TokenKind::Comma) {
                                    self.advance();
                                } else {
                                    break;
                                }
                            }
                        }
                        self.expect(
                            TokenKind::Gt,
                            "expected `>` to close turbofish type arguments",
                        )?;
                    }
                    self.expect(TokenKind::LParen, "expected `(` after method name")?;
                    self.skip_newlines();
                    let mut args = Vec::new();
                    if !self.check(TokenKind::RParen) {
                        loop {
                            args.push(self.parse_arg_expr()?);
                            self.skip_newlines();
                            if self.check(TokenKind::Comma) {
                                self.advance();
                                self.skip_newlines();
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
                        method: ident,
                        type_args,
                        args,
                        span: span_join(start_span, rparen.span),
                    };
                }
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
            TokenKind::LParen => self.parse_lambda(),
            TokenKind::Ident | TokenKind::KwSelf => {
                self.advance();
                if self.check(TokenKind::LParen) {
                    self.advance();
                    self.skip_newlines();
                    let mut args = Vec::new();
                    if !self.check(TokenKind::RParen) {
                        loop {
                            args.push(self.parse_arg_expr()?);
                            self.skip_newlines();
                            if self.check(TokenKind::Comma) {
                                self.advance();
                                self.skip_newlines();
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
                let value =
                    u64::from_str_radix(stripped, 16).map_err(|_| OnewayError::ParseError {
                        message: format!("invalid hex literal `{}`", tok.lexeme),
                        span: tok.span,
                    })?;
                Ok(Expr::HexLit {
                    value,
                    span: tok.span,
                })
            }
            TokenKind::LBrace => {
                let open = tok.span;
                self.advance();
                let json = self.parse_json_object_body(open)?;
                let end = self.previous_span();
                Ok(Expr::JsonLit {
                    value: json,
                    span: span_join(open, end),
                })
            }
            TokenKind::LBracket => {
                let open = tok.span;
                self.advance();
                let json = self.parse_json_array_body(open)?;
                let end = self.previous_span();
                Ok(Expr::JsonLit {
                    value: json,
                    span: span_join(open, end),
                })
            }
            _ => Err(OnewayError::ParseError {
                message: format!("expected an expression (got {})", tok.kind),
                span: tok.span,
            }),
        }
    }

    fn parse_json_object_body(&mut self, open_span: Span) -> Result<String> {
        let mut out = String::from('{');
        self.skip_newlines();
        let mut first = true;
        while !self.check(TokenKind::RBrace) && !self.is_at_end() {
            if !first {
                self.expect(TokenKind::Comma, "expected `,` or `}` in JSON object")?;
                self.skip_newlines();
                if self.check(TokenKind::RBrace) {
                    break; // trailing comma
                }
                out.push(',');
            }
            first = false;
            let key_tok =
                self.expect(TokenKind::StringLit, "expected string key in JSON object")?;
            out.push_str(&json_encode_string(&key_tok.lexeme));
            self.skip_newlines();
            self.expect(TokenKind::Colon, "expected `:` after JSON key")?;
            self.skip_newlines();
            let val = self.parse_json_value(open_span)?;
            self.skip_newlines();
            out.push(':');
            out.push_str(&val);
        }
        self.expect(TokenKind::RBrace, "expected `}` to close JSON object")?;
        out.push('}');
        Ok(out)
    }

    fn parse_json_array_body(&mut self, open_span: Span) -> Result<String> {
        let mut out = String::from('[');
        self.skip_newlines();
        let mut first = true;
        while !self.check(TokenKind::RBracket) && !self.is_at_end() {
            if !first {
                self.expect(TokenKind::Comma, "expected `,` or `]` in JSON array")?;
                self.skip_newlines();
                if self.check(TokenKind::RBracket) {
                    break; // trailing comma
                }
                out.push(',');
            }
            first = false;
            let val = self.parse_json_value(open_span)?;
            self.skip_newlines();
            out.push_str(&val);
        }
        self.expect(TokenKind::RBracket, "expected `]` to close JSON array")?;
        out.push(']');
        Ok(out)
    }

    fn parse_json_value(&mut self, open_span: Span) -> Result<String> {
        let tok = self.peek().clone();
        match tok.kind {
                TokenKind::LBrace => {
                    self.advance();
                    self.parse_json_object_body(tok.span)
                }
                TokenKind::LBracket => {
                    self.advance();
                    self.parse_json_array_body(tok.span)
                }
                TokenKind::StringLit => {
                    self.advance();
                    Ok(json_encode_string(&tok.lexeme))
                }
                TokenKind::IntLit => {
                    self.advance();
                    Ok(tok.lexeme)
                }
                TokenKind::FloatLit => {
                    self.advance();
                    Ok(tok.lexeme)
                }
                TokenKind::Minus => {
                    self.advance();
                    let num = self.peek().clone();
                    match num.kind {
                        TokenKind::IntLit | TokenKind::FloatLit => {
                            self.advance();
                            Ok(format!("-{}", num.lexeme))
                        }
                        _ => Err(OnewayError::ParseError {
                            message: "expected a number after `-` in JSON literal".to_string(),
                            span: num.span,
                        }),
                    }
                }
                TokenKind::Ident => match tok.lexeme.as_str() {
                    "true" | "false" | "null" => {
                        self.advance();
                        Ok(tok.lexeme)
                    }
                    _ => Err(OnewayError::ParseError {
                        message: format!(
                            "unexpected `{}` in JSON literal — expected a string, number, object, array, `true`, `false`, or `null`",
                            tok.lexeme
                        ),
                        span: tok.span,
                    }),
                },
                _ => Err(OnewayError::ParseError {
                    message: format!("expected a JSON value, got {}", tok.kind),
                    span: open_span,
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
        self.expect(
            TokenKind::RParen,
            "expected `)` to close lambda parameter list",
        )?;
        self.expect(
            TokenKind::Arrow,
            "expected `->` after lambda parameter list",
        )?;
        let return_ty = self.parse_type_expr()?;
        let body = self.parse_block()?;
        Ok(Expr::Lambda {
            params,
            return_ty,
            body,
            span: span_join(lparen.span, self.previous_span()),
        })
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm> {
        let start = self.current_span();
        // Each arm is: (VariantType) -> ReturnType { body }
        self.expect(TokenKind::LParen, "expected `(` to begin dispatch arm")?;
        // Parse the single variant type — may have generics: Err<String>, Ok<Int>, Branch
        let param_ty = self.parse_type_atom()?;
        self.expect(TokenKind::RParen, "expected `)` to close dispatch arm")?;
        self.expect(TokenKind::Arrow, "expected `->` in dispatch arm")?;
        let return_ty = self.parse_type_expr()?;
        let body = self.parse_block()?;
        let end = self.previous_span();
        Ok(MatchArm {
            param_ty,
            return_ty,
            body,
            span: span_join(start, end),
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

    fn is_pascal_case_str(s: &str) -> bool {
        s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Eof)
    }
}

fn span_join(a: Span, b: Span) -> Span {
    Span::new(a.start.min(b.start), a.end.max(b.end), a.line, a.column)
}

/// Re-encode a Oneway string value (already unescaped by the scanner) as a
/// JSON string literal, including the surrounding double-quote characters.
fn json_encode_string(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
