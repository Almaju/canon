use std::fmt;
use std::sync::{Mutex, OnceLock};

/// The files a process has lexed, indexed by `Span::file`. Interned once
/// per path so a bundled package parsed on the first load keeps a valid
/// id across every later load in the same process (the LSP, `canon test`
/// over a directory). Id `0` is reserved for spans that come from no
/// file — synthesized items and tooling probes.
fn files() -> &'static Mutex<Vec<String>> {
    static FILES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    FILES.get_or_init(|| Mutex::new(Vec::new()))
}

/// The id every span lexed from `path` carries; the same path always
/// gets the same id.
pub fn file_id(path: &str) -> u32 {
    let mut files = files().lock().unwrap();
    match files.iter().position(|p| p == path) {
        Some(i) => i as u32 + 1,
        None => {
            files.push(path.to_string());
            files.len() as u32
        }
    }
}

/// The path behind a span's file id; `None` for id `0`.
pub fn file_path(id: u32) -> Option<String> {
    let files = files().lock().unwrap();
    id.checked_sub(1)
        .and_then(|i| files.get(i as usize).cloned())
}

/// Represents a location span in source code.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub column: u32,
    /// The file the span points into — see `file_id`.
    pub file: u32,
}

impl Span {
    pub fn new(start: usize, end: usize, line: u32, column: u32) -> Self {
        Self {
            start,
            end,
            line,
            column,
            file: 0,
        }
    }

    pub fn with_file(self, file: u32) -> Self {
        Self { file, ..self }
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
    /// phase, so this is an ordinary compile error.
    FormatError { message: String, span: Span },
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
