//! `textDocument/completion` — the discovery providers the `->` / `.`
//! operator split was designed for (docs/src/spec/types-only.md, § The
//! One-Operator Endgame — "typing `.` offers fields and typing `->`
//! offers functions"):
//!
//!   * after `->`: every reachable declaration whose input product
//!     contains the piped value's type — constructors (family members
//!     and piped newtype wraps included) and named functions — plus
//!     the builtin pipe vocabulary applicable to that type (`Sum`,
//!     `Print`, `Joined`, …), gated by the checker's builtin table.
//!   * after `.`: the left value's product components (fields are
//!     accessed by type name), plus 1-based positional indexes when a
//!     component type repeats.
//!
//! Scope (v1, deliberate): the candidate universe is the open buffer's
//! declarations, its import closure (the compiler's own resolution for
//! names the buffer already references), and the entire bundled-stdlib
//! wrapper tier — so stdlib names the buffer doesn't reference yet are
//! discoverable, which is the point of the feature. Local project
//! files the buffer doesn't yet reference are *not* enumerated.
//! camelCase FFI bindings are excluded: the types-only surface reaches
//! them through their PascalCase result newtypes. The piped value's
//! type is resolved from the chain text on the cursor's line (plus
//! `->`/`.`-led continuation lines above it) using the checker's own
//! expression-typing rules. Degradation is graceful by construction:
//! an unresolvable left type returns the full unfiltered declaration
//! list for `->` and the empty list for `.` — never an error, never a
//! panic, and nothing here writes to the filesystem.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{self, resolve_new_syntax, Expr, FunctionDef, Item, Module, Param, TypeExpr};
use crate::checker::{self, SymbolTable};
use crate::lexer::{Scanner, TokenKind};
use crate::parser::Parser;

use super::handlers::format_type_expr;

/// One completion result. `label` is the text the editor inserts,
/// `detail` the signature line shown beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub detail: String,
    pub kind: CompletionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Constructor,
    Field,
    Function,
}

impl CompletionKind {
    /// The LSP `CompletionItemKind` numeric code.
    pub fn lsp_code(self) -> u32 {
        match self {
            CompletionKind::Function => 3,
            CompletionKind::Constructor => 4,
            CompletionKind::Field => 5,
        }
    }
}

/// Completion items for the given cursor position (0-based LSP
/// line/character). `file_path` roots the import-closure resolution,
/// exactly as diagnostics do; it need not exist on disk.
pub fn completion_items(
    source: &str,
    file_path: &str,
    line: u32,
    character: u32,
) -> Vec<CompletionItem> {
    let Some((trigger, left_text)) = completion_context(source, line, character) else {
        return Vec::new();
    };
    let (items, symbols) = declaration_universe(source, file_path);
    let left_ty = left_expr(&left_text)
        .map(|e| checker::expr_type_name_in_scope(&e, &symbols))
        .filter(|t| t != "<unknown>" && symbols.knows_type(t));
    match trigger {
        Trigger::Arrow => arrow_items(left_ty.as_deref(), &items, &symbols),
        Trigger::Dot => dot_items(left_ty.as_deref(), &symbols),
    }
}

// ===========================================================================
// Cursor context
// ===========================================================================

enum Trigger {
    Arrow,
    Dot,
}

/// Classify the cursor position: completion fires only directly after a
/// pipe arrow or a field dot (with an optional partially typed name
/// after either — the editor filters on that prefix itself). The `>`
/// trigger character alone is NOT a context: the preceding `-` is
/// verified here, so the closing `>` of `Option<Int>` never completes.
fn completion_context(source: &str, line: u32, character: u32) -> Option<(Trigger, String)> {
    let line_text = source.lines().nth(line as usize)?;
    let mut col = (character as usize).min(line_text.len());
    while col > 0 && !line_text.is_char_boundary(col) {
        col -= 1;
    }
    let prefix = &line_text[..col];
    let stripped = prefix
        .trim_end_matches(|c: char| c.is_ascii_alphanumeric() || c == '_')
        .trim_end();
    if let Some(left) = stripped.strip_suffix("->") {
        return Some((Trigger::Arrow, chain_text(source, line, left)));
    }
    if let Some(left) = stripped.strip_suffix('.') {
        return Some((Trigger::Dot, chain_text(source, line, left)));
    }
    None
}

/// The text whose trailing expression flows into the operator at the
/// cursor. Starts as the current line's prefix; while that prefix is
/// blank or itself a `->`/`.` continuation, the previous line is
/// prepended — Canon chains wrap with the operator opening the next
/// line.
fn chain_text(source: &str, line: u32, left_on_line: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut text = left_on_line.to_string();
    let mut ln = line as usize;
    for _ in 0..64 {
        let t = text.trim_start();
        if !(t.is_empty() || t.starts_with("->") || t.starts_with('.')) || ln == 0 {
            break;
        }
        ln -= 1;
        text = format!("{}\n{}", lines.get(ln).copied().unwrap_or(""), text);
    }
    text
}

/// Parse the trailing expression of `text` — the chain the cursor's
/// operator applies to. Lexes the text, walks the tokens backwards to
/// find where that expression starts (cutting at any token that can't
/// sit inside a value chain: an unmatched `(`, a block brace, a
/// declaration arrow, a top-level `*`, or a newline not followed by a
/// chain operator), then parses the remaining slice on its own.
fn left_expr(text: &str) -> Option<Expr> {
    let mut scanner = Scanner::new(text);
    let tokens = scanner.scan_tokens().ok()?;
    let toks: Vec<_> = tokens.iter().filter(|t| t.kind != TokenKind::Eof).collect();
    let mut end = toks.len();
    while end > 0 && toks[end - 1].kind == TokenKind::Newline {
        end -= 1;
    }
    if end == 0 {
        return None;
    }

    let mut start = 0usize;
    let mut depth = 0i32; // scanning backwards: `)` opens, `(` closes
    let mut after: Option<TokenKind> = None; // token to the right of `i`
    for i in (0..end).rev() {
        let kind = toks[i].kind;
        match kind {
            TokenKind::RParen => depth += 1,
            TokenKind::LParen => {
                if depth == 0 {
                    start = i + 1;
                    break;
                }
                depth -= 1;
            }
            // A newline inside a chain is always followed by the
            // operator that continues it; any other newline ends the
            // previous statement.
            TokenKind::Newline if depth == 0 => {
                if !matches!(after, Some(TokenKind::Arrow) | Some(TokenKind::Dot)) {
                    start = i + 1;
                    break;
                }
            }
            TokenKind::Star
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::LBracket
            | TokenKind::RBracket
            | TokenKind::FatArrow
            | TokenKind::Eq
            | TokenKind::Plus
            | TokenKind::Comma
            | TokenKind::Colon
            | TokenKind::Caret
            | TokenKind::Lt
            | TokenKind::Gt
            | TokenKind::Minus
                if depth == 0 =>
            {
                start = i + 1;
                break;
            }
            _ => {}
        }
        after = Some(kind);
    }
    if start >= end {
        return None;
    }

    let slice = text.get(toks[start].span.start..toks[end - 1].span.end)?;
    let mut scanner = Scanner::new(slice);
    let tokens = scanner.scan_tokens().ok()?;
    let mut parser = Parser::new(tokens);
    parser.parse_expression().ok()
}

// ===========================================================================
// Declaration universe
// ===========================================================================

/// Every declaration completion may offer, and the symbol table over
/// all of them (used to type the left expression and to walk alias /
/// variant chains). Buffer first — parsed with recovery, since the line
/// being completed is mid-edit — then its import closure, then the full
/// bundled-stdlib wrapper tier.
fn declaration_universe(source: &str, file_path: &str) -> (Vec<Item>, SymbolTable) {
    let mut items: Vec<Item> = Vec::new();
    let mut scanner = Scanner::new(source);
    if let Ok(tokens) = scanner.scan_tokens() {
        let mut parser = Parser::new(tokens);
        let (mut module, _errors) = parser.parse_recover();
        resolve_new_syntax(&mut module);
        let dir = Path::new(file_path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        items.extend(crate::loader::load_import_closure(&module.items, &dir));
        items.extend(module.items);
    }
    items.extend(crate::loader::bundled_wrapper_items().iter().cloned());
    let module = Module {
        items,
        span: crate::error::Span::new(0, 0, 1, 1),
    };
    let symbols = checker::symbols_for_tooling(&module);
    (module.items, symbols)
}

// ===========================================================================
// `->` completion — functions whose input product contains the type
// ===========================================================================

/// A pipeable declaration: `inputs` are the component type names of its
/// input product. Commutative calling means a compatible value pipes in
/// from any of them.
struct Candidate {
    label: String,
    detail: String,
    kind: CompletionKind,
    inputs: Vec<String>,
}

fn collect_candidates(items: &[Item]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for item in items {
        let (label, detail, kind, inputs) = match item {
            Item::Function(f) => {
                let label = if f.name.name == "Self" {
                    match &f.receiver {
                        Some(r) => r.name.clone(),
                        None => continue,
                    }
                } else {
                    f.name.name.clone()
                };
                // `main` is the entry (no one pipes into it); camelCase
                // is the FFI boundary, reached through PascalCase
                // result newtypes.
                if label == "main" || !label.starts_with(|c: char| c.is_ascii_uppercase()) {
                    continue;
                }
                let mut inputs = Vec::new();
                if f.name.name != "Self" {
                    if let Some(r) = &f.receiver {
                        inputs.push(r.name.clone());
                    }
                }
                flatten_param_components(&f.params, &mut inputs);
                let kind = if f.name.name == "Self" || f.receiver.is_none() {
                    CompletionKind::Constructor
                } else {
                    CompletionKind::Function
                };
                (label, function_detail(f), kind, inputs)
            }
            Item::TypeDef(td) => {
                // A product constructs from its components, a newtype
                // wraps its base — both are piped constructions even
                // without a bodied arrow. Unions and shapes aren't.
                let inputs: Vec<String> = match &td.body {
                    TypeExpr::Product { fields, .. } => fields
                        .iter()
                        .filter_map(|f| f.simple_name().map(str::to_string))
                        .collect(),
                    TypeExpr::Named { name, .. } => vec![name.clone()],
                    _ => continue,
                };
                let detail = format!("{} = {}", td.name.name, format_type_expr(&td.body));
                (
                    td.name.name.clone(),
                    detail,
                    CompletionKind::Constructor,
                    inputs,
                )
            }
        };
        match index.get(&label) {
            Some(&i) => {
                // Constructor families and typedef + constructor pairs
                // merge into one item: the union of pipeable inputs,
                // preferring a signature detail over a typedef body.
                for input in inputs {
                    if !out[i].inputs.contains(&input) {
                        out[i].inputs.push(input);
                    }
                }
                if !out[i].detail.contains("=>") && detail.contains("=>") {
                    out[i].detail = detail;
                }
            }
            None => {
                index.insert(label.clone(), out.len());
                out.push(Candidate {
                    label,
                    detail,
                    kind,
                    inputs,
                });
            }
        }
    }
    out
}

/// Flatten declaration params into input-product component names, the
/// same way the checker registers commutative keys (a product param
/// contributes each component).
fn flatten_param_components(params: &[Param], inputs: &mut Vec<String>) {
    for p in params {
        match &p.ty {
            TypeExpr::Named { .. } => {
                if let Some(n) = p.ty.simple_name() {
                    inputs.push(n.to_string());
                }
            }
            TypeExpr::Product { fields, .. } => {
                for f in fields {
                    if let Some(n) = f.simple_name() {
                        inputs.push(n.to_string());
                    }
                }
            }
            _ => {}
        }
    }
}

/// A declaration-shaped signature: `(A * B) => C`, `A => B`,
/// `Unit => X`.
fn function_detail(f: &FunctionDef) -> String {
    let ret = format_type_expr(&f.return_ty);
    let mut comps: Vec<String> = Vec::new();
    if f.name.name != "Self" {
        if let Some(r) = &f.receiver {
            comps.push(r.name.clone());
        }
    }
    for p in &f.params {
        comps.push(format_type_expr(&p.ty));
    }
    match comps.len() {
        0 => format!("Unit => {}", ret),
        1 => format!("{} => {}", comps[0], ret),
        _ => format!("({}) => {}", comps.join(" * "), ret),
    }
}

fn arrow_items(
    left_ty: Option<&str>,
    items: &[Item],
    symbols: &SymbolTable,
) -> Vec<CompletionItem> {
    let candidates = collect_candidates(items);
    let mut out: Vec<CompletionItem> = Vec::new();
    match left_ty {
        Some(ty) => {
            let left_set = widening_set(symbols, ty);
            for c in &candidates {
                if c.inputs.iter().any(|i| compatible(symbols, &left_set, i)) {
                    out.push(CompletionItem {
                        label: c.label.clone(),
                        detail: c.detail.clone(),
                        kind: c.kind,
                    });
                }
            }
            // Builtin pipe vocabulary, gated on the checker's builtin
            // table for the value's erased base type(s) — scalar
            // newtypes erase, so the whole widening chain is probed.
            for (pascal, camel) in ast::builtin_pipe_aliases() {
                let hit = left_set
                    .iter()
                    .map(|base| (base, checker::method_return_type(base, camel)))
                    .find(|(_, ret)| ret != "<unknown>");
                if let Some((base, ret)) = hit {
                    out.push(CompletionItem {
                        label: (*pascal).to_string(),
                        detail: format!("{} => {}", base, ret),
                        kind: CompletionKind::Function,
                    });
                }
            }
        }
        None => {
            // Unresolvable left value: degrade to the full declaration
            // list — better than nothing, never an error.
            for c in &candidates {
                out.push(CompletionItem {
                    label: c.label.clone(),
                    detail: c.detail.clone(),
                    kind: c.kind,
                });
            }
            for (pascal, _) in ast::builtin_pipe_aliases() {
                out.push(CompletionItem {
                    label: (*pascal).to_string(),
                    detail: "compiler builtin".to_string(),
                    kind: CompletionKind::Function,
                });
            }
        }
    }
    // Wherever ordering is discretionary, alphabetical order. The sort
    // is stable, so on a label collision the specific candidate (pushed
    // before the builtin fallback) survives the dedup.
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out.dedup_by(|a, b| a.label == b.label);
    out
}

/// The type names a value of type `name` can act as: itself, its alias
/// chain (`Uppercased` → `String` — scalar newtypes erase), and — for a
/// variant — its parent union's chain (`True` pipes wherever a `Bool`
/// does).
fn widening_set(symbols: &SymbolTable, name: &str) -> Vec<String> {
    let mut set = alias_chain(symbols, name);
    if let Some(parent) = symbols.variant_of.get(name) {
        for n in alias_chain(symbols, parent) {
            if !set.contains(&n) {
                set.push(n);
            }
        }
    }
    set
}

fn alias_chain(symbols: &SymbolTable, name: &str) -> Vec<String> {
    let mut out = vec![name.to_string()];
    let mut cur = name.to_string();
    for _ in 0..20 {
        match symbols.aliases.get(&cur) {
            Some(next) => {
                if out.contains(next) {
                    break;
                }
                out.push(next.clone());
                cur = next.clone();
            }
            None => break,
        }
    }
    out
}

/// Whether a value whose widening set is `left` can fill an input
/// component declared as `component`: the chains intersect (`"hi"`
/// (String) fills a `Greeting` component when `Greeting = String`).
fn compatible(symbols: &SymbolTable, left: &[String], component: &str) -> bool {
    let comp_chain = alias_chain(symbols, component);
    left.iter().any(|l| comp_chain.contains(l))
}

// ===========================================================================
// `.` completion — the value's components
// ===========================================================================

fn dot_items(left_ty: Option<&str>, symbols: &SymbolTable) -> Vec<CompletionItem> {
    let Some(ty) = left_ty else {
        return Vec::new();
    };
    let mut cur = ty.to_string();
    for _ in 0..20 {
        if let Some(fields) = symbols.product_fields.get(&cur) {
            let mut out: Vec<CompletionItem> = Vec::new();
            for (i, field) in fields.iter().enumerate() {
                if !out.iter().any(|item| item.label == *field) {
                    out.push(CompletionItem {
                        label: field.clone(),
                        detail: format!("component {} of {}", i + 1, cur),
                        kind: CompletionKind::Field,
                    });
                }
            }
            // A repeated component type is only reachable positionally
            // (1-based, like all Canon indexing) — offer the indexes.
            let unique: HashSet<&String> = fields.iter().collect();
            if unique.len() != fields.len() {
                for (i, field) in fields.iter().enumerate() {
                    out.push(CompletionItem {
                        label: (i + 1).to_string(),
                        detail: format!("{} (component {})", field, i + 1),
                        kind: CompletionKind::Field,
                    });
                }
            }
            // Declaration order, not alphabetical: position is meaning.
            return out;
        }
        match symbols.aliases.get(&cur) {
            Some(next) => cur = next.clone(),
            None => return Vec::new(),
        }
    }
    Vec::new()
}
