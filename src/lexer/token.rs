use crate::error::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Ident,

    IntLit,
    FloatLit,
    HexLit,
    StringLit,

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
    Pipe,
    Amp,
    Comma,
    Colon,
    Dot,
    Question,
    Star,
    Ellipsis,

    KwMatch,
    KwMut,
    KwUse,
    KwSelf,
    KwImpl,
    KwExtern,
    KwWhile,
    KwFor,

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
            TokenKind::Pipe => "`|`",
            TokenKind::Amp => "`&`",
            TokenKind::Comma => "`,`",
            TokenKind::Colon => "`:`",
            TokenKind::Dot => "`.`",
            TokenKind::Question => "`?`",
            TokenKind::Star => "`*`",
            TokenKind::Ellipsis => "`...`",
            TokenKind::KwMatch => "`match`",
            TokenKind::KwMut => "`mut`",
            TokenKind::KwUse => "`use`",
            TokenKind::KwSelf => "`Self`",
            TokenKind::KwImpl => "`impl`",
            TokenKind::KwExtern => "`extern`",
            TokenKind::KwWhile => "`while`",
            TokenKind::KwFor => "`for`",
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
