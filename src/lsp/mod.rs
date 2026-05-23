/// Minimal LSP server for the Oneway programming language.
///
/// Communicates over stdin/stdout using JSON-RPC 2.0 with Content-Length framing.
/// No external dependencies — only std and the `oneway` library crate.
use crate::ast::{resolve_new_syntax, FunctionDef, Item, Module, TypeDef, TypeExpr};
use crate::checker;
use crate::error::OnewayError;
use crate::formatter;
use crate::lexer::Scanner;
use crate::loader::kebab_case;
use crate::parser::Parser;

use std::collections::HashMap;
use std::io::{self, BufRead, Write};

/// Path to the std/ directory, baked in at compile time.
/// Points to the correct location when running from the Oneway source tree.
const STDLIB_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/std");

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run() {
    let mut server = LspServer::new();
    server.run();
}

// ---------------------------------------------------------------------------
// Server state
// ---------------------------------------------------------------------------

struct LspServer {
    /// Open file contents keyed by URI.
    files: HashMap<String, String>,
    initialized: bool,
}

impl LspServer {
    fn new() -> Self {
        Self {
            files: HashMap::new(),
            initialized: false,
        }
    }

    // -----------------------------------------------------------------------
    // Main loop
    // -----------------------------------------------------------------------

    fn run(&mut self) {
        let stdin = io::stdin();
        let mut reader = stdin.lock();

        loop {
            match read_message(&mut reader) {
                Ok(msg) => self.handle_message(&msg),
                Err(e) => {
                    eprintln!("oneway-lsp: read error: {}", e);
                    break;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Dispatch
    // -----------------------------------------------------------------------

    fn handle_message(&mut self, msg: &str) {
        let method = json_get_string(msg, "method");
        let id = json_get_id(msg);

        match method.as_deref() {
            Some("initialize") => self.handle_initialize(id),
            Some("initialized") => {
                self.initialized = true;
            }
            Some("shutdown") => {
                if let Some(id) = id {
                    send_response(&id, "null");
                }
            }
            Some("exit") => std::process::exit(0),
            Some("textDocument/didOpen") => self.handle_did_open(msg),
            Some("textDocument/didChange") => self.handle_did_change(msg),
            Some("textDocument/didSave") => self.handle_did_save(msg),
            Some("textDocument/didClose") => self.handle_did_close(msg),
            // Notifications we receive but don't need to act on.
            Some("workspace/didChangeConfiguration")
            | Some("workspace/didChangeWatchedFiles")
            | Some("$/cancelRequest") => {}
            Some("textDocument/hover") => self.handle_hover(msg, id),
            Some("textDocument/definition") => self.handle_definition(msg, id),
            Some("textDocument/formatting") => self.handle_formatting(msg, id),
            Some(m) => {
                eprintln!("oneway-lsp: unhandled method: {}", m);
                // If it has an id, respond with method-not-found
                if let Some(id) = id {
                    send_error(&id, -32601, "method not found");
                }
            }
            None => {
                eprintln!("oneway-lsp: message has no method field");
            }
        }
    }

    // -----------------------------------------------------------------------
    // initialize
    // -----------------------------------------------------------------------

    fn handle_initialize(&mut self, id: Option<String>) {
        let result = r#"{
            "capabilities": {
                "textDocumentSync": {
                    "openClose": true,
                    "change": 1,
                    "save": { "includeText": true }
                },
                "hoverProvider": true,
                "definitionProvider": true,
                "documentFormattingProvider": true
            },
            "serverInfo": {
                "name": "oneway-lsp",
                "version": "0.1.0"
            }
        }"#;
        if let Some(id) = id {
            send_response(&id, result);
        }
    }

    // -----------------------------------------------------------------------
    // textDocument/didOpen
    // -----------------------------------------------------------------------

    fn handle_did_open(&mut self, msg: &str) {
        if let Some(uri) = json_get_nested_string(msg, "textDocument", "uri") {
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

    fn handle_did_change(&mut self, msg: &str) {
        if let Some(uri) = json_get_nested_string(msg, "textDocument", "uri") {
            // Full document text is in contentChanges[0].text
            if let Some(text) = extract_content_change_text(msg) {
                let text = json_unescape(&text);
                self.files.insert(uri.clone(), text);
                self.publish_diagnostics(&uri);
            }
        }
    }

    // -----------------------------------------------------------------------
    // textDocument/didSave
    // -----------------------------------------------------------------------

    fn handle_did_save(&mut self, msg: &str) {
        if let Some(uri) = json_get_nested_string(msg, "textDocument", "uri") {
            // If the save notification includes text, use it
            if let Some(text) = extract_param_text(msg) {
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

    fn handle_did_close(&mut self, msg: &str) {
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
    // Diagnostics
    // -----------------------------------------------------------------------

    fn publish_diagnostics(&self, uri: &str) {
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
            // LSP uses 0-based lines/columns; Oneway uses 1-based.
            let line = if span.line > 0 { span.line - 1 } else { 0 };
            let col = if span.column > 0 { span.column - 1 } else { 0 };
            let end_col = col + (span.end.saturating_sub(span.start) as u32).max(1);
            diags.push_str(&format!(
                r#"{{"range":{{"start":{{"line":{},"character":{}}},"end":{{"line":{},"character":{}}}}},"severity":1,"source":"oneway","message":"{}"}}"#,
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

    // -----------------------------------------------------------------------
    // Hover
    // -----------------------------------------------------------------------

    fn handle_hover(&self, msg: &str, id: Option<String>) {
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

    fn handle_definition(&self, msg: &str, id: Option<String>) {
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
        // Phase 1: definition in the current file.
        let location = find_definition(&module, &word).map(|span| (uri.clone(), span));

        // Phase 2: follow `use` imports if not found locally.
        let location = location.or_else(|| {
            let current_path = uri_to_path(&uri);
            find_definition_in_imports(&module, &word, &current_path)
        });

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

    fn handle_formatting(&self, msg: &str, id: Option<String>) {
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

// ===========================================================================
// Compiler integration
// ===========================================================================

/// Recursively collect non-`use` items from a module and all its transitive
/// imports into `out`. `seen` tracks already-visited module names to break
/// cycles. `current_file` is the requesting file, used to resolve relative
/// local imports.
fn collect_import_items(
    type_name: &str,
    current_file: &str,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<crate::ast::Item>,
) {
    if !seen.insert(type_name.to_string()) {
        return; // already loaded
    }
    let Some(ow_path) = resolve_ow_file(type_name, current_file) else {
        return;
    };
    let Ok(src) = std::fs::read_to_string(&ow_path) else {
        return;
    };
    let Some(imported) = parse_source(&src) else {
        return;
    };
    // Recurse into this module's own imports first (using ow_path as base).
    for item in &imported.items {
        if let crate::ast::Item::Use(u) = item {
            let inner = u.name.name.split('/').last().unwrap_or(&u.name.name);
            collect_import_items(inner, &ow_path, seen, out);
        }
    }
    // Then add this module's own non-use definitions.
    for item in imported.items {
        if !matches!(item, crate::ast::Item::Use(_)) {
            out.push(item);
        }
    }
}

/// Check source text and return all errors.
///
/// `file_path` is the filesystem path of the file being checked (not a URI).
/// If provided, all `use` imports are resolved relative to it and their
/// definitions are loaded into the module before checking — giving the
/// checker full knowledge of imported types and methods.
fn check_source(source: &str, file_path: &str) -> Vec<OnewayError> {
    // 1. Parse the in-memory source.
    let mut scanner = Scanner::new(source);
    let tokens = match scanner.scan_tokens() {
        Ok(t) => t,
        Err(e) => return vec![e],
    };
    let mut parser = Parser::new(tokens);
    let mut current = match parser.parse() {
        Ok(m) => m,
        Err(e) => return vec![e],
    };
    resolve_new_syntax(&mut current);

    // 2. Collect items from every `use` import, recursively following
    //    transitive imports (e.g. `use HttpClient` brings in `use Url` which
    //    defines `Url` and `InvalidUrl`). A `seen` set prevents cycles.
    let mut imported_items: Vec<crate::ast::Item> = Vec::new();
    let mut seen = std::collections::HashSet::<String>::new();
    for item in &current.items {
        let crate::ast::Item::Use(u) = item else {
            continue;
        };
        let type_name = u.name.name.split('/').last().unwrap_or(&u.name.name);
        collect_import_items(type_name, file_path, &mut seen, &mut imported_items);
    }

    // 3. Build a combined module: imported items first, then current file's
    //    non-use items. `entry_items_start` tells the checker which items
    //    belong to the user's file (the only ones subject to ordering rules).
    let entry_items_start = imported_items.len();
    for item in current.items {
        // Strip use declarations here too — they were already resolved above
        // and keeping them causes false "use must appear before definitions"
        // and alphabetical ordering errors.
        if !matches!(item, crate::ast::Item::Use(_)) {
            imported_items.push(item);
        }
    }
    let combined = crate::ast::Module {
        items: imported_items,
        span: current.span,
    };

    let mut errors = checker::check_with_entry(&combined, entry_items_start);
    errors.retain(|e| e.message() != "no `main` entry point defined");
    errors
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
            _ => {}
        }
    }
    // Fall back to built-in type descriptions.
    builtin_hover(name)
}

/// Hover info for built-in types, capabilities, and well-known constructors.
fn builtin_hover(name: &str) -> Option<String> {
    let desc = match name {
        // Core numeric / text types
        "Int"    => "```oneway\nInt\n```\nA 64-bit signed integer.",
        "Float"  => "```oneway\nFloat\n```\nA 64-bit floating-point number.",
        "Hex"    => "```oneway\nHex\n```\nA 64-bit unsigned integer displayed in hexadecimal.",
        "String" => "```oneway\nString = Byte^*\n```\nA UTF-8 string.",
        "Byte"   => "```oneway\nByte\n```\nA single byte (u8).",
        "Bytes"  => "```oneway\nBytes = Byte^*\n```\nA byte sequence.",
        // Unit / Never
        "Unit"  => "```oneway\nUnit\n```\nThe singleton type with exactly one value: `Unit`.",
        "Never" => "```oneway\nNever\n```\nThe uninhabited type — a function returning `Never` does not return.",
        // Bool and its variants
        "Bool"  => "```oneway\nBool = False + True\n```\nThe built-in boolean type.",
        "False" => "```oneway\nFalse\n```\nVariant of `Bool`. The falsy value.",
        "True"  => "```oneway\nTrue\n```\nVariant of `Bool`. The truthy value.",
        // Ord and its variants
        "Ord"     => "```oneway\nOrd = Equal + Greater + Less\n```\nComparison result.",
        "Equal"   => "```oneway\nEqual\n```\nVariant of `Ord`.",
        "Greater" => "```oneway\nGreater\n```\nVariant of `Ord`.",
        "Less"    => "```oneway\nLess\n```\nVariant of `Ord`.",
        // Generic containers
        "List"   => "```oneway\nList<T>\n```\nAn ordered sequence of values of type `T`.",
        "Map"    => "```oneway\nMap<K, V>\n```\nA sorted key-value map. `K` must implement `Ord`.",
        "Set"    => "```oneway\nSet<T>\n```\nA sorted set of values. `T` must implement `Ord`.",
        "Option" => "```oneway\nOption<T> = None + Some<T>\n```\nAn optional value.",
        "Some"   => "```oneway\nSome<T>\n```\nVariant of `Option<T>` — value is present.",
        "None"   => "```oneway\nNone\n```\nVariant of `Option<T>` — value is absent.",
        "Result" => "```oneway\nResult<T, E> = Err<E> + Ok<T>\n```\nA fallible computation.",
        "Ok"     => "```oneway\nOk<T>\n```\nVariant of `Result<T, E>` — success.",
        "Err"    => "```oneway\nErr<E>\n```\nVariant of `Result<T, E>` — failure.",
        // Capabilities
        "Clock"      => "```oneway\nClock\n```\nCapability for reading the current time (non-suspending).",
        "Filesystem" => "```oneway\nFilesystem\n```\nCapability for filesystem I/O (suspending — makes the function async).",
        "Network"    => "```oneway\nNetwork\n```\nCapability for network I/O (suspending — makes the function async).",
        "Random"     => "```oneway\nRandom\n```\nCapability for random number generation (non-suspending).",
        "Stderr"     => "```oneway\nStderr\n```\nCapability for writing to stderr (non-suspending).",
        "Stdin"      => "```oneway\nStdin\n```\nCapability for reading from stdin (non-suspending).",
        "Stdout"     => "```oneway\nStdout\n```\nCapability for writing to stdout (non-suspending).",
        // Misc flags
        "Off" => "```oneway\nOff\n```\nSingleton flag type — the \"off\" state.",
        "On"  => "```oneway\nOn\n```\nSingleton flag type — the \"on\" state.",
        "Bit" => "```oneway\nBit = Off + On\n```\nA single bit.",
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
                    let info = format!("```oneway\n{}\n```\nVariant of `{}`", name, td.name.name);
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
    format!("```oneway\n{}{} = {}\n```", td.name.name, generics, body)
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
    format!("```oneway\n{}\n```", sig)
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

/// Resolve the filesystem path of a `.ow` file for a `use X` declaration.
/// Checks (in order): local file, local module dir, stdlib.
fn resolve_ow_file(type_name: &str, current_file: &str) -> Option<String> {
    use std::path::Path;
    let stem = kebab_case(type_name);
    let dir = Path::new(current_file).parent().unwrap_or(Path::new("."));

    // 1. Local file: <dir>/<stem>.ow
    let local = dir.join(format!("{}.ow", stem));
    if local.exists() {
        return local.to_str().map(|s| s.to_string());
    }
    // 2. Local module dir: <dir>/<stem>/main.ow
    let local_mod = dir.join(&stem).join("main.ow");
    if local_mod.exists() {
        return local_mod.to_str().map(|s| s.to_string());
    }
    // 3. Stdlib
    let stdlib = Path::new(STDLIB_DIR).join(format!("{}.ow", stem));
    if stdlib.exists() {
        return stdlib.to_str().map(|s| s.to_string());
    }
    None
}

/// Search imported modules for a definition. Returns (file_uri, span) on success.
/// If the word IS the imported type name (e.g. clicking `Json` in `use Json`),
/// navigates to the top of the imported file.
fn find_definition_in_imports(
    module: &Module,
    word: &str,
    current_file: &str,
) -> Option<(String, crate::error::Span)> {
    use crate::error::Span;
    for item in &module.items {
        let Item::Use(u) = item else { continue };
        let path_str = &u.name.name;
        let type_name = path_str.split('/').last().unwrap_or(path_str);
        let Some(file_path) = resolve_ow_file(type_name, current_file) else {
            continue;
        };

        // Clicking the import name itself → top of the imported file.
        if type_name == word {
            let file_uri = path_to_uri(&file_path);
            let top = Span {
                start: 0,
                end: 0,
                line: 1,
                column: 1,
            };
            return Some((file_uri, top));
        }

        // Otherwise search inside the imported file.
        let Ok(src) = std::fs::read_to_string(&file_path) else {
            continue;
        };
        let Some(imported) = parse_source(&src) else {
            continue;
        };
        if let Some(span) = find_definition(&imported, word) {
            return Some((path_to_uri(&file_path), span));
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
            // `use` items are resolved via find_definition_in_imports, not here.
            Item::Use(_) => {}
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

// ===========================================================================
// Source text helpers
// ===========================================================================

/// Given a source string and a 0-based (line, character) position, return the
/// word (identifier) at that position.
fn word_at_position(source: &str, line: u32, character: u32) -> Option<String> {
    let target_line = source.lines().nth(line as usize)?;
    let col = character as usize;

    if col >= target_line.len() {
        // Try the character just before if we're at the end
        if col == 0 {
            return None;
        }
        // Fall through — we'll check the byte at col below
    }

    // Find the word boundaries around `col`
    let bytes = target_line.as_bytes();
    if col >= bytes.len() {
        return None;
    }
    if !is_ident_char(bytes[col]) {
        return None;
    }

    let mut start = col;
    while start > 0 && is_ident_char(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end < bytes.len() && is_ident_char(bytes[end]) {
        end += 1;
    }

    if start == end {
        return None;
    }

    Some(target_line[start..end].to_string())
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

// ===========================================================================
// URI helpers
// ===========================================================================

fn uri_to_path(uri: &str) -> String {
    if let Some(rest) = uri.strip_prefix("file://") {
        // Percent-decode common sequences
        percent_decode(rest)
    } else {
        uri.to_string()
    }
}

fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(val) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                result.push(val as char);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

// ===========================================================================
// LSP transport — reading
// ===========================================================================

fn read_message(reader: &mut impl BufRead) -> io::Result<String> {
    // Read headers until we find Content-Length
    let mut content_length: Option<usize> = None;

    loop {
        let mut header = String::new();
        let n = reader.read_line(&mut header)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF on stdin"));
        }
        let header = header.trim();
        if header.is_empty() {
            // Empty line = end of headers
            break;
        }
        if let Some(rest) = header.strip_prefix("Content-Length:") {
            if let Ok(len) = rest.trim().parse::<usize>() {
                content_length = Some(len);
            }
        }
        // Skip other headers (e.g. Content-Type)
    }

    let len = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length header")
    })?;

    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;

    String::from_utf8(body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// ===========================================================================
// LSP transport — writing
// ===========================================================================

fn send_message(json: &str) {
    let out = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(out.as_bytes());
    let _ = lock.flush();
}

fn send_response(id: &str, result: &str) {
    let msg = format!(r#"{{"jsonrpc":"2.0","id":{},"result":{}}}"#, id, result);
    send_message(&msg);
}

fn send_error(id: &str, code: i32, message: &str) {
    let msg = format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":{},"message":"{}"}}}}"#,
        id,
        code,
        json_escape(message)
    );
    send_message(&msg);
}

// ===========================================================================
// Minimal JSON helpers
// ===========================================================================

/// Escape a string for inclusion in a JSON string value.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Unescape a JSON string value (reverse of json_escape).
fn json_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Ok(code) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(code) {
                            out.push(ch);
                        }
                    }
                }
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Extract a top-level string field from a JSON object.
/// Searches for `"key": "value"` or `"key":"value"`.
fn json_get_string(json: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\"", key);
    let idx = json.find(&pattern)?;
    let after_key = idx + pattern.len();
    let rest = &json[after_key..];

    // Skip optional whitespace and colon
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':')?;
    let rest = rest.trim_start();

    if rest.starts_with('"') {
        extract_json_string(rest)
    } else {
        None
    }
}

/// Extract a string field nested one level: search for the outer key,
/// then within the following object search for the inner key.
fn json_get_nested_string(json: &str, outer: &str, inner: &str) -> Option<String> {
    let pattern = format!("\"{}\"", outer);
    let idx = json.find(&pattern)?;
    let after_key = idx + pattern.len();
    let rest = &json[after_key..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':')?;
    let rest = rest.trim_start();

    // Find the opening brace of the nested object
    if !rest.starts_with('{') {
        return None;
    }

    // Find the matching closing brace
    let obj_str = extract_json_object(rest)?;

    // Now search for inner key within this object
    json_get_string(&obj_str, inner)
}

/// Extract the `id` field from a JSON-RPC message.
/// The id can be a number or a string.
fn json_get_id(json: &str) -> Option<String> {
    let pattern = "\"id\"";
    let idx = json.find(pattern)?;
    let after_key = idx + pattern.len();
    let rest = &json[after_key..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':')?;
    let rest = rest.trim_start();

    if rest.starts_with('"') {
        // String id — return it with quotes
        let s = extract_json_string(rest)?;
        Some(format!("\"{}\"", json_escape(&s)))
    } else {
        // Numeric id — read until non-digit
        let end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '-')
            .unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        Some(rest[..end].to_string())
    }
}

/// Extract position (line, character) from `params.position`.
fn json_get_position(json: &str) -> Option<(u32, u32)> {
    // Find "position" object
    let pattern = "\"position\"";
    let idx = json.find(pattern)?;
    let after_key = idx + pattern.len();
    let rest = &json[after_key..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':')?;
    let rest = rest.trim_start();

    let obj = extract_json_object(rest)?;

    let line = json_get_number(&obj, "line")?;
    let character = json_get_number(&obj, "character")?;
    Some((line, character))
}

/// Extract a numeric field from a JSON object.
fn json_get_number(json: &str, key: &str) -> Option<u32> {
    let pattern = format!("\"{}\"", key);
    let idx = json.find(&pattern)?;
    let after_key = idx + pattern.len();
    let rest = &json[after_key..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':')?;
    let rest = rest.trim_start();

    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    rest[..end].parse().ok()
}

/// Extract a JSON string value starting at the opening quote.
/// Handles escape sequences.
fn extract_json_string(s: &str) -> Option<String> {
    if !s.starts_with('"') {
        return None;
    }
    let mut result = String::new();
    let mut chars = s[1..].chars();
    loop {
        match chars.next() {
            None => return None, // unterminated string
            Some('"') => return Some(result),
            Some('\\') => match chars.next() {
                Some('"') => result.push('"'),
                Some('\\') => result.push('\\'),
                Some('/') => result.push('/'),
                Some('n') => result.push('\n'),
                Some('r') => result.push('\r'),
                Some('t') => result.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Ok(code) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(code) {
                            result.push(ch);
                        }
                    }
                }
                Some(c) => {
                    result.push('\\');
                    result.push(c);
                }
                None => return None,
            },
            Some(c) => result.push(c),
        }
    }
}

/// Extract a balanced JSON object starting at `{`, returns the full substring
/// including the braces.
fn extract_json_object(s: &str) -> Option<String> {
    if !s.starts_with('{') {
        return None;
    }
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, ch) in s.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if in_string {
            if ch == '\\' {
                escape_next = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Extract the text from contentChanges[0].text in a didChange notification.
fn extract_content_change_text(json: &str) -> Option<String> {
    // Find "contentChanges" then find the first "text" inside it
    let pattern = "\"contentChanges\"";
    let idx = json.find(pattern)?;
    let rest = &json[idx + pattern.len()..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':')?;
    let rest = rest.trim_start();

    // Should start with [
    if !rest.starts_with('[') {
        return None;
    }

    // Find the first "text" field inside the array
    json_get_string(rest, "text")
}

/// Extract params.text for didSave notifications that include text.
fn extract_param_text(json: &str) -> Option<String> {
    // Look for "text" in the params — but be careful not to match
    // textDocument. We find it by looking after "params".
    let params_idx = json.find("\"params\"")?;
    let params_rest = &json[params_idx..];
    // Skip the textDocument sub-object and look for a direct "text" field
    // We'll look for "text" that isn't inside "textDocument"
    let td_pattern = "\"textDocument\"";
    let text_pattern = "\"text\"";

    // Find the last occurrence of "text" in params (which won't be inside textDocument)
    let after_td = if let Some(td_idx) = params_rest.find(td_pattern) {
        let td_rest = &params_rest[td_idx + td_pattern.len()..];
        // Skip the textDocument object
        let td_rest = td_rest.trim_start();
        if let Some(td_rest) = td_rest.strip_prefix(':') {
            let td_rest = td_rest.trim_start();
            if let Some(obj) = extract_json_object(td_rest) {
                let skip = td_idx
                    + td_pattern.len()
                    + (td_rest.as_ptr() as usize
                        - params_rest[td_idx + td_pattern.len()..].as_ptr() as usize)
                    + obj.len();
                &params_rest[skip..]
            } else {
                params_rest
            }
        } else {
            params_rest
        }
    } else {
        params_rest
    };

    // Now look for "text" in whatever remains after textDocument
    if let Some(text_idx) = after_td.find(text_pattern) {
        let rest = &after_td[text_idx + text_pattern.len()..];
        let rest = rest.trim_start();
        let rest = rest.strip_prefix(':')?;
        let rest = rest.trim_start();
        if rest.starts_with('"') {
            return extract_json_string(rest);
        }
    }

    None
}
