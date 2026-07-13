use std::fmt;

/// Represents a location span in source code.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub column: u32,
}

impl Span {
    pub fn new(start: usize, end: usize, line: u32, column: u32) -> Self {
        Self {
            start,
            end,
            line,
            column,
        }
    }
}

/// The set of all error kinds produced by the Canon compiler.
#[derive(Debug, Clone)]
pub enum CanonError {
    /// An error produced during lexical analysis.
    LexError { message: String, span: Span },
    /// An error produced during parsing.
    ParseError { message: String, span: Span },
    /// An error produced during type/sort checking.
    CheckError { message: String, span: Span },
    /// A divergence from canonical form — formatting is a compiler
    /// phase, so this is an ordinary compile error. Carries the path of
    /// the offending file: unlike the other kinds, format errors are
    /// raised per source file of a multi-file load, not against the
    /// entry, and the span points into that file.
    FormatError {
        message: String,
        path: String,
        span: Span,
    },
}

impl CanonError {
    /// Returns a reference to the span associated with this error.
    pub fn span(&self) -> &Span {
        match self {
            CanonError::LexError { span, .. } => span,
            CanonError::ParseError { span, .. } => span,
            CanonError::CheckError { span, .. } => span,
            CanonError::FormatError { span, .. } => span,
        }
    }

    /// Returns a reference to the message associated with this error.
    pub fn message(&self) -> &str {
        match self {
            CanonError::LexError { message, .. } => message,
            CanonError::ParseError { message, .. } => message,
            CanonError::CheckError { message, .. } => message,
            CanonError::FormatError { message, .. } => message,
        }
    }

    /// Returns the name of the compiler phase that produced this error.
    fn phase(&self) -> &'static str {
        match self {
            CanonError::LexError { .. } => "lex error",
            CanonError::ParseError { .. } => "parse error",
            CanonError::CheckError { .. } => "check error",
            CanonError::FormatError { .. } => "format error",
        }
    }
}

impl fmt::Display for CanonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let span = self.span();
        write!(
            f,
            "{} at {}:{}: {}",
            self.phase(),
            span.line,
            span.column,
            self.message()
        )
    }
}

impl std::error::Error for CanonError {}

/// A convenience `Result` type that uses `CanonError` as the error variant.
pub type Result<T> = std::result::Result<T, CanonError>;
