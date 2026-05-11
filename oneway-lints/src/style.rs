use rustc_ast::ast;
use rustc_lint::{EarlyContext, EarlyLintPass, LintContext};
use rustc_session::{declare_lint, impl_lint_pass};
use rustc_span::{BytePos, FileName, Span};

// ---------------------------------------------------------------------------
// NO_GLOB_IMPORTS
// ---------------------------------------------------------------------------

declare_lint! {
    /// **Deny** — no wildcard imports. Every imported symbol must be named
    /// explicitly.
    pub NO_GLOB_IMPORTS,
    Deny,
    "no wildcard imports — name every imported symbol"
}

pub struct NoGlobImports;
impl_lint_pass!(NoGlobImports => [NO_GLOB_IMPORTS]);
impl EarlyLintPass for NoGlobImports {
    // TODO: implement check_item (detect UseTreeKind::Glob)
}

// ---------------------------------------------------------------------------
// NO_COMMENTS
// ---------------------------------------------------------------------------

declare_lint! {
    /// **Deny** — non-doc comments are forbidden. Doc comments (`///`, `//!`,
    /// `/** */`, `/*! */`) are allowed because they ship to docs.rs and
    /// describe a public API contract. Regular comments (`//`, `/* */`)
    /// usually narrate code that should rename, extract, or newtype itself
    /// into clarity instead.
    pub NO_COMMENTS,
    Deny,
    "non-doc comments are forbidden — rename, extract, or use a doc comment"
}

fn is_local_path(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy();
    !s.contains("/.cargo/")
        && !s.contains("/.rustup/")
        && !s.contains("/rustlib/")
        && !s.starts_with("<")
}

/// Scan source text and return byte ranges of every non-doc line and block
/// comment. Doc comments (`///`, `//!`, `/** */`, `/*! */`) are skipped so
/// they remain available for docs.rs output. Carefully skips comments inside
/// string, raw-string, byte-string, and char literals.
fn find_comments(src: &str) -> Vec<(usize, usize)> {
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut out = Vec::new();

    while i < len {
        let b = bytes[i];

        if b == b'"' {
            i += 1;
            while i < len {
                match bytes[i] {
                    b'\\' if i + 1 < len => i += 2,
                    b'"' => {
                        i += 1;
                        break;
                    }
                    _ => i += 1,
                }
            }
            continue;
        }

        // Raw string: r"..." / r#"..."# / br"..." / br#"..."#
        let raw_start = match (b, bytes.get(i + 1).copied()) {
            (b'r', Some(c)) if c == b'"' || c == b'#' => Some(i + 1),
            (b'b', Some(b'r')) if bytes.get(i + 2).is_some_and(|&c| c == b'"' || c == b'#') => {
                Some(i + 2)
            }
            _ => None,
        };
        if let Some(after_prefix) = raw_start {
            let mut j = after_prefix;
            let mut hashes = 0;
            while j < len && bytes[j] == b'#' {
                hashes += 1;
                j += 1;
            }
            if j < len && bytes[j] == b'"' {
                i = j + 1;
                while i < len {
                    if bytes[i] == b'"' {
                        let mut k = i + 1;
                        let mut close = 0;
                        while k < len && close < hashes && bytes[k] == b'#' {
                            close += 1;
                            k += 1;
                        }
                        if close == hashes {
                            i = k;
                            break;
                        }
                    }
                    i += 1;
                }
                continue;
            }
        }

        // Char literal or lifetime: scan forward to see whether a closing
        // `'` shows up before non-identifier chars.  If yes, char literal;
        // if no, lifetime (skip one byte).
        if b == b'\'' {
            let mut k = i + 1;
            let mut probe = 0;
            let mut found_close = false;
            while k < len && probe < 6 {
                if bytes[k] == b'\\' && k + 1 < len {
                    k += 2;
                    probe += 1;
                    continue;
                }
                if bytes[k] == b'\'' {
                    found_close = true;
                    break;
                }
                if !(bytes[k].is_ascii_alphanumeric() || bytes[k] == b'_') {
                    break;
                }
                k += 1;
                probe += 1;
            }
            if found_close {
                i = k + 1;
            } else {
                i += 1;
            }
            continue;
        }

        if b == b'/' && i + 1 < len {
            match bytes[i + 1] {
                b'/' => {
                    let start = i;
                    let third = bytes.get(i + 2).copied();
                    let fourth = bytes.get(i + 3).copied();
                    let is_outer_doc = third == Some(b'/') && fourth != Some(b'/');
                    let is_inner_doc = third == Some(b'!');
                    let is_doc = is_outer_doc || is_inner_doc;
                    while i < len && bytes[i] != b'\n' {
                        i += 1;
                    }
                    if !is_doc {
                        out.push((start, i));
                    }
                    continue;
                }
                b'*' => {
                    let start = i;
                    let third = bytes.get(i + 2).copied();
                    let fourth = bytes.get(i + 3).copied();
                    let is_outer_doc =
                        third == Some(b'*') && fourth != Some(b'*') && fourth != Some(b'/');
                    let is_inner_doc = third == Some(b'!');
                    let is_doc = is_outer_doc || is_inner_doc;
                    i += 2;
                    let mut depth: u32 = 1;
                    while i + 1 < len && depth > 0 {
                        if bytes[i] == b'/' && bytes[i + 1] == b'*' {
                            depth += 1;
                            i += 2;
                        } else if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                            depth -= 1;
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                    if !is_doc {
                        out.push((start, i));
                    }
                    continue;
                }
                _ => {}
            }
        }

        i += 1;
    }

    out
}

pub struct NoComments;
impl_lint_pass!(NoComments => [NO_COMMENTS]);

impl EarlyLintPass for NoComments {
    fn check_crate(&mut self, cx: &EarlyContext<'_>, _krate: &ast::Crate) {
        let source_map = cx.sess().source_map();
        for file in source_map.files().iter() {
            let path = match &file.name {
                FileName::Real(real) => real.local_path_if_available().to_path_buf(),
                _ => continue,
            };
            if !is_local_path(&path) {
                continue;
            }
            let Some(src) = file.src.as_ref() else { continue };
            let base = file.start_pos;
            for (lo, hi) in find_comments(src) {
                let span = Span::with_root_ctxt(
                    base + BytePos(lo as u32),
                    base + BytePos(hi as u32),
                );
                cx.opt_span_lint(NO_COMMENTS, Some(span), |diag| {
                    diag.primary_message(
                        "non-doc comment — rename, extract, or convert to a doc comment",
                    );
                });
            }
        }
    }
}
