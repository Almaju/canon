//! Server state and diagnostics.
//!
//! Holds the open-document map and the diagnostics path — the compiler
//! integration that turns buffer text into `publishDiagnostics`
//! notifications.
use crate::ast::resolve_new_syntax;
use crate::checker;
use crate::error::CanonError;
use crate::lexer::Scanner;
use crate::parser::Parser;

use std::collections::HashMap;

use super::{json_escape, send_message, uri_to_path};

pub(super) struct LspServer {
    /// Open file contents keyed by URI.
    pub(super) files: HashMap<String, String>,
    pub(super) initialized: bool,
}

impl LspServer {
    pub(super) fn new() -> Self {
        Self {
            files: HashMap::new(),
            initialized: false,
        }
    }

    // -----------------------------------------------------------------------
    // Diagnostics
    // -----------------------------------------------------------------------

    pub(super) fn publish_diagnostics(&self, uri: &str) {
        let source = match self.files.get(uri) {
            Some(s) => s.as_str(),
            None => return,
        };
        let file_path = uri_to_path(uri);
        let errors = check_source(source, &file_path);
        let mut diags = String::from("[");
        for (i, err) in errors.iter().enumerate() {
            if i > 0 {
                diags.push(',');
            }
            let span = err.span();
            // LSP uses 0-based lines/columns; Canon uses 1-based.
            let line = if span.line > 0 { span.line - 1 } else { 0 };
            let col = if span.column > 0 { span.column - 1 } else { 0 };
            let end_col = col + (span.end.saturating_sub(span.start) as u32).max(1);
            diags.push_str(&format!(
                r#"{{"range":{{"start":{{"line":{},"character":{}}},"end":{{"line":{},"character":{}}}}},"severity":1,"source":"canon","message":"{}"}}"#,
                line, col, line, end_col,
                json_escape(err.message())
            ));
        }
        diags.push(']');

        let notification = format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":"{}","diagnostics":{}}}}}"#,
            json_escape(uri),
            diags
        );
        send_message(&notification);
    }
}

// ===========================================================================
// Compiler integration
// ===========================================================================

/// Check source text and return all errors.
///
/// `file_path` is the filesystem path of the file being checked (not a
/// URI). Every name the buffer references but doesn't define is resolved
/// through the compiler's own reference discovery (`load_import_closure`),
/// so the LSP and the compiler never disagree about what `Foo` is.
fn check_source(source: &str, file_path: &str) -> Vec<CanonError> {
    // 1. Parse the in-memory source.
    let mut scanner = Scanner::new(source);
    let tokens = match scanner.scan_tokens() {
        Ok(t) => t,
        Err(e) => return vec![e],
    };
    let mut parser = Parser::new(tokens);
    // Recover from syntax errors so the editor sees every parse error in
    // the buffer, not just the first. When the parse is broken we report
    // those errors and stop — running the checker on a partial AST would
    // bury the real syntax errors under spurious type errors.
    let (mut current, parse_errors) = parser.parse_recover();
    if !parse_errors.is_empty() {
        return parse_errors;
    }
    resolve_new_syntax(&mut current);

    // 2. Load the import closure of everything the buffer references,
    //    rooted at the buffer's directory — same rule as the compiler.
    let dir = std::path::Path::new(file_path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let mut combined_items = crate::loader::load_import_closure(&current.items, &dir);

    // 3. Build a combined module: imported items first, then the current
    //    file's items. `entry_items_start` tells the checker which items
    //    belong to the user's file (the only ones subject to ordering rules).
    let entry_items_start = combined_items.len();
    combined_items.extend(current.items);
    let combined = crate::ast::Module {
        items: combined_items,
        span: current.span,
    };

    let mut errors = checker::check_with_entry(&combined, entry_items_start);
    errors.retain(|e| e.message() != "no `main` entry point defined");
    errors
}
