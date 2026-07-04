use crate::ast::extract_receiver_from_params;
use crate::ast::*;
use crate::error::{CanonError, Result, Span};
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

        // `use` was removed from the language: imports are automatic.
        // Recognize the old keyword purely to steer migration — the
        // message says what replaced it.
        if self.check(TokenKind::KwUse) {
            return Err(CanonError::ParseError {
                message: "`use` has been removed: a reference to `Foo` resolves to `foo.can` \
                          automatically (project files, then `deps/`, then the standard \
                          library) — delete this line"
                    .to_string(),
                span: start_span,
            });
        }

        let first = self.expect(TokenKind::Ident, "expected a top-level definition")?;
        let first_ident = Ident {
            name: first.lexeme.clone(),
            span: first.span,
        };

        let pre_eq_generics = if self.check(TokenKind::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::Eq, "expected `=` in top-level definition")?;

        if self.check(TokenKind::LParen) || self.check(TokenKind::Lt) {
            if !pre_eq_generics.is_empty() {
                return Err(CanonError::ParseError {
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
            } else if matches!(
                crate::ast::entry_world_of(&return_ty),
                Some(crate::ast::EntryWorld::Http)
            ) {
                // World-shape return (`Response` / `Result<Response, _>`):
                // this is an HTTP entry, not a method. Suppress receiver
                // extraction so `home = (Request) -> Response { … }` stays
                // a free function with `Request` as its parameter (not
                // a method on `Request`). See `WASI-HTTP-HANDLER.md`
                // §Entry-point selection.
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
                extern_wasm: None,
                span: span_join(start_span, end_span),
            }));
        }

        if receiver.is_some() {
            return Err(CanonError::ParseError {
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
        // Allow the `*` to appear on the next line so that multi-line product
        // type definitions (emitted by the formatter) round-trip correctly.
        if self.peek_past_newlines() != TokenKind::Star {
            return Ok(first);
        }
        let mut fields = vec![first];
        while self.peek_past_newlines() == TokenKind::Star {
            self.skip_newlines();
            self.advance(); // consume `*`
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
            .map_err(|_| CanonError::ParseError {
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
            // `(T)` with no `->` following is type *grouping*, not a
            // function type — the formatter parenthesises function-typed
            // product members (`Stream<T> * ((T) -> Bool)`), so the
            // parens must round-trip. Only the single-type, non-generic
            // form qualifies as a group.
            if !self.check(TokenKind::Arrow) && generic_params.is_empty() && params.len() == 1 {
                return Ok(params.pop().unwrap());
            }
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
                    // Optional generic type args after the field/method
                    // name (`value.Option<Content>`, etc.). See
                    // `consume_phantom_type_args` for the rationale.
                    let _consumed_generics = self.consume_phantom_type_args(&name_tok.lexeme)?;
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
                // Optional generic type arguments after a PascalCase
                // identifier: `Option<Content>`, `List<Choice>`, etc.
                // Accepted and discarded (see `consume_phantom_type_args`).
                // Skipped when followed by `(` to avoid swallowing the
                // turbofish-like form `Foo<T>(arg)` mid-call — type args
                // before a constructor call use `::<>` (turbofish) syntax.
                let _consumed_generics = self.consume_phantom_type_args(&tok.lexeme)?;
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
                let value: i64 = tok.lexeme.parse().map_err(|_| CanonError::ParseError {
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
                let value: f64 = tok.lexeme.parse().map_err(|_| CanonError::ParseError {
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
                    u64::from_str_radix(stripped, 16).map_err(|_| CanonError::ParseError {
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
                let mut builder = JsonPartBuilder::new();
                builder.push_char('{');
                self.parse_json_object_body(open, &mut builder)?;
                builder.push_char('}');
                let end = self.previous_span();
                Ok(Expr::JsonLit {
                    parts: builder.finish(),
                    span: span_join(open, end),
                })
            }
            TokenKind::LBracket => {
                let open = tok.span;
                self.advance();
                let mut builder = JsonPartBuilder::new();
                builder.push_char('[');
                self.parse_json_array_body(open, &mut builder)?;
                builder.push_char(']');
                let end = self.previous_span();
                Ok(Expr::JsonLit {
                    parts: builder.finish(),
                    span: span_join(open, end),
                })
            }
            TokenKind::HtmlText | TokenKind::HtmlEnd => self.parse_html_literal(),
            _ => Err(CanonError::ParseError {
                message: format!("expected an expression (got {})", tok.kind),
                span: tok.span,
            }),
        }
    }

    /// Parse an HTML literal from the raw-fragment tokens the scanner
    /// emits: zero or more `HtmlText` fragments (each followed by an
    /// interpolated expression — the scanner already consumed the
    /// hole's braces) and a final `HtmlEnd` fragment.
    ///
    /// Pure-literal interpolations fold to Static text so an
    /// all-constant literal stays zero-runtime-cost: strings are
    /// HTML-escaped at parse time, ints/floats render as digits, and a
    /// nested HTML literal splices its parts verbatim (it is already
    /// HTML). Anything with runtime content becomes an `Interp` part,
    /// which the codegen `.ToHtml()`-converts and concats into the
    /// surrounding markup.
    fn parse_html_literal(&mut self) -> Result<Expr> {
        let start = self.peek().span;
        let mut parts: Vec<HtmlLitPart> = Vec::new();
        loop {
            let tok = self.peek().clone();
            let is_end = match tok.kind {
                TokenKind::HtmlText => false,
                TokenKind::HtmlEnd => true,
                _ => {
                    return Err(CanonError::ParseError {
                        message: format!(
                            "expected `}}` to close HTML interpolation (got {})",
                            tok.kind
                        ),
                        span: tok.span,
                    });
                }
            };
            self.advance();
            push_html_static(&mut parts, &tok.lexeme);
            if is_end {
                break;
            }
            self.skip_newlines();
            let expr = self.parse_expr()?;
            self.skip_newlines();
            match expr {
                Expr::StringLit { value, .. } => {
                    push_html_static(&mut parts, &html_encode_text(&value));
                }
                Expr::IntLit { value, .. } => {
                    push_html_static(&mut parts, &value.to_string());
                }
                Expr::FloatLit { value, .. } => {
                    push_html_static(&mut parts, &value.to_string());
                }
                Expr::HtmlLit {
                    parts: inner_parts, ..
                } => {
                    // Splice the nested literal's parts directly so
                    // static chunks merge into the outer accumulator.
                    for p in inner_parts {
                        match p {
                            HtmlLitPart::Static(s) => push_html_static(&mut parts, &s),
                            HtmlLitPart::Interp(e) => parts.push(HtmlLitPart::Interp(e)),
                        }
                    }
                }
                other => {
                    parts.push(HtmlLitPart::Interp(Box::new(other)));
                }
            }
        }
        let end = self.previous_span();
        Ok(Expr::HtmlLit {
            parts,
            span: span_join(start, end),
        })
    }

    fn parse_json_object_body(
        &mut self,
        open_span: Span,
        builder: &mut JsonPartBuilder,
    ) -> Result<()> {
        self.skip_newlines();
        let mut first = true;
        while !self.check(TokenKind::RBrace) && !self.is_at_end() {
            if !first {
                self.expect(TokenKind::Comma, "expected `,` or `}` in JSON object")?;
                self.skip_newlines();
                if self.check(TokenKind::RBrace) {
                    break; // trailing comma
                }
                builder.push_char(',');
            }
            first = false;
            let key_tok =
                self.expect(TokenKind::StringLit, "expected string key in JSON object")?;
            builder.push_text(&json_encode_string(&key_tok.lexeme));
            self.skip_newlines();
            self.expect(TokenKind::Colon, "expected `:` after JSON key")?;
            self.skip_newlines();
            builder.push_char(':');
            self.parse_json_value(open_span, builder)?;
            self.skip_newlines();
        }
        self.expect(TokenKind::RBrace, "expected `}` to close JSON object")?;
        Ok(())
    }

    fn parse_json_array_body(
        &mut self,
        open_span: Span,
        builder: &mut JsonPartBuilder,
    ) -> Result<()> {
        self.skip_newlines();
        let mut first = true;
        while !self.check(TokenKind::RBracket) && !self.is_at_end() {
            if !first {
                self.expect(TokenKind::Comma, "expected `,` or `]` in JSON array")?;
                self.skip_newlines();
                if self.check(TokenKind::RBracket) {
                    break; // trailing comma
                }
                builder.push_char(',');
            }
            first = false;
            self.parse_json_value(open_span, builder)?;
            self.skip_newlines();
        }
        self.expect(TokenKind::RBracket, "expected `]` to close JSON array")?;
        Ok(())
    }

    /// Parse a single JSON value, appending it to `builder`. The strategy:
    ///
    /// 1. JSON-only forms (`{`, `[`, the keywords `true` / `false` /
    ///    `null`, and `-IntLit` / `-FloatLit` since Canon has no unary
    ///    minus at expression level) are consumed directly and emitted
    ///    as Static text.
    /// 2. Everything else is parsed as a full Canon expression. If the
    ///    resulting expression is itself a pure literal
    ///    (`StringLit` / `IntLit` / `FloatLit` / `JsonLit`), it's still
    ///    emitted as Static — so `{"k": 42}` stays fully constant. Any
    ///    expression with runtime content (`Ident`, `MethodCall`, etc.)
    ///    becomes an `Interp` part, which the codegen later
    ///    `.ToJson()`-converts and concats into the surrounding
    ///    scaffolding.
    ///
    /// This two-tier approach lets `{"x": foo.bar()}` parse as a JSON
    /// literal *with* an interpolated expression, without committing
    /// eagerly to the literal-shaped path on bare `IntLit`s.
    fn parse_json_value(&mut self, _open_span: Span, builder: &mut JsonPartBuilder) -> Result<()> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::LBrace => {
                self.advance();
                builder.push_char('{');
                self.parse_json_object_body(tok.span, builder)?;
                builder.push_char('}');
                return Ok(());
            }
            TokenKind::LBracket => {
                self.advance();
                builder.push_char('[');
                self.parse_json_array_body(tok.span, builder)?;
                builder.push_char(']');
                return Ok(());
            }
            TokenKind::Minus => {
                // `-IntLit` / `-FloatLit` are folded as a static JSON
                // negative number. Canon has no general unary-minus, so
                // a bare `-` followed by anything else is an error here.
                self.advance();
                let num = self.peek().clone();
                match num.kind {
                    TokenKind::IntLit | TokenKind::FloatLit => {
                        self.advance();
                        builder.push_char('-');
                        builder.push_text(&num.lexeme);
                        return Ok(());
                    }
                    _ => {
                        return Err(CanonError::ParseError {
                            message: "expected a number after `-` in JSON literal".to_string(),
                            span: num.span,
                        });
                    }
                }
            }
            TokenKind::Ident if matches!(tok.lexeme.as_str(), "true" | "false" | "null") => {
                // JSON keywords: not valid Canon identifiers, so we have
                // to handle them before `parse_expr` would choke. (Canon
                // booleans are capitalised: `True()` / `False()`.)
                self.advance();
                builder.push_text(&tok.lexeme);
                return Ok(());
            }
            _ => {}
        }

        // General case: parse a full Canon expression. This handles
        // bare literals (`42`, `"hi"`), method chains (`x.foo()`,
        // `1.add(2)`), constructors (`Email("a")`), field access
        // (`User.Email`), `?` on Results, etc.
        let expr = self.parse_expr()?;
        // Fold pure-literal exprs into Static text so all-constant JSON
        // literals stay zero-runtime-cost. Anything with runtime content
        // becomes an Interp.
        match expr {
            Expr::StringLit { value, .. } => {
                builder.push_text(&json_encode_string(&value));
            }
            Expr::IntLit { value, .. } => {
                builder.push_text(&value.to_string());
            }
            Expr::FloatLit { value, .. } => {
                builder.push_text(&value.to_string());
            }
            Expr::JsonLit { parts, .. } => {
                // Inline the nested literal's parts directly so static
                // chunks merge into the outer accumulator.
                for p in parts {
                    match p {
                        JsonLitPart::Static(s) => builder.push_text(&s),
                        JsonLitPart::Interp(e) => builder.push_interp(*e),
                    }
                }
            }
            other => {
                builder.push_interp(other);
            }
        }
        Ok(())
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
        // or a literal-pattern arm: ("literal") / (123) -> ReturnType { body }
        self.expect(TokenKind::LParen, "expected `(` to begin dispatch arm")?;
        // A literal token in pattern position makes this a literal arm
        // (equality dispatch on a `String` / `Int` scrutinee). The
        // `param_ty` records the matching primitive type name so every
        // type-shaped consumer of the arm stays well-formed.
        let (param_ty, literal) = match self.peek().kind {
            TokenKind::StringLit => {
                let tok = self.advance();
                (
                    TypeExpr::Named {
                        name: "String".to_string(),
                        generics: Vec::new(),
                        span: tok.span,
                    },
                    Some(ArmLiteral::Str(tok.lexeme.clone())),
                )
            }
            TokenKind::IntLit => {
                let tok = self.advance().clone();
                let value: i64 = tok.lexeme.parse().map_err(|_| CanonError::ParseError {
                    message: format!("invalid integer literal `{}`", tok.lexeme),
                    span: tok.span,
                })?;
                (
                    TypeExpr::Named {
                        name: "Int".to_string(),
                        generics: Vec::new(),
                        span: tok.span,
                    },
                    Some(ArmLiteral::Int(value)),
                )
            }
            // Parse the single variant type — may have generics:
            // Err<String>, Ok<Int>, Branch
            _ => (self.parse_type_atom()?, None),
        };
        self.expect(TokenKind::RParen, "expected `)` to close dispatch arm")?;
        self.expect(TokenKind::Arrow, "expected `->` in dispatch arm")?;
        let return_ty = self.parse_type_expr()?;
        let body = self.parse_block()?;
        let end = self.previous_span();
        Ok(MatchArm {
            param_ty,
            literal,
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
            Err(CanonError::ParseError {
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
        s.chars().next().is_some_and(char::is_uppercase)
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Eof)
    }

    /// If the next token is `<` and the current identifier is PascalCase,
    /// consume a `<T1, T2, ...>` type-argument list and discard it.
    ///
    /// Returns `true` iff a type-argument list was consumed.
    ///
    /// This exists so that parameterized type names (`Option<Content>`,
    /// `List<Choice>`, `Result<T, E>`) can appear in expression position
    /// where they would otherwise be a parse error. Examples:
    ///
    /// ```text
    /// // Dispatch on a value whose static type carries generic args:
    /// Option<Content>.(
    ///     * (None)          -> Json { ... }
    ///     * (Some<Content>) -> Json { ... }
    /// )
    ///
    /// // Field access where the field's underlying type is generic:
    /// wrapper.Option<Content>
    /// ```
    ///
    /// The generic args are accepted but discarded — runtime values don't
    /// carry generic parameters, only their unparameterized type names
    /// reach codegen. The identifier (`Option`, `List`, `Result`, …) is
    /// what the checker and codegen look up. `<` is not used as a binary
    /// operator anywhere in expression position, so consuming it here is
    /// unambiguous.
    fn consume_phantom_type_args(&mut self, ident_lexeme: &str) -> Result<bool> {
        if !Self::is_pascal_case_str(ident_lexeme) {
            return Ok(false);
        }
        if !self.check(TokenKind::Lt) {
            return Ok(false);
        }
        self.advance();
        if !self.check(TokenKind::Gt) {
            loop {
                // Parse the type arg purely for syntactic acceptance.
                // The result is dropped — it doesn't affect runtime
                // behaviour.
                let _ = self.parse_type_expr()?;
                if self.check(TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect(TokenKind::Gt, "expected `>` to close generic argument list")?;
        Ok(true)
    }
}

fn span_join(a: Span, b: Span) -> Span {
    Span::new(a.start.min(b.start), a.end.max(b.end), a.line, a.column)
}

/// Accumulator used while parsing a JSON literal: collects pre-encoded
/// JSON text fragments into a flat list of `JsonLitPart`s, merging
/// consecutive static text so the final `parts` always alternates
/// `Static` ↔ `Interp` (starting and ending with `Static` when not
/// empty). See `Expr::JsonLit`.
struct JsonPartBuilder {
    parts: Vec<JsonLitPart>,
    current: String,
}

impl JsonPartBuilder {
    fn new() -> Self {
        Self {
            parts: Vec::new(),
            current: String::new(),
        }
    }

    fn push_text(&mut self, s: &str) {
        self.current.push_str(s);
    }

    fn push_char(&mut self, c: char) {
        self.current.push(c);
    }

    fn push_interp(&mut self, expr: Expr) {
        if !self.current.is_empty() {
            self.parts
                .push(JsonLitPart::Static(std::mem::take(&mut self.current)));
        }
        self.parts.push(JsonLitPart::Interp(Box::new(expr)));
    }

    fn finish(mut self) -> Vec<JsonLitPart> {
        if !self.current.is_empty() {
            self.parts.push(JsonLitPart::Static(self.current));
        }
        self.parts
    }
}

/// Re-encode a Canon string value (already unescaped by the scanner) as a
/// JSON string literal, including the surrounding double-quote characters.
/// Append a static HTML fragment, merging into a preceding `Static`
/// part so all-constant literals collapse to a single part (which is
/// what triggers the codegen's zero-cost fast path). Empty fragments
/// (e.g. two adjacent interpolation holes) are dropped.
fn push_html_static(parts: &mut Vec<HtmlLitPart>, s: &str) {
    if s.is_empty() {
        return;
    }
    if let Some(HtmlLitPart::Static(last)) = parts.last_mut() {
        last.push_str(s);
    } else {
        parts.push(HtmlLitPart::Static(s.to_string()));
    }
}

/// HTML-escape a string for element text content — the parse-time
/// equivalent of the stdlib's `text()` (`packages/canon/std/src/web/
/// html.can`), applied when a string-literal interpolation is folded
/// statically.
fn html_encode_text(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("&quot;"),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

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
