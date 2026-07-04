use crate::error::{CanonError, Result, Span};
use crate::lexer::token::{Token, TokenKind};

/// Scanner modes beyond ordinary token scanning. HTML literals are raw
/// text — their content never tokenizes as Canon — so the scanner keeps
/// a mode stack: `Html` while inside a literal's markup, `Interp` while
/// inside a `{…}` interpolation hole (ordinary tokens, plus brace-depth
/// tracking to find the hole's closing `}`, which is swallowed). The
/// stack nests: an interpolation may itself contain another HTML
/// literal.
enum LexMode {
    Html(HtmlState),
    Interp { brace_depth: u32 },
}

/// Where an HTML-mode scan left off. Persisted across interpolation
/// holes so a `{…}` inside an attribute value (`<div class="{c}">`)
/// resumes mid-tag, mid-quote.
struct HtmlState {
    /// Number of currently open (unclosed) elements. The literal is
    /// complete when this returns to zero after a tag closes.
    depth: u32,
    /// `Some` while inside an opening tag (between `<name` and its
    /// `>`). Carries the tag name (for void-element handling) and the
    /// active attribute-value quote, if any.
    tag: Option<TagState>,
    /// Span bookkeeping for "unterminated HTML literal" errors.
    start_line: u32,
    start_col: u32,
}

struct TagState {
    name: String,
    quote: Option<u8>,
}

/// What ended an HTML chunk scan.
enum HtmlChunk {
    /// Hit a `{` — an interpolation hole opens (the `{` is consumed).
    Interp,
    /// The literal's root element closed — the literal is complete.
    Done,
}

/// Elements that never take a closing tag (HTML's void elements), so
/// `<br>` doesn't open a nesting level the depth counter would wait on.
fn is_void_element(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

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
        let mut modes: Vec<LexMode> = Vec::new();
        loop {
            if let Some(LexMode::Html(state)) = modes.last_mut() {
                // Inside an HTML literal: scan a raw chunk (verbatim,
                // no whitespace skipping) up to the next interpolation
                // hole or the literal's end.
                let (tok, outcome) = self.scan_html_chunk(state)?;
                tokens.push(tok);
                match outcome {
                    HtmlChunk::Interp => modes.push(LexMode::Interp { brace_depth: 0 }),
                    HtmlChunk::Done => {
                        modes.pop();
                    }
                }
                continue;
            }
            self.skip_inline_whitespace();
            if self.pos >= self.bytes.len() {
                if !modes.is_empty() {
                    return Err(self.err_at(
                        self.line,
                        self.column,
                        "unterminated HTML interpolation: expected `}`",
                    ));
                }
                break;
            }
            // An HTML literal starts at `<` immediately followed by a
            // lowercase tag name — a position where `<` is never valid
            // Canon (generic arguments are PascalCase types).
            if self.bytes[self.pos] == b'<'
                && self.peek_byte(1).is_some_and(|b| b.is_ascii_lowercase())
            {
                modes.push(LexMode::Html(HtmlState {
                    depth: 0,
                    tag: None,
                    start_line: self.line,
                    start_col: self.column,
                }));
                continue;
            }
            let tok = self.scan_one()?;
            match tok.kind {
                TokenKind::LBrace => {
                    if let Some(LexMode::Interp { brace_depth }) = modes.last_mut() {
                        *brace_depth += 1;
                    }
                }
                TokenKind::RBrace => {
                    if let Some(LexMode::Interp { brace_depth }) = modes.last_mut() {
                        if *brace_depth == 0 {
                            // The hole's closing `}` — swallow it and
                            // resume the enclosing HTML literal.
                            modes.pop();
                            continue;
                        }
                        *brace_depth -= 1;
                    }
                }
                _ => {}
            }
            tokens.push(tok);
        }
        tokens.push(Token {
            kind: TokenKind::Eof,
            lexeme: String::new(),
            span: self.point_span(),
        });
        Ok(tokens)
    }

    /// Scan one raw fragment of an HTML literal, starting either at the
    /// literal's opening `<` or just after an interpolation hole. Ends
    /// at a `{` (interpolation opens; `{{` / `}}` escape literal
    /// braces) or when the root element closes. Depth counts opening
    /// tags (+1, except self-closing and void elements) and closing
    /// tags (−1); comments and doctypes pass through verbatim; a stray
    /// `<` not followed by a tag shape is literal text.
    fn scan_html_chunk(&mut self, state: &mut HtmlState) -> Result<(Token, HtmlChunk)> {
        let start_pos = self.pos;
        let start_line = self.line;
        let start_col = self.column;
        let lit_line = state.start_line;
        let lit_col = state.start_col;
        let mut text: Vec<u8> = Vec::new();
        loop {
            if self.pos >= self.bytes.len() {
                return Err(self.err_at(lit_line, lit_col, "unterminated HTML literal"));
            }
            let b = self.bytes[self.pos];
            // `{` opens an interpolation hole anywhere in the literal
            // (element text or attribute value); `{{` and `}}` are
            // escapes for literal braces.
            if b == b'{' {
                if self.peek_byte(1) == Some(b'{') {
                    text.push(b'{');
                    self.bump_raw();
                    self.bump_raw();
                    continue;
                }
                self.bump_raw(); // consume the `{`
                let span = Span::new(start_pos, self.pos, start_line, start_col);
                return Ok((
                    Token {
                        kind: TokenKind::HtmlText,
                        lexeme: html_chunk_string(text),
                        span,
                    },
                    HtmlChunk::Interp,
                ));
            }
            if b == b'}' && self.peek_byte(1) == Some(b'}') {
                text.push(b'}');
                self.bump_raw();
                self.bump_raw();
                continue;
            }
            if let Some(tag) = &mut state.tag {
                // Inside an opening tag: track attribute-value quotes
                // and watch for the closing `>`.
                if let Some(q) = tag.quote {
                    if b == q {
                        tag.quote = None;
                    }
                    text.push(b);
                    self.bump_raw();
                } else if b == b'"' || b == b'\'' {
                    tag.quote = Some(b);
                    text.push(b);
                    self.bump_raw();
                } else if b == b'>' {
                    let self_closing = text
                        .iter()
                        .rev()
                        .find(|c| !c.is_ascii_whitespace())
                        .is_some_and(|c| *c == b'/');
                    let void = is_void_element(&tag.name);
                    text.push(b'>');
                    self.bump_raw();
                    if !self_closing && !void {
                        state.depth += 1;
                    }
                    state.tag = None;
                    if state.depth == 0 {
                        let span = Span::new(start_pos, self.pos, start_line, start_col);
                        return Ok((
                            Token {
                                kind: TokenKind::HtmlEnd,
                                lexeme: html_chunk_string(text),
                                span,
                            },
                            HtmlChunk::Done,
                        ));
                    }
                } else {
                    text.push(b);
                    self.bump_raw();
                }
                continue;
            }
            // Element text content.
            if b == b'<' {
                match self.peek_byte(1) {
                    Some(b'/') => {
                        // Closing tag: copy `</name>` through verbatim.
                        text.push(b'<');
                        self.bump_raw();
                        text.push(b'/');
                        self.bump_raw();
                        loop {
                            if self.pos >= self.bytes.len() {
                                return Err(self.err_at(
                                    lit_line,
                                    lit_col,
                                    "unterminated HTML literal",
                                ));
                            }
                            let c = self.bytes[self.pos];
                            text.push(c);
                            self.bump_raw();
                            if c == b'>' {
                                break;
                            }
                        }
                        if state.depth == 0 {
                            return Err(self.err_at(
                                lit_line,
                                lit_col,
                                "unmatched closing tag in HTML literal",
                            ));
                        }
                        state.depth -= 1;
                        if state.depth == 0 {
                            let span = Span::new(start_pos, self.pos, start_line, start_col);
                            return Ok((
                                Token {
                                    kind: TokenKind::HtmlEnd,
                                    lexeme: html_chunk_string(text),
                                    span,
                                },
                                HtmlChunk::Done,
                            ));
                        }
                    }
                    Some(c) if c.is_ascii_alphabetic() => {
                        // Opening tag: capture the name, then scan
                        // attributes in tag state.
                        text.push(b'<');
                        self.bump_raw();
                        let mut name = String::new();
                        while self.pos < self.bytes.len() {
                            let c = self.bytes[self.pos];
                            if c.is_ascii_alphanumeric() || c == b'-' {
                                name.push(c.to_ascii_lowercase() as char);
                                text.push(c);
                                self.bump_raw();
                            } else {
                                break;
                            }
                        }
                        state.tag = Some(TagState { name, quote: None });
                    }
                    Some(b'!') => {
                        // Comment (`<!-- … -->`) or doctype-shaped
                        // (`<!… >`): copy through verbatim, no depth
                        // change.
                        if self.source[self.pos..].starts_with("<!--") {
                            loop {
                                if self.pos >= self.bytes.len() {
                                    return Err(self.err_at(
                                        lit_line,
                                        lit_col,
                                        "unterminated HTML literal",
                                    ));
                                }
                                if self.source[self.pos..].starts_with("-->") {
                                    for _ in 0..3 {
                                        text.push(self.bytes[self.pos]);
                                        self.bump_raw();
                                    }
                                    break;
                                }
                                text.push(self.bytes[self.pos]);
                                self.bump_raw();
                            }
                        } else {
                            loop {
                                if self.pos >= self.bytes.len() {
                                    return Err(self.err_at(
                                        lit_line,
                                        lit_col,
                                        "unterminated HTML literal",
                                    ));
                                }
                                let c = self.bytes[self.pos];
                                text.push(c);
                                self.bump_raw();
                                if c == b'>' {
                                    break;
                                }
                            }
                        }
                    }
                    _ => {
                        // Stray `<` in text (e.g. `1 < 2`) — literal.
                        text.push(b'<');
                        self.bump_raw();
                    }
                }
                continue;
            }
            text.push(b);
            self.bump_raw();
        }
    }

    /// Advance one byte, keeping line/column bookkeeping — the raw-text
    /// counterpart of `single` for HTML-mode scanning.
    fn bump_raw(&mut self) {
        if self.bytes[self.pos] == b'\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        self.pos += 1;
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

/// Rebuild an HTML chunk's accumulated bytes as a `String`. Chunks are
/// verbatim byte copies out of the (UTF-8) source, split only at ASCII
/// delimiters, so the bytes are always valid UTF-8.
fn html_chunk_string(text: Vec<u8>) -> String {
    String::from_utf8(text).expect("HTML chunks split on ASCII boundaries")
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}
