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
    /// **Deny** — non-doc comments must declare *why* they exist. Allowed:
    /// doc comments (`///`, `//!`, `/** */`, `/*! */`), comments starting
    /// with a label (`WHY:`, `SAFETY:`, `NOTE:`, `HACK:`, `TODO:`, `FIXME:`,
    /// `PERF:`), and comments containing a link (`http://`, `https://`) or
    /// ticket reference (`#1234`). Plain narrating comments like
    /// `// increment by 1` are forbidden — rename or extract instead.
    pub NO_COMMENTS,
    Deny,
    "non-doc comments must declare their purpose with a label, link, or ticket ref"
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

const ALLOWED_LABELS: &[&str] = &[
    "FIXME", "HACK", "NOTE", "PERF", "SAFETY", "TODO", "WHY",
];

/// Returns true if the given line of comment content carries a label, link,
/// or ticket reference that justifies the comment's existence.
fn line_is_justified(line: &str) -> bool {
    let trimmed = line
        .trim_start_matches(|c: char| c == '/' || c == '*')
        .trim_start();

    for label in ALLOWED_LABELS {
        if let Some(rest) = trimmed.strip_prefix(label) {
            if rest.starts_with(':') {
                return true;
            }
        }
    }

    if trimmed.contains("http://") || trimmed.contains("https://") {
        return true;
    }

    let bytes = trimmed.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'#' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
            return true;
        }
    }

    false
}

/// True if `a` (a `//` line comment) and `b` (the next comment) are part of
/// the same logical comment block: they sit on adjacent lines with only
/// whitespace before the second `//`.
fn line_comments_are_consecutive(src: &str, a_end: usize, b_start: usize) -> bool {
    let bytes = src.as_bytes();
    if a_end >= b_start || bytes.get(a_end) != Some(&b'\n') {
        return false;
    }
    bytes[a_end + 1..b_start]
        .iter()
        .all(|&c| c == b' ' || c == b'\t')
}

fn is_block_comment(src: &str, lo: usize) -> bool {
    src.as_bytes().get(lo..lo + 2) == Some(b"/*")
}

/// Group consecutive `//` line comments into logical comment blocks.
/// Block comments (`/* */`) are always their own group.
fn group_comments(src: &str, comments: &[(usize, usize)]) -> Vec<Vec<(usize, usize)>> {
    let mut groups: Vec<Vec<(usize, usize)>> = Vec::new();
    for &range in comments {
        let (lo, _) = range;
        let block = is_block_comment(src, lo);
        let extend = !block
            && groups
                .last()
                .and_then(|g| g.last().copied())
                .is_some_and(|(prev_lo, prev_hi)| {
                    !is_block_comment(src, prev_lo)
                        && line_comments_are_consecutive(src, prev_hi, lo)
                });
        if extend {
            groups.last_mut().unwrap().push(range);
        } else {
            groups.push(vec![range]);
        }
    }
    groups
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
            let comments = find_comments(src);
            for group in group_comments(src, &comments) {
                let group_text: String = group
                    .iter()
                    .map(|&(lo, hi)| &src[lo..hi])
                    .collect::<Vec<_>>()
                    .join("\n");
                if group_text.lines().any(line_is_justified) {
                    continue;
                }
                let (lo, _) = group[0];
                let (_, hi) = *group.last().unwrap();
                let span = Span::with_root_ctxt(
                    base + BytePos(lo as u32),
                    base + BytePos(hi as u32),
                );
                cx.opt_span_lint(NO_COMMENTS, Some(span), |diag| {
                    diag.primary_message(
                        "comment must declare its purpose — prefix with `WHY:`, `SAFETY:`, `NOTE:`, `HACK:`, `TODO:`, `FIXME:`, `PERF:`, or include a link or `#1234` ticket ref",
                    );
                });
            }
        }
    }
}
