//! `textDocument/*` request and notification handlers, plus the hover
//! and go-to-definition machinery they depend on.
use crate::ast::{resolve_new_syntax, FunctionDef, Item, Module, TypeDef, TypeExpr};
use crate::bindgen::camel_to_kebab;
use crate::formatter;
use crate::lexer::Scanner;
use crate::loader::{kebab_case, BUNDLED_PACKAGES};
use crate::manifest::{self, ImportSource};
use crate::parser::Parser;

use std::path::{Path, PathBuf};

use super::state::LspServer;
use super::{
    json_escape, json_get_nested_string, json_get_position, json_unescape, send_message,
    send_response, uri_to_path, word_at_position,
};

impl LspServer {
    // -----------------------------------------------------------------------
    // textDocument/didOpen
    // -----------------------------------------------------------------------

    pub(super) fn handle_did_open(&mut self, msg: &str) {
        if let Some(uri) = json_get_nested_string(msg, "textDocument", "uri") {
            if !uri.ends_with(".can") {
                return;
            }
            if let Some(text) = json_get_nested_string(msg, "textDocument", "text") {
                let text = json_unescape(&text);
                self.files.insert(uri.clone(), text);
                self.publish_diagnostics(&uri);
            }
        }
    }

    // -----------------------------------------------------------------------
    // textDocument/didChange  (full sync — change kind 1)
    // -----------------------------------------------------------------------

    pub(super) fn handle_did_change(&mut self, msg: &str) {
        if let Some(uri) = json_get_nested_string(msg, "textDocument", "uri") {
            if !uri.ends_with(".can") {
                return;
            }
            // Full document text is in contentChanges[0].text
            if let Some(text) = super::extract_content_change_text(msg) {
                let text = json_unescape(&text);
                self.files.insert(uri.clone(), text);
                self.publish_diagnostics(&uri);
            }
        }
    }

    // -----------------------------------------------------------------------
    // textDocument/didSave
    // -----------------------------------------------------------------------

    pub(super) fn handle_did_save(&mut self, msg: &str) {
        if let Some(uri) = json_get_nested_string(msg, "textDocument", "uri") {
            if !uri.ends_with(".can") {
                return;
            }
            // If the save notification includes text, use it
            if let Some(text) = super::extract_param_text(msg) {
                let text = json_unescape(&text);
                self.files.insert(uri.clone(), text);
            } else {
                // Otherwise re-read from disk
                let path = uri_to_path(&uri);
                if let Ok(text) = std::fs::read_to_string(&path) {
                    self.files.insert(uri.clone(), text);
                }
            }
            self.publish_diagnostics(&uri);
        }
    }

    // -----------------------------------------------------------------------
    // textDocument/didClose
    // -----------------------------------------------------------------------

    pub(super) fn handle_did_close(&mut self, msg: &str) {
        if let Some(uri) = json_get_nested_string(msg, "textDocument", "uri") {
            self.files.remove(&uri);
            // Clear diagnostics for the closed file
            let notification = format!(
                r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":"{}","diagnostics":[]}}}}"#,
                json_escape(&uri)
            );
            send_message(&notification);
        }
    }

    // -----------------------------------------------------------------------
    // Hover
    // -----------------------------------------------------------------------

    pub(super) fn handle_hover(&self, msg: &str, id: Option<String>) {
        let id = match id {
            Some(id) => id,
            None => return,
        };

        let uri = match json_get_nested_string(msg, "textDocument", "uri") {
            Some(u) => u,
            None => {
                send_response(&id, "null");
                return;
            }
        };
        let (line, character) = match json_get_position(msg) {
            Some(pos) => pos,
            None => {
                send_response(&id, "null");
                return;
            }
        };

        let source = match self.files.get(&uri) {
            Some(s) => s.as_str(),
            None => {
                send_response(&id, "null");
                return;
            }
        };

        // Parse the file to get definitions
        let module = match parse_source(source) {
            Some(m) => m,
            None => {
                send_response(&id, "null");
                return;
            }
        };

        // Find the word at the cursor position
        let word = match word_at_position(source, line, character) {
            Some(w) => w,
            None => {
                send_response(&id, "null");
                return;
            }
        };

        // Look up the word in definitions
        if let Some(info) = lookup_hover_info(&module, &word) {
            let hover = format!(
                r#"{{"contents":{{"kind":"markdown","value":"{}"}}}}"#,
                json_escape(&info)
            );
            send_response(&id, &hover);
        } else {
            send_response(&id, "null");
        }
    }

    // -----------------------------------------------------------------------
    // Go to Definition
    // -----------------------------------------------------------------------

    pub(super) fn handle_definition(&self, msg: &str, id: Option<String>) {
        let id = match id {
            Some(id) => id,
            None => return,
        };

        let uri = match json_get_nested_string(msg, "textDocument", "uri") {
            Some(u) => u,
            None => {
                send_response(&id, "null");
                return;
            }
        };
        let (line, character) = match json_get_position(msg) {
            Some(pos) => pos,
            None => {
                send_response(&id, "null");
                return;
            }
        };

        let source = match self.files.get(&uri) {
            Some(s) => s.as_str(),
            None => {
                send_response(&id, "null");
                return;
            }
        };

        let module = match parse_source(source) {
            Some(m) => m,
            None => {
                send_response(&id, "null");
                return;
            }
        };

        let word = match word_at_position(source, line, character) {
            Some(w) => w,
            None => {
                send_response(&id, "null");
                return;
            }
        };

        // Collect all definitions and find matching one
        let current_path = uri_to_path(&uri);

        // Phase 0: if the current file is a binding file, jump straight
        // to the matching WIT declaration. The binding `.can` file is a
        // derived artifact — the WIT is the authoritative source of
        // truth, so go-to-def lands the user on the spec rather than on
        // the regenerable shim. Falls through to the ordinary lookup if
        // the WIT can't be resolved (WIT file not on disk, decl not
        // found).
        let wit_location =
            resolve_to_wit_declaration(&current_path, &word).map(|(p, line, col)| {
                (
                    path_to_uri(&p.to_string_lossy()),
                    crate::error::Span {
                        start: 0,
                        end: 0,
                        line,
                        column: col,
                    },
                )
            });

        // Phase 1: definition in the current file.
        let location = wit_location
            .or_else(|| find_definition(&module, &word).map(|span| (uri.clone(), span)));

        // Phase 2: follow `use` imports if not found locally.
        let location =
            location.or_else(|| find_definition_in_imports(&module, &word, &current_path));

        if let Some((def_uri, span)) = location {
            let def_line = if span.line > 0 { span.line - 1 } else { 0 };
            let def_col = if span.column > 0 { span.column - 1 } else { 0 };
            let end_col = def_col + (span.end.saturating_sub(span.start) as u32).max(1);
            let result = format!(
                r#"{{"uri":"{}","range":{{"start":{{"line":{},"character":{}}},"end":{{"line":{},"character":{}}}}}}}"#,
                json_escape(&def_uri),
                def_line,
                def_col,
                def_line,
                end_col
            );
            send_response(&id, &result);
        } else {
            send_response(&id, "null");
        }
    }

    // -----------------------------------------------------------------------
    // Formatting
    // -----------------------------------------------------------------------

    pub(super) fn handle_formatting(&self, msg: &str, id: Option<String>) {
        let id = match id {
            Some(id) => id,
            None => return,
        };

        let uri = match json_get_nested_string(msg, "textDocument", "uri") {
            Some(u) => u,
            None => {
                send_response(&id, "null");
                return;
            }
        };

        let source = match self.files.get(&uri) {
            Some(s) => s.as_str(),
            None => {
                send_response(&id, "null");
                return;
            }
        };

        let formatted = match formatter::format(source) {
            Ok(f) => f,
            Err(_) => {
                // If the source can't be parsed, return no edits.
                send_response(&id, "[]");
                return;
            }
        };

        if formatted == source {
            // Already formatted — return empty edit list.
            send_response(&id, "[]");
            return;
        }

        // Return a single TextEdit that replaces the entire document.
        let line_count = source.lines().count() + 1;
        let last_line_len = source.lines().last().map_or(0, |l| l.len());
        let result = format!(
            r#"[{{"range":{{"start":{{"line":0,"character":0}},"end":{{"line":{},"character":{}}}}},"newText":"{}"}}]"#,
            line_count,
            last_line_len,
            json_escape(&formatted)
        );
        send_response(&id, &result);
    }
}

/// Parse source text into a Module, returning None on failure.
fn parse_source(source: &str) -> Option<Module> {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens().ok()?;
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse().ok()?;
    resolve_new_syntax(&mut module);
    Some(module)
}

// ===========================================================================
// Hover — type/signature lookup
// ===========================================================================

fn lookup_hover_info(module: &Module, name: &str) -> Option<String> {
    // Check user-defined items first.
    for item in &module.items {
        match item {
            Item::TypeDef(td) => {
                if td.name.name == name {
                    return Some(format_type_def_hover(td));
                }
                if let Some(info) = variant_hover(td, name) {
                    return Some(info);
                }
            }
            Item::Function(func) => {
                let display_name = effective_function_name(func);
                if display_name == name {
                    return Some(format_function_hover(func));
                }
            }
        }
    }
    // Fall back to built-in type descriptions.
    builtin_hover(name)
}

/// Hover info for built-in types, capabilities, and well-known constructors.
fn builtin_hover(name: &str) -> Option<String> {
    let desc = match name {
        // Core numeric / text types
        "Int"    => "```canon\nInt\n```\nA 64-bit signed integer.",
        "Float"  => "```canon\nFloat\n```\nA 64-bit floating-point number.",
        "Hex"    => "```canon\nHex\n```\nA 64-bit unsigned integer displayed in hexadecimal.",
        "String" => "```canon\nString = Byte^*\n```\nA UTF-8 string.",
        "Byte"   => "```canon\nByte\n```\nA single byte (u8).",
        "Bytes"  => "```canon\nBytes = Byte^*\n```\nA byte sequence.",
        // Unit / Never
        "Unit"  => "```canon\nUnit\n```\nThe singleton type with exactly one value: `Unit`.",
        "Never" => "```canon\nNever\n```\nThe uninhabited type: a function returning `Never` does not return.",
        // Bool and its variants
        "Bool"  => "```canon\nBool = False + True\n```\nThe built-in boolean type.",
        "False" => "```canon\nFalse\n```\nVariant of `Bool`. The falsy value.",
        "True"  => "```canon\nTrue\n```\nVariant of `Bool`. The truthy value.",
        // Ord and its variants
        "Ord"     => "```canon\nOrd = Equal + Greater + Less\n```\nComparison result.",
        "Equal"   => "```canon\nEqual\n```\nVariant of `Ord`.",
        "Greater" => "```canon\nGreater\n```\nVariant of `Ord`.",
        "Less"    => "```canon\nLess\n```\nVariant of `Ord`.",
        // Generic containers
        "List"   => "```canon\nList<T>\n```\nAn ordered sequence of values of type `T`.",
        "Map"    => "```canon\nMap<K, V>\n```\nA sorted key-value map. `K` must implement `Ord`.",
        "Set"    => "```canon\nSet<T>\n```\nA sorted set of values. `T` must implement `Ord`.",
        "Option" => "```canon\nOption<T> = None + Some<T>\n```\nAn optional value.",
        "Some"   => "```canon\nSome<T>\n```\nVariant of `Option<T>`: value is present.",
        "None"   => "```canon\nNone\n```\nVariant of `Option<T>`: value is absent.",
        "Result" => "```canon\nResult<T, E> = Err<E> + Ok<T>\n```\nA fallible computation.",
        "Ok"     => "```canon\nOk<T>\n```\nVariant of `Result<T, E>`: success.",
        "Err"    => "```canon\nErr<E>\n```\nVariant of `Result<T, E>`: failure.",
        // Capabilities
        "Clock"      => "```canon\nClock\n```\nCapability for reading the current time (non-suspending).",
        "Filesystem" => "```canon\nFilesystem\n```\nCapability for filesystem I/O (suspending; makes the function async).",
        "Network"    => "```canon\nNetwork\n```\nCapability for network I/O (suspending; makes the function async).",

        "Stderr"     => "```canon\nStderr\n```\nCapability for writing to stderr (non-suspending).",
        "Stdin"      => "```canon\nStdin\n```\nCapability for reading from stdin (non-suspending).",
        "Stdout"     => "```canon\nStdout\n```\nCapability for writing to stdout (non-suspending).",
        _ => return None,
    };
    Some(desc.to_string())
}

/// Check if `name` is a variant inside this type definition.
fn variant_hover(td: &TypeDef, name: &str) -> Option<String> {
    if let TypeExpr::Union { variants, .. } = &td.body {
        for v in variants {
            if let Some(vname) = v.simple_name() {
                if vname == name {
                    let info = format!("```canon\n{}\n```\nVariant of `{}`", name, td.name.name);
                    return Some(info);
                }
            }
        }
    }
    None
}

fn effective_function_name(func: &FunctionDef) -> &str {
    // After resolve_new_syntax, constructor funcs have name "Self" and receiver = type.
    // For hover we want to match on the original name.
    if func.name.name == "Self" {
        if let Some(recv) = &func.receiver {
            return &recv.name;
        }
    }
    &func.name.name
}

fn format_type_def_hover(td: &TypeDef) -> String {
    let generics = format_generic_params(&td.generic_params);
    let body = format_type_expr(&td.body);
    format!("```canon\n{}{} = {}\n```", td.name.name, generics, body)
}

fn format_function_hover(func: &FunctionDef) -> String {
    let name = effective_function_name(func);
    let generics = format_generic_params(&func.generic_params);
    let params: Vec<String> = func
        .params
        .iter()
        .map(|p| format_type_expr(&p.ty))
        .collect();

    let mut sig = String::new();
    if let Some(recv) = &func.receiver {
        if func.name.name != "Self" {
            // Trait-style: Receiver.name = ...
            sig.push_str(&format!("{}.{}{}", recv.name, func.name.name, generics));
        } else {
            // Constructor: TypeName = ...
            sig.push_str(&format!("{}{}", recv.name, generics));
        }
    } else {
        sig.push_str(&format!("{}{}", name, generics));
    }

    let ret = format_type_expr(&func.return_ty);
    if params.is_empty() {
        sig.push_str(&format!(" = () -> {}", ret));
    } else {
        sig.push_str(&format!(" = ({}) -> {}", params.join(", "), ret));
    }
    format!("```canon\n{}\n```", sig)
}

fn format_generic_params(params: &[crate::ast::GenericParam]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = params
        .iter()
        .map(|p| {
            if let Some(bound) = &p.bound {
                format!("{}: {}", p.name.name, format_type_expr(bound))
            } else {
                p.name.name.clone()
            }
        })
        .collect();
    format!("({})", parts.join(", "))
}

fn format_type_expr(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named { name, generics, .. } => {
            if generics.is_empty() {
                name.clone()
            } else {
                let gs: Vec<String> = generics.iter().map(format_type_expr).collect();
                format!("{}({})", name, gs.join(", "))
            }
        }
        TypeExpr::Union { variants, .. } => {
            let vs: Vec<String> = variants.iter().map(format_type_expr).collect();
            vs.join(" + ")
        }
        TypeExpr::Product { fields, .. } => {
            let fs: Vec<String> = fields.iter().map(format_type_expr).collect();
            fs.join(" * ")
        }
        TypeExpr::Repeat { ty, count, .. } => {
            format!("{}^{}", format_type_expr(ty), count)
        }
        TypeExpr::Spread { ty, .. } => {
            format!("{}^*", format_type_expr(ty))
        }
        TypeExpr::Function {
            params, return_ty, ..
        } => {
            let ps: Vec<String> = params.iter().map(format_type_expr).collect();
            format!("({}) -> {}", ps.join(", "), format_type_expr(return_ty))
        }
    }
}

// ===========================================================================
// Go to Definition
// ===========================================================================

/// A definition we collected from the AST.
struct DefInfo {
    name: String,
    span: crate::error::Span,
}

fn find_definition(module: &Module, name: &str) -> Option<crate::error::Span> {
    let defs = collect_definitions(module);
    for def in &defs {
        if def.name == name {
            return Some(def.span);
        }
    }
    None
}

/// Convert a filesystem path to a `file://` URI.
fn path_to_uri(path: &str) -> String {
    if path.starts_with('/') {
        format!("file://{}", path)
    } else {
        format!("file:///{}", path)
    }
}

// -----------------------------------------------------------------------
// Slice 5: go-to-definition from a bindgen file → WIT source.
//
// `canon install` writes bindgen output with bare `extern Wasm`
// markers; the canonical-ABI URN lives in `<bindgen>/_install.toml`.
// The WIT file itself — the actual source-of-truth definition the
// bindgen was generated from — is reachable through the project
// manifest's `[imports]` table.
//
// The flow when the LSP gets a textDocument/definition request on a
// bindgen file:
//
//   1. `urn_for_bindgen_file(current_file)` — is this file a bindgen
//      artifact? Returns the file's interface URN if so.
//   2. `wit_file_for_urn(urn, current_file)` — consult the manifest's
//      `[imports]` table to find the matching `.wit` file on disk.
//   3. `find_wit_decl(wit_path, kebab_name)` — scan the WIT for a
//      declaration matching the (camelCase → kebab) version of the
//      identifier the user clicked.
//
// Returns `(wit_file_path, line, column)` (1-indexed). All failures
// return `None` so the LSP can fall back to the ordinary in-file /
// follow-imports resolution.
// -----------------------------------------------------------------------

/// Top-level entry point for the slice-5 navigation. Combines the three
/// helpers below. Returns the target file path plus a 1-indexed line and
/// column inside it.
fn resolve_to_wit_declaration(current_file: &str, word: &str) -> Option<(PathBuf, u32, u32)> {
    let urn = urn_for_bindgen_file(current_file)?;
    let (wit_path, _iface) = wit_file_for_urn(&urn, current_file)?;
    let kebab = camel_to_kebab(word);
    let (line, col) = find_wit_decl(&wit_path, &kebab)?;
    Some((wit_path, line, col))
}

/// If the cursor sits on a `bindings "<urn>"` directive, resolve the
/// URN to its backing WIT and return the target location. With a
/// `#fn-suffix` in the URN, the cursor lands on that specific function;
/// without one, it lands on the `interface <name>` declaration.
///
/// Works from *any* file (hand-written `std/` wrappers, generated
/// `bindgen/` files, or user-project code) — the lookup is anchored
/// at the file's project root, not at a `bindgen/` directory. This is
/// what makes the `bindings` directive's URN clickable as a navigation
/// hint regardless of context.
/// Determine the WIT interface URN that backs the given source file,
/// if it's a binding file. The URN is derived from the file's vendored
/// path (`<ns>/<name>@<version>/<iface>.can` — see docs/src/spec/modules.md), for
/// both bundled package files and files under a project's `bindgen/`
/// or `deps/` root.
fn urn_for_bindgen_file(current_file: &str) -> Option<String> {
    // Bundled-package lookup.
    for pkg in BUNDLED_PACKAGES {
        for file in pkg.files {
            if file.abs_path == current_file {
                return crate::loader::urn_base_for_bundled_path(file.path);
            }
        }
    }

    // Project vendored-tree lookup.
    let path = PathBuf::from(current_file);
    let project_root = find_project_root_from(&path)?;
    for root in ["bindgen", "deps"] {
        if let Ok(rel) = path.strip_prefix(project_root.join(root)) {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            return crate::loader::urn_base_for_bundled_path(&rel_str);
        }
    }
    None
}

/// Locate the `.wit` file backing a given URN. The lookup goes through
/// the project manifest's `[imports]` table: for each entry whose key
/// is a prefix of the URN's `<ns>/<pkg>`, the entry's source path
/// points at either a single `.wit` (return that) or a directory (look
/// for `<pkg>.wit` inside).
///
/// For bundled bindgen files (the compiler's own `canon/std`), the
/// project root we walk up to is `packages/canon/std/`, whose
/// manifest's `[imports]` declares `"wasi" = "../../../wit-vendor/wasi"`.
/// The lookup resolves relative to that manifest's directory, landing
/// in the repo-root `wit-vendor/`.
fn wit_file_for_urn(urn: &str, current_file: &str) -> Option<(PathBuf, String)> {
    // Parse `<ns>:<pkg>/<iface>@<ver>` (or without `@<ver>`).
    let head = urn.rsplit_once('@').map(|(h, _)| h).unwrap_or(urn);
    let (ns_pkg, iface) = head.rsplit_once('/')?;
    let (ns, pkg) = ns_pkg.split_once(':')?;
    let target = format!("{}/{}", ns, pkg);

    let project_root = find_project_root_from(&PathBuf::from(current_file))?;
    let manifest_src = std::fs::read_to_string(project_root.join("canon.toml")).ok()?;
    let manifest = manifest::parse(&manifest_src).ok()?;

    for (key, source) in &manifest.imports {
        let ImportSource::Wit(rel) = source else {
            continue;
        };
        // Match the URN's `<ns>/<pkg>` against the manifest key. Either
        // exact (`"wasi/clocks"`) or a broader prefix (`"wasi"`).
        let key_matches = target == *key || target.starts_with(&format!("{}/", key));
        if !key_matches {
            continue;
        }
        let source_path = project_root.join(rel);
        if source_path.is_file() {
            return Some((source_path, iface.to_string()));
        }
        if source_path.is_dir() {
            let candidate = source_path.join(format!("{}.wit", pkg));
            if candidate.is_file() {
                return Some((candidate, iface.to_string()));
            }
        }
    }
    None
}

/// Scan a `.wit` file for a kebab-case identifier appearing in a
/// declaration position. Returns 1-indexed `(line, column)` of the
/// declaration's name token, or `None` if no match.
///
/// We don't reach for a full WIT parser — the LSP only needs to land
/// the cursor near the right symbol, and the WIT syntax is regular
/// enough that a small line-by-line scan does the job. Recognised
/// declaration shapes:
///
///   * `<name>: func(…)` — free-standing function
///   * `type <name> = …` — type alias
///   * `record <name> { … }` — record / product
///   * `variant <name> { … }` — variant / sum
///   * `enum <name> { … }` — enum
///   * `flags <name> { … }` — bitflags
///   * `resource <name>` — resource type
///   * `interface <name>` — interface itself (matched when the user
///     navigates from the bindgen file by clicking on a name that
///     happens to equal the interface's snake-cased filename)
fn find_wit_decl(wit_path: &Path, kebab_name: &str) -> Option<(u32, u32)> {
    let src = std::fs::read_to_string(wit_path).ok()?;
    for (idx, line) in src.lines().enumerate() {
        let line_no = idx as u32 + 1;
        let leading_ws = line.len() - line.trim_start().len();
        let trimmed = line.trim_start();

        // Function: `<name>: func(…)` or `<name>: async func(…)`.
        if let Some(rest) = trimmed.strip_prefix(kebab_name) {
            if let Some(after_colon) = rest.strip_prefix(':') {
                let after = after_colon.trim_start();
                if after.starts_with("func") || after.starts_with("async") {
                    return Some((line_no, (leading_ws + 1) as u32));
                }
            }
        }

        // Keyword-led declarations: `type foo`, `record foo`, etc.
        for kw in [
            "type",
            "record",
            "variant",
            "enum",
            "flags",
            "resource",
            "interface",
        ] {
            let prefix = format!("{} {}", kw, kebab_name);
            if trimmed.starts_with(&prefix) {
                // Check that the kebab name ends cleanly (next char is
                // space, `{`, `=`, `(`, `<`, or end-of-line) so we don't
                // match prefixes like `foo` against `foo-bar`.
                let after = &trimmed[prefix.len()..];
                if after
                    .chars()
                    .next()
                    .map(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
                    .unwrap_or(true)
                {
                    return Some((line_no, (leading_ws + kw.len() + 2) as u32));
                }
            }
        }
    }
    None
}

/// Walk up from `start` looking for the nearest directory that contains
/// an `canon.toml`. Mirrors `loader::find_project_root` (which is
/// private to that module — duplicated here rather than exported to
/// keep the loader API surface narrow).
fn find_project_root_from(start: &Path) -> Option<PathBuf> {
    let mut cur = if start.is_file() {
        start.parent()?
    } else {
        start
    };
    loop {
        if cur.join("canon.toml").is_file() {
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
}

/// Resolve the filesystem path of a `.can` file for a local `use X`
/// declaration. Checks the entry file's sibling directory first, then
/// the module form. Stdlib imports go through [`stdlib_file_stem`]
/// instead — the loader's `name → file_stem` mapping is non-trivial and
/// duplicating it would just guarantee drift.
fn resolve_local_can_file(type_name: &str, current_file: &str) -> Option<String> {
    use std::path::Path;
    let stem = kebab_case(type_name);
    let dir = Path::new(current_file).parent().unwrap_or(Path::new("."));

    // 1. Local file: <dir>/<stem>.can
    let local = dir.join(format!("{}.can", stem));
    if local.exists() {
        return local.to_str().map(|s| s.to_string());
    }
    // 2. Local module dir: <dir>/<stem>/main.can
    let local_mod = dir.join(&stem).join("main.can");
    if local_mod.exists() {
        return local_mod.to_str().map(|s| s.to_string());
    }
    None
}

/// Search imported files for a definition of `word`. Returns
/// (file_uri, span) on success. With imports automatic, the candidate
/// files are found the same way the loader finds them: the local tree
/// by filename convention, then the bundled packages by declared name.
fn find_definition_in_imports(
    _module: &Module,
    word: &str,
    current_file: &str,
) -> Option<(String, crate::error::Span)> {
    use crate::error::Span;
    // Local: `word` → `<kebab>.can` / `<kebab>/main.can` next to the file.
    if let Some(file_path) = resolve_local_can_file(word, current_file) {
        if let Ok(src) = std::fs::read_to_string(&file_path) {
            if let Some(imported) = parse_source(&src) {
                if let Some(span) = find_definition(&imported, word) {
                    return Some((path_to_uri(&file_path), span));
                }
            }
        }
        // The file exists but doesn't declare `word` under that exact
        // name (e.g. a `Self`-renamed constructor) — top of file.
        let top = Span {
            start: 0,
            end: 0,
            line: 1,
            column: 1,
        };
        return Some((path_to_uri(&file_path), top));
    }

    // Bundled: any shipped file declaring `word`, when its build-time
    // source path still exists on disk (running from the source tree).
    for file in crate::loader::bundled_files_declaring(word) {
        if !std::path::Path::new(file.abs_path).exists() {
            continue;
        }
        let Some(imported) = parse_source(file.source) else {
            continue;
        };
        if let Some(span) = find_definition(&imported, word) {
            return Some((path_to_uri(file.abs_path), span));
        }
    }
    None
}

fn collect_definitions(module: &Module) -> Vec<DefInfo> {
    let mut defs = Vec::new();
    for item in &module.items {
        match item {
            Item::TypeDef(td) => {
                defs.push(DefInfo {
                    name: td.name.name.clone(),
                    span: td.name.span,
                });
                // Also collect variant names
                collect_variant_defs(td, &mut defs);
            }
            Item::Function(func) => {
                let name = effective_function_name(func).to_string();
                let span = if func.name.name == "Self" {
                    // For constructors, the name span is the receiver span
                    func.receiver
                        .as_ref()
                        .map(|r| r.span)
                        .unwrap_or(func.name.span)
                } else {
                    func.name.span
                };
                defs.push(DefInfo { name, span });
            }
        }
    }
    defs
}

fn collect_variant_defs(td: &TypeDef, defs: &mut Vec<DefInfo>) {
    if let TypeExpr::Union { variants, .. } = &td.body {
        for v in variants {
            if let TypeExpr::Named { name, span, .. } = v {
                defs.push(DefInfo {
                    name: name.clone(),
                    span: *span,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the slice-5 WIT-navigation helpers. These exercise the
    //! filesystem-level lookups end-to-end against the project's own
    //! `wit-vendor/wasi/` fixtures; the higher-level LSP request/response
    //! plumbing is still tested by hand against editors.
    use super::*;
    use std::fs;

    /// Build a fresh tmpdir under `target/lsp-test-tmp/<name>`. Matches
    /// the pattern used in `tests/install_test.rs`; we duplicate the
    /// helper rather than share to keep this module's deps minimal.
    fn tmpdir(name: &str) -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("target");
        p.push("lsp-test-tmp");
        p.push(name);
        if p.exists() {
            fs::remove_dir_all(&p).expect("clean tmpdir");
        }
        fs::create_dir_all(&p).expect("create tmpdir");
        p
    }

    #[test]
    fn find_wit_decl_locates_function() {
        // `wit-vendor/wasi/clocks.wit` declares `now: func() -> ...`
        // inside the `monotonic-clock` interface. Find it.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wit-vendor/wasi/clocks.wit");
        let (line, col) = find_wit_decl(&path, "now").expect("should find `now`");
        assert!(line > 0);
        assert!(col > 0);

        // Read back that exact line and verify it's the function decl.
        let src = fs::read_to_string(&path).unwrap();
        let actual_line = src.lines().nth((line - 1) as usize).unwrap();
        assert!(
            actual_line.contains("now:") && actual_line.contains("func"),
            "line {line} should be `now: func…`, got `{actual_line}`",
        );
    }

    #[test]
    fn find_wit_decl_locates_kebab_function() {
        // `get-resolution: func()` is the kebab form `getResolution`
        // becomes in the bindgen. Verify the kebab scan finds it.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wit-vendor/wasi/clocks.wit");
        let (line, _) =
            find_wit_decl(&path, "get-resolution").expect("should find `get-resolution`");
        let src = fs::read_to_string(&path).unwrap();
        let actual_line = src.lines().nth((line - 1) as usize).unwrap();
        assert!(
            actual_line.contains("get-resolution:") && actual_line.contains("func"),
            "line {line} should be `get-resolution: func…`, got `{actual_line}`",
        );
    }

    #[test]
    fn find_wit_decl_locates_type_alias() {
        // `type duration = u64` inside `wasi:clocks/types`.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wit-vendor/wasi/clocks.wit");
        let (line, _) = find_wit_decl(&path, "duration").expect("should find `duration`");
        let src = fs::read_to_string(&path).unwrap();
        let actual_line = src.lines().nth((line - 1) as usize).unwrap();
        assert!(
            actual_line.contains("type duration"),
            "line {line} should be `type duration…`, got `{actual_line}`",
        );
    }

    #[test]
    fn find_wit_decl_locates_interface() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wit-vendor/wasi/clocks.wit");
        let (line, _) =
            find_wit_decl(&path, "monotonic-clock").expect("should find `monotonic-clock`");
        let src = fs::read_to_string(&path).unwrap();
        let actual_line = src.lines().nth((line - 1) as usize).unwrap();
        assert!(
            actual_line.contains("interface monotonic-clock"),
            "line {line} should be `interface monotonic-clock…`, got `{actual_line}`",
        );
    }

    #[test]
    fn find_wit_decl_does_not_match_prefixes() {
        // `now` is a real function in clocks.wit; `no` is not, but we
        // need to make sure we don't half-match `now:` against the
        // kebab name `no`.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wit-vendor/wasi/clocks.wit");
        let result = find_wit_decl(&path, "no");
        assert!(
            result.is_none(),
            "`no` is not a declaration in clocks.wit; got {result:?}",
        );
    }

    #[test]
    fn end_to_end_resolves_function_in_bindgen_file_to_wit() {
        // Full slice-5 contract:
        //   1. tmp project with manifest + vendored WIT directory
        //   2. run install — produces `bindgen/wasi/.../monotonic_clock.can` + index
        //   3. resolve_to_wit_declaration(<that file>, "now")
        //      should land us on the `now: func…` line in the WIT
        let root = tmpdir("e2e_resolve");
        let vendor_dir = root.join("vendor");
        fs::create_dir_all(&vendor_dir).unwrap();
        let wit_src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/wit/monotonic-clock.wit");
        fs::copy(&wit_src, vendor_dir.join("monotonic-clock.wit")).unwrap();
        fs::write(
            root.join("canon.toml"),
            r#"
        name = "my-app"
        version = "0.1.0"

        [imports]
        "wasi/clocks" = "vendor/monotonic-clock.wit"
        "#,
        )
        .unwrap();
        crate::install::install(&root).expect("install");

        let bindgen_file = root.join("bindgen/wasi/clocks@0.3.0-rc-2026-03-15/monotonic_clock.can");
        assert!(bindgen_file.is_file());
        let bindgen_str = bindgen_file.to_string_lossy().to_string();

        let (wit_path, line, col) =
            resolve_to_wit_declaration(&bindgen_str, "now").expect("should resolve");
        assert_eq!(wit_path, vendor_dir.join("monotonic-clock.wit"));

        let src = fs::read_to_string(&wit_path).unwrap();
        let actual_line = src.lines().nth((line - 1) as usize).unwrap();
        assert!(
            actual_line.contains("now:") && actual_line.contains("func"),
            "resolved line {line} (col {col}) should be `now: func…`, got `{actual_line}`",
        );
    }

    #[test]
    fn end_to_end_resolves_camel_case_back_to_kebab() {
        // `getResolution` (camelCase as it appears in the bindgen) must
        // be reverse-mapped to `get-resolution` for the WIT scan.
        let root = tmpdir("e2e_camel");
        let vendor_dir = root.join("vendor");
        fs::create_dir_all(&vendor_dir).unwrap();
        let wit_src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/wit/monotonic-clock.wit");
        fs::copy(&wit_src, vendor_dir.join("monotonic-clock.wit")).unwrap();
        fs::write(
            root.join("canon.toml"),
            r#"
        name = "my-app"
        version = "0.1.0"

        [imports]
        "wasi/clocks" = "vendor/monotonic-clock.wit"
        "#,
        )
        .unwrap();
        crate::install::install(&root).expect("install");

        let bindgen_file = root.join("bindgen/wasi/clocks@0.3.0-rc-2026-03-15/monotonic_clock.can");
        let (_, line, _) =
            resolve_to_wit_declaration(&bindgen_file.to_string_lossy(), "getResolution")
                .expect("camel name should resolve via kebab conversion");

        let src = fs::read_to_string(vendor_dir.join("monotonic-clock.wit")).unwrap();
        let actual = src.lines().nth((line - 1) as usize).unwrap();
        assert!(
            actual.contains("get-resolution"),
            "line {line} should be `get-resolution…`, got `{actual}`",
        );
    }

    #[test]
    fn returns_none_for_non_bindgen_file() {
        // A regular `.can` file (not under any `bindgen/` directory) should
        // resolve to nothing — the LSP will then fall back to its
        // ordinary in-file / follow-imports lookup.
        let root = tmpdir("non_bindgen");
        fs::write(
            root.join("canon.toml"),
            "name = \"my-app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let src_dir = root.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let main = src_dir.join("main.can");
        fs::write(&main, "main = () -> Unit { Unit() }\n").unwrap();

        let result = resolve_to_wit_declaration(&main.to_string_lossy(), "main");
        assert!(
            result.is_none(),
            "non-bindgen file should yield no WIT location; got {result:?}",
        );
    }
}
