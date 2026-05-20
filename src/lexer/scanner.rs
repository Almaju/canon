use crate::error::{OnewayError, Result, Span};
use crate::lexer::token::{Token, TokenKind};

pub struct Scanner<'a> {
    source: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line: u32,
    column: u32,
}

impl<'a> Scanner<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            pos: 0,
            line: 1,
            column: 1,
        }
    }

    pub fn scan_tokens(&mut self) -> Result<Vec<Token>> {
        let mut tokens = Vec::new();
        loop {
            self.skip_inline_whitespace();
            if self.pos >= self.bytes.len() {
                break;
            }
            let tok = self.scan_one()?;
            tokens.push(tok);
        }
        tokens.push(Token {
            kind: TokenKind::Eof,
            lexeme: String::new(),
            span: self.point_span(),
        });
        Ok(tokens)
    }

    fn skip_inline_whitespace(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b' ' | b'\t' | b'\r' => {
                    self.pos += 1;
                    self.column += 1;
                }
                _ => break,
            }
        }
    }

    fn scan_one(&mut self) -> Result<Token> {
        let start_pos = self.pos;
        let start_line = self.line;
        let start_col = self.column;
        let c = self.bytes[self.pos];

        let (kind, lexeme) = match c {
            b'\n' => {
                self.pos += 1;
                self.line += 1;
                self.column = 1;
                (TokenKind::Newline, "\n".to_string())
            }
            b'(' => self.single(TokenKind::LParen, "("),
            b')' => self.single(TokenKind::RParen, ")"),
            b'{' => self.single(TokenKind::LBrace, "{"),
            b'}' => self.single(TokenKind::RBrace, "}"),
            b'[' => self.single(TokenKind::LBracket, "["),
            b']' => self.single(TokenKind::RBracket, "]"),
            b'<' => self.single(TokenKind::Lt, "<"),
            b'>' => self.single(TokenKind::Gt, ">"),
            b'+' => self.single(TokenKind::Plus, "+"),
            b',' => self.single(TokenKind::Comma, ","),
            b':' => {
                if self.peek_byte(1) == Some(b':') {
                    self.pos += 2;
                    self.column += 2;
                    (TokenKind::ColonColon, "::".to_string())
                } else {
                    self.single(TokenKind::Colon, ":")
                }
            }
            b'?' => self.single(TokenKind::Question, "?"),
            b'*' => self.single(TokenKind::Star, "*"),
            b'^' => self.single(TokenKind::Caret, "^"),
            b'=' => {
                if self.peek_byte(1) == Some(b'>') {
                    self.pos += 2;
                    self.column += 2;
                    (TokenKind::FatArrow, "=>".to_string())
                } else {
                    self.single(TokenKind::Eq, "=")
                }
            }
            b'-' => {
                if self.peek_byte(1) == Some(b'>') {
                    self.pos += 2;
                    self.column += 2;
                    (TokenKind::Arrow, "->".to_string())
                } else {
                    return Err(self.err_at(start_line, start_col, "unexpected `-`"));
                }
            }
            b'.' => {
                if self.peek_byte(1) == Some(b'.') && self.peek_byte(2) == Some(b'.') {
                    self.pos += 3;
                    self.column += 3;
                    (TokenKind::Ellipsis, "...".to_string())
                } else {
                    self.single(TokenKind::Dot, ".")
                }
            }
            b'"' => self.scan_string(start_line, start_col)?,
            c if c.is_ascii_digit() => self.scan_number(start_line, start_col)?,
            c if is_ident_start(c) => self.scan_ident(start_pos),
            _ => {
                return Err(self.err_at(
                    start_line,
                    start_col,
                    &format!("unexpected character `{}`", c as char),
                ));
            }
        };

        let span = Span::new(start_pos, self.pos, start_line, start_col);
        Ok(Token { kind, lexeme, span })
    }

    fn single(&mut self, kind: TokenKind, s: &str) -> (TokenKind, String) {
        self.pos += 1;
        self.column += 1;
        (kind, s.to_string())
    }

    fn scan_ident(&mut self, start_pos: usize) -> (TokenKind, String) {
        while self.pos < self.bytes.len() && is_ident_continue(self.bytes[self.pos]) {
            self.pos += 1;
            self.column += 1;
        }
        let lex = self.source[start_pos..self.pos].to_string();
        let kind = match lex.as_str() {
            "mut" => TokenKind::KwMut,
            "use" => TokenKind::KwUse,
            "Self" => TokenKind::KwSelf,
            "impl" => TokenKind::KwImpl,
            "extern" => TokenKind::KwExtern,
            _ => TokenKind::Ident,
        };
        (kind, lex)
    }

    fn scan_number(&mut self, start_line: u32, start_col: u32) -> Result<(TokenKind, String)> {
        let start_pos = self.pos;

        if self.bytes[self.pos] == b'0' && self.peek_byte(1) == Some(b'x') {
            self.pos += 2;
            self.column += 2;
            let hex_start = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_hexdigit() {
                self.pos += 1;
                self.column += 1;
            }
            if self.pos == hex_start {
                return Err(self.err_at(
                    start_line,
                    start_col,
                    "hex literal requires at least one digit after `0x`",
                ));
            }
            return Ok((
                TokenKind::HexLit,
                self.source[start_pos..self.pos].to_string(),
            ));
        }

        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
            self.column += 1;
        }

        if self.peek_byte(0) == Some(b'.')
            && self.peek_byte(1).map_or(false, |b| b.is_ascii_digit())
        {
            self.pos += 1;
            self.column += 1;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
                self.column += 1;
            }
            return Ok((
                TokenKind::FloatLit,
                self.source[start_pos..self.pos].to_string(),
            ));
        }

        Ok((
            TokenKind::IntLit,
            self.source[start_pos..self.pos].to_string(),
        ))
    }

    fn scan_string(&mut self, start_line: u32, start_col: u32) -> Result<(TokenKind, String)> {
        self.pos += 1;
        self.column += 1;
        let content_start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos] != b'"' {
            if self.bytes[self.pos] == b'\n' {
                return Err(self.err_at(start_line, start_col, "unterminated string literal"));
            }
            self.pos += 1;
            self.column += 1;
        }
        if self.pos >= self.bytes.len() {
            return Err(self.err_at(start_line, start_col, "unterminated string literal"));
        }
        let content = self.source[content_start..self.pos].to_string();
        self.pos += 1;
        self.column += 1;
        Ok((TokenKind::StringLit, content))
    }

    fn peek_byte(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn point_span(&self) -> Span {
        Span::new(self.pos, self.pos, self.line, self.column)
    }

    fn err_at(&self, line: u32, column: u32, msg: &str) -> OnewayError {
        OnewayError::LexError {
            message: msg.to_string(),
            span: Span::new(self.pos, self.pos, line, column),
        }
    }
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}
