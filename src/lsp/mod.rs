/// Minimal LSP server for the Canon programming language.
///
/// Communicates over stdin/stdout using JSON-RPC 2.0 with Content-Length framing.
/// No external dependencies — only std and the `canon` library crate.
use crate::ast::{resolve_new_syntax, FunctionDef, Item, Module, TypeDef, TypeExpr};
use crate::bindgen::camel_to_kebab;
use crate::checker;
use crate::error::CanonError;
use crate::formatter;
use crate::lexer::Scanner;
use crate::loader::{kebab_case, resolve_bundled_use, BundledFile, BUNDLED_PACKAGES};
use crate::manifest::{self, ImportSource};
use crate::parser::Parser;

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

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
                    eprintln!("canon-lsp: read error: {}", e);
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
                eprintln!("canon-lsp: unhandled method: {}", m);
                // If it has an id, respond with method-not-found
                if let Some(id) = id {
                    send_error(&id, -32601, "method not found");
                }
            }
            None => {
                eprintln!("canon-lsp: message has no method field");
            }
        }
    }

    // -----------------------------------------------------------------------
    // initialize
    // -----------------------------------------------------------------------

    fn handle_initialize(&mut self, id: Option<String>) {
        let result = format!(
            r#"{{
            "capabilities": {{
                "textDocumentSync": {{
                    "openClose": true,
                    "change": 1,
                    "save": {{ "includeText": true }}
                }},
                "hoverProvider": true,
                "definitionProvider": true,
                "documentFormattingProvider": true
            }},
            "serverInfo": {{
                "name": "canon-lsp",
                "version": "{}"
            }}
        }}"#,
            env!("CARGO_PKG_VERSION")
        );
        if let Some(id) = id {
            send_response(&id, &result);
        }
    }

    // -----------------------------------------------------------------------
    // textDocument/didOpen
    // -----------------------------------------------------------------------

    fn handle_did_open(&mut self, msg: &str) {
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

    fn handle_did_change(&mut self, msg: &str) {
        if let Some(uri) = json_get_nested_string(msg, "textDocument", "uri") {
            if !uri.ends_with(".can") {
                return;
            }
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
            if !uri.ends_with(".can") {
                return;
            }
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
        let current_path = uri_to_path(&uri);

        // Phase 0b: if the current file is a bindgen-generated file,
        // jump straight to the matching WIT declaration. The bindgen
        // `.can` file is a derived artifact — the WIT is the authoritative
        // source of truth, so go-to-def lands the user on the spec
        // rather than on the regenerable shim. Falls through to the
        // ordinary lookup if the WIT can't be resolved (no install index,
        // missing manifest, WIT file not on disk, decl not found).
        let wit_location = {
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
            })
        };

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
///
/// `bundled` is `Some` when the `use` path resolves into a shipped package
/// (`canon/std`, `canon/wasi`, …) and `None` for local-file imports. The
/// compiler's own loader makes the same split — we must too, otherwise the
/// LSP and the compiler disagree about what `Foo` is.
fn collect_import_items(
    use_path: &str,
    bundled: Option<&'static BundledFile>,
    current_file: &str,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<crate::ast::Item>,
) {
    if !seen.insert(use_path.to_string()) {
        return; // already loaded
    }

    // Bundled-package resolution: pull the embedded source out of the
    // loader. This is the same registry the compiler uses, so the LSP and
    // the compiler never disagree on what `use canon/std/Foo` means.
    if let Some(file) = bundled {
        let Some(imported) = parse_source(file.source) else {
            return;
        };
        for item in &imported.items {
            if let crate::ast::Item::Use(u) = item {
                let (inner_path, inner_bundled) = parse_use_path(&u.name.name);
                // Transitive imports inherit the current file as the
                // resolution base — the embedded packages have no
                // filesystem location, and local imports from within
                // a bundled package resolve against the package itself.
                collect_import_items(inner_path, inner_bundled, current_file, seen, out);
            }
        }
        for item in imported.items {
            if !matches!(item, crate::ast::Item::Use(_)) {
                out.push(item);
            }
        }
        return;
    }

    // Local resolution: file relative to `current_file`. Only the last
    // segment (the type name) of the use path is meaningful for the
    // local-file form — multi-segment local paths aren't supported by
    // this helper yet (the compiler's loader handles them via
    // `process_use`, but LSP-side definition-following is single-segment
    // only for now).
    let type_name = use_path.rsplit('/').next().unwrap_or(use_path);
    let Some(ow_path) = resolve_local_ow_file(type_name, current_file) else {
        return;
    };
    let Ok(src) = std::fs::read_to_string(&ow_path) else {
        return;
    };
    let Some(imported) = parse_source(&src) else {
        return;
    };
    for item in &imported.items {
        if let crate::ast::Item::Use(u) = item {
            let (inner_path, inner_bundled) = parse_use_path(&u.name.name);
            collect_import_items(inner_path, inner_bundled, &ow_path, seen, out);
        }
    }
    for item in imported.items {
        if !matches!(item, crate::ast::Item::Use(_)) {
            out.push(item);
        }
    }
}

/// Classify a `use` path: if it matches a bundled package, return the
/// bundled file; otherwise return `None`. The returned `&str` is the
/// original full path — callers use it as the seen-set key and (for the
/// local case) extract the last segment for filesystem lookup.
fn parse_use_path(path: &str) -> (&str, Option<&'static BundledFile>) {
    let bundled = resolve_bundled_use(path).map(|(_, file)| file);
    (path, bundled)
}

/// Check source text and return all errors.
///
/// `file_path` is the filesystem path of the file being checked (not a URI).
/// If provided, all `use` imports are resolved relative to it and their
/// definitions are loaded into the module before checking — giving the
/// checker full knowledge of imported types and methods.
fn check_source(source: &str, file_path: &str) -> Vec<CanonError> {
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
    //    transitive imports (e.g. `use canon/std/HttpServer` brings in
    //    `use canon/std/Request` etc.). A `seen` set prevents cycles. The
    //    full use path is preserved through `parse_use_path` so the
    //    embedded packages and on-disk files don't collide.
    let mut imported_items: Vec<crate::ast::Item> = Vec::new();
    let mut seen = std::collections::HashSet::<String>::new();
    for item in &current.items {
        let crate::ast::Item::Use(u) = item else {
            continue;
        };
        let (use_path, bundled) = parse_use_path(&u.name.name);
        collect_import_items(use_path, bundled, file_path, &mut seen, &mut imported_items);
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

/// Determine the WIT interface URN that backs the given source file,
/// if it's a binding file. The URN is derived from the file's vendored
/// path (`<ns>/<name>@<version>/<iface>.can` — see PACKAGES.md), for
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
fn resolve_local_ow_file(type_name: &str, current_file: &str) -> Option<String> {
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

/// Compose the on-disk path of a bundled file. Used so go-to-definition
/// on a `use canon/std/Foo` import can navigate to the actual file in
/// the source tree. Returns `None` if the use path doesn't match any
/// bundled package, or if the build-time absolute path no longer exists
/// (e.g. when running from an installed binary).
fn resolve_bundled_ow_file(use_path: &str) -> Option<String> {
    use std::path::Path;
    let (_, file) = resolve_bundled_use(use_path)?;
    let path = Path::new(file.abs_path);
    if path.exists() {
        Some(file.abs_path.to_string())
    } else {
        None
    }
}

/// Search imported modules for a definition. Returns (file_uri, span) on success.
/// If the word IS the imported type name (e.g. clicking `Json` in `use std/Json`),
/// navigates to the top of the imported file.
fn find_definition_in_imports(
    module: &Module,
    word: &str,
    current_file: &str,
) -> Option<(String, crate::error::Span)> {
    use crate::error::Span;
    for item in &module.items {
        let Item::Use(u) = item else { continue };
        let (use_path, bundled) = parse_use_path(&u.name.name);
        // The clickable target name is always the last segment of the use
        // path — the type or file name the user is importing.
        let type_name = use_path.rsplit('/').next().unwrap_or(use_path);
        let file_path = if bundled.is_some() {
            resolve_bundled_ow_file(use_path)?
        } else {
            match resolve_local_ow_file(type_name, current_file) {
                Some(p) => p,
                None => continue,
            }
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
            // `use` items are resolved via find_definition_in_imports;
            // `bindings` / `package` items don't introduce new symbols
            // (the loader synthesizes FunctionDefs for the function-type
            // aliases beneath a `bindings` directive; `package` is pure
            // provenance).
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
