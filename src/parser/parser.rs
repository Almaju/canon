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
        let func = self.parse_function_def()?;
        Ok(Item::Function(func))
    }

    fn parse_function_def(&mut self) -> Result<FunctionDef> {
        let start_span = self.current_span();
        let first = self.expect(TokenKind::Ident, "expected function or receiver name")?;
        let first_ident = Ident {
            name: first.lexeme.clone(),
            span: first.span,
        };

        let (receiver, name) = if self.check(TokenKind::Dot) {
            self.advance();
            let name_tok = self.expect(TokenKind::Ident, "expected function name after `.`")?;
            (
                Some(first_ident),
                Ident {
                    name: name_tok.lexeme.clone(),
                    span: name_tok.span,
                },
            )
        } else {
            (None, first_ident)
        };

        self.expect(TokenKind::Eq, "expected `=` in function definition")?;
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
        let body = self.parse_block()?;

        let end_span = self.previous_span();
        Ok(FunctionDef {
            receiver,
            name,
            params,
            return_ty,
            body,
            span: span_join(start_span, end_span),
        })
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
        let tok = self.expect(TokenKind::Ident, "expected a type name")?;
        Ok(TypeExpr {
            name: tok.lexeme.clone(),
            span: tok.span,
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
        let tok = self.expect(TokenKind::Ident, "expected an expression")?;
        Ok(Expr::Ident(Ident {
            name: tok.lexeme.clone(),
            span: tok.span,
        }))
    }

    fn skip_newlines(&mut self) {
        while self.check(TokenKind::Newline) {
            self.advance();
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
