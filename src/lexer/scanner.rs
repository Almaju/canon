use crate::error::{CanonError, Result, Span};
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
            b'/' => self.single(TokenKind::Slash, "/"),
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
                    self.single(TokenKind::Minus, "-")
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

        if self.peek_byte(0) == Some(b'.') && self.peek_byte(1).is_some_and(|b| b.is_ascii_digit())
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
        self.pos += 1; // consume opening "
        self.column += 1;
        let mut content = String::new();
        loop {
            if self.pos >= self.bytes.len() {
                return Err(self.err_at(start_line, start_col, "unterminated string literal"));
            }
            match self.bytes[self.pos] {
                b'"' => {
                    self.pos += 1;
                    self.column += 1;
                    break;
                }
                b'\n' => {
                    return Err(self.err_at(start_line, start_col, "unterminated string literal"));
                }
                b'\\' => {
                    let esc_line = self.line;
                    let esc_col = self.column;
                    self.pos += 1;
                    self.column += 1;
                    if self.pos >= self.bytes.len() {
                        return Err(self.err_at(
                            start_line,
                            start_col,
                            "unterminated string literal",
                        ));
                    }
                    match self.bytes[self.pos] {
                        b'\\' => {
                            content.push('\\');
                            self.pos += 1;
                            self.column += 1;
                        }
                        b'"' => {
                            content.push('"');
                            self.pos += 1;
                            self.column += 1;
                        }
                        b'n' => {
                            content.push('\n');
                            self.pos += 1;
                            self.column += 1;
                        }
                        b'r' => {
                            content.push('\r');
                            self.pos += 1;
                            self.column += 1;
                        }
                        b't' => {
                            content.push('\t');
                            self.pos += 1;
                            self.column += 1;
                        }
                        b'0' => {
                            content.push('\0');
                            self.pos += 1;
                            self.column += 1;
                        }
                        b'x' => {
                            self.pos += 1;
                            self.column += 1;
                            let hi = self.consume_hex_digit(esc_line, esc_col)?;
                            let lo = self.consume_hex_digit(esc_line, esc_col)?;
                            content.push(char::from((hi << 4) | lo));
                        }
                        b'u' => {
                            self.pos += 1;
                            self.column += 1;
                            let code = self.consume_hex_digits(4, esc_line, esc_col)?;
                            let ch = char::from_u32(code).ok_or_else(|| {
                                self.err_at(
                                    esc_line,
                                    esc_col,
                                    &format!("invalid Unicode scalar value U+{:04X}", code),
                                )
                            })?;
                            content.push(ch);
                        }
                        b'U' => {
                            self.pos += 1;
                            self.column += 1;
                            let code = self.consume_hex_digits(8, esc_line, esc_col)?;
                            let ch = char::from_u32(code).ok_or_else(|| {
                                self.err_at(
                                    esc_line,
                                    esc_col,
                                    &format!("invalid Unicode scalar value U+{:08X}", code),
                                )
                            })?;
                            content.push(ch);
                        }
                        other => {
                            return Err(self.err_at(
                                esc_line,
                                esc_col,
                                &format!("unknown escape sequence '\\{}'", other as char),
                            ));
                        }
                    }
                }
                b => {
                    content.push(b as char);
                    self.pos += 1;
                    self.column += 1;
                }
            }
        }
        Ok((TokenKind::StringLit, content))
    }

    fn consume_hex_digit(&mut self, err_line: u32, err_col: u32) -> Result<u8> {
        if self.pos >= self.bytes.len() {
            return Err(self.err_at(err_line, err_col, "unexpected end of escape sequence"));
        }
        let b = self.bytes[self.pos];
        let val = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => {
                return Err(self.err_at(err_line, err_col, "expected hex digit in escape sequence"))
            }
        };
        self.pos += 1;
        self.column += 1;
        Ok(val)
    }

    fn consume_hex_digits(&mut self, count: usize, err_line: u32, err_col: u32) -> Result<u32> {
        let mut acc: u32 = 0;
        for _ in 0..count {
            acc = (acc << 4) | self.consume_hex_digit(err_line, err_col)? as u32;
        }
        Ok(acc)
    }

    fn peek_byte(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn point_span(&self) -> Span {
        Span::new(self.pos, self.pos, self.line, self.column)
    }

    fn err_at(&self, line: u32, column: u32, msg: &str) -> CanonError {
        CanonError::LexError {
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
