use crate::error::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Ident,

    IntLit,
    FloatLit,
    HexLit,
    StringLit,
    /// A raw fragment of an HTML literal that is followed by a `{…}`
    /// interpolation (the scanner has already consumed the `{`).
    HtmlText,
    /// The final raw fragment of an HTML literal (the literal's root
    /// element closed with this fragment).
    HtmlEnd,

    Eq,
    FatArrow,
    Arrow,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Lt,
    Gt,
    Plus,
    Comma,
    Colon,
    Dot,
    Question,
    Star,
    Caret,
    Minus,

    KwMut,
    KwSelf,
    KwImpl,

    Newline,
    Eof,
}

impl std::fmt::Display for TokenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TokenKind::Ident => "identifier",
            TokenKind::IntLit => "int literal",
            TokenKind::FloatLit => "float literal",
            TokenKind::HexLit => "hex literal",
            TokenKind::StringLit => "string literal",
            TokenKind::HtmlText => "HTML literal fragment",
            TokenKind::HtmlEnd => "HTML literal",
            TokenKind::Eq => "`=`",
            TokenKind::FatArrow => "`=>`",
            TokenKind::Arrow => "`->`",
            TokenKind::LParen => "`(`",
            TokenKind::RParen => "`)`",
            TokenKind::LBrace => "`{`",
            TokenKind::RBrace => "`}`",
            TokenKind::LBracket => "`[`",
            TokenKind::RBracket => "`]`",
            TokenKind::Lt => "`<`",
            TokenKind::Gt => "`>`",
            TokenKind::Plus => "`+`",
            TokenKind::Comma => "`,`",
            TokenKind::Colon => "`:`",
            TokenKind::Dot => "`.`",
            TokenKind::Question => "`?`",
            TokenKind::Star => "`*`",
            TokenKind::Caret => "`^`",
            TokenKind::Minus => "`-`",
            TokenKind::KwMut => "`mut`",
            TokenKind::KwSelf => "`Self`",
            TokenKind::KwImpl => "`impl`",
            TokenKind::Newline => "newline",
            TokenKind::Eof => "end of file",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub lexeme: String,
    pub span: Span,
}
