/// Minimal LSP server for the Canon programming language.
///
/// Communicates over stdin/stdout using JSON-RPC 2.0 with Content-Length framing.
/// No external dependencies — only std and the `canon` library crate.
///
/// The server is split across three files:
///   * `mod.rs`      — entry point, the read loop, method dispatch, the
///                     `initialize` capability declaration, and the LSP
///                     transport / JSON / URI / source-text helpers shared
///                     by the handlers.
///   * `state.rs`    — the `LspServer` document state and diagnostics.
///   * `handlers.rs` — the `textDocument/*` handlers plus the hover and
///                     go-to-definition machinery.
mod handlers;
mod state;

use state::LspServer;

use std::io::{self, BufRead, Write};

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run() {
    let mut server = LspServer::new();
    server.run();
}

impl LspServer {
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
}

// ===========================================================================
// Source text helpers
// ===========================================================================

/// Given a source string and a 0-based (line, character) position, return the
/// word (identifier) at that position.
pub(super) fn word_at_position(source: &str, line: u32, character: u32) -> Option<String> {
    let target_line = source.lines().nth(line as usize)?;
    let mut col = character as usize;

    if col >= target_line.len() {
        // Cursor is at or past the end of the line — try the character
        // just before it instead.
        if col == 0 {
            return None;
        }
        col -= 1;
    }

    // Find the word boundaries around `col`
    let bytes = target_line.as_bytes();
    if col >= bytes.len() || !is_ident_char(bytes[col]) {
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

pub(super) fn uri_to_path(uri: &str) -> String {
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

pub(super) fn send_message(json: &str) {
    let out = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(out.as_bytes());
    let _ = lock.flush();
}

pub(super) fn send_response(id: &str, result: &str) {
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
pub(super) fn json_escape(s: &str) -> String {
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
pub(super) fn json_unescape(s: &str) -> String {
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
pub(super) fn json_get_nested_string(json: &str, outer: &str, inner: &str) -> Option<String> {
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
pub(super) fn json_get_position(json: &str) -> Option<(u32, u32)> {
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
pub(super) fn extract_content_change_text(json: &str) -> Option<String> {
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
pub(super) fn extract_param_text(json: &str) -> Option<String> {
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
    use super::*;

    #[test]
    fn word_at_position_finds_word_in_middle() {
        assert_eq!(
            word_at_position("Foo -> Bar", 0, 1),
            Some("Foo".to_string())
        );
    }

    #[test]
    fn word_at_position_finds_word_at_line_end() {
        // The cursor sits right after the last character of the word —
        // the position an editor reports right after the user finishes
        // typing an identifier, or after clicking at the end of a line.
        // `character` here is one past the last valid index, matching
        // the doc comment's stated intent ("try the character just
        // before if we're at the end").
        assert_eq!(word_at_position("Foo", 0, 3), Some("Foo".to_string()));
    }

    #[test]
    fn word_at_position_returns_none_on_whitespace() {
        assert_eq!(word_at_position("Foo Bar", 0, 3), None);
    }

    #[test]
    fn word_at_position_returns_none_past_end_of_empty_line() {
        assert_eq!(word_at_position("", 0, 5), None);
    }

    #[test]
    fn word_at_position_returns_none_for_missing_line() {
        assert_eq!(word_at_position("Foo", 5, 0), None);
    }
}
