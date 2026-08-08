//! The compiler's browser entry point — `canon` compiled to
//! `wasm32-unknown-unknown` and driven from JavaScript.
//!
//! The docs site's playground and its click-to-run snippets compile Canon
//! in the page: there is no build step and no server. Everything the
//! compiler needs is already in the binary (the standard library is baked
//! in by `build.rs`), so the module has no imports at all — the host hands
//! it source bytes and gets a component back.
//!
//! The ABI is four functions and a tag byte:
//!
//! ```text
//! canon_alloc(len)        -> ptr    // write the source here
//! canon_format(ptr, len)  -> len    // canonical source
//! canon_compile(ptr, len) -> len    // byte length of the result
//! canon_result()          -> ptr    // result[0] tags the rest:
//!                                   //   0 = diagnostics, UTF-8 text
//!                                   //   1 = the payload (see each fn)
//! ```
//!
//! `canon_format` exists because canonical format is a compiler phase,
//! not a lint: unformatted source is a *checker error*, so a playground
//! that only compiled would spend its life reporting spacing. The page
//! formats what you typed and then compiles that, which is also the
//! honest demonstration — one shape per program, applied as you watch.
//!
//! The host instantiates a fresh instance per compile, so nothing here
//! has to reset. A panic aborts (that is `wasm32-unknown-unknown`'s
//! strategy), which traps into the host with the instance's memory still
//! readable — the hook leaves the message in the result buffer so the
//! trap surfaces as a diagnostic rather than `unreachable`.

use crate::{checker, codegen, loader};
use std::cell::RefCell;
use std::path::Path;

/// Name the playground reports errors against. A playground program is
/// one file with no directory around it.
const ENTRY: &str = "playground.can";

const TAG_DIAGNOSTICS: u8 = 0;
const TAG_PAYLOAD: u8 = 1;

thread_local! {
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn set_result(tag: u8, body: &[u8]) -> usize {
    RESULT.with(|cell| {
        let mut out = cell.borrow_mut();
        out.clear();
        out.push(tag);
        out.extend_from_slice(body);
        out.len()
    })
}

#[no_mangle]
pub extern "C" fn canon_alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

#[no_mangle]
pub extern "C" fn canon_format(ptr: *const u8, len: usize) -> usize {
    let source = match source_of(ptr, len) {
        Ok(source) => source,
        Err(len) => return len,
    };
    match crate::formatter::format(source) {
        Ok(canonical) => set_result(TAG_PAYLOAD, canonical.as_bytes()),
        Err(err) => set_result(TAG_DIAGNOSTICS, render(&[err]).as_bytes()),
    }
}

#[no_mangle]
pub extern "C" fn canon_compile(ptr: *const u8, len: usize) -> usize {
    let source = match source_of(ptr, len) {
        Ok(source) => source,
        Err(len) => return len,
    };
    let loaded = match loader::load_text(Path::new(ENTRY), source) {
        Ok(loaded) => loaded,
        Err(err) => return set_result(TAG_DIAGNOSTICS, render(&[err]).as_bytes()),
    };
    let errors = checker::check_loaded(&loaded);
    if !errors.is_empty() {
        return set_result(TAG_DIAGNOSTICS, render(&errors).as_bytes());
    }
    set_result(TAG_PAYLOAD, &codegen::generate(&loaded.module))
}

/// Borrows the host-written source, installing the panic hook on the way
/// in. On failure the result buffer already holds the diagnostic and the
/// error carries its length, so callers just return it.
fn source_of<'a>(ptr: *const u8, len: usize) -> std::result::Result<&'a str, usize> {
    std::panic::set_hook(Box::new(|info| {
        set_result(
            TAG_DIAGNOSTICS,
            format!("compiler panic: {info}").as_bytes(),
        );
    }));
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    std::str::from_utf8(bytes)
        .map_err(|_| set_result(TAG_DIAGNOSTICS, b"source is not valid UTF-8"))
}

#[no_mangle]
pub extern "C" fn canon_result() -> *const u8 {
    RESULT.with(|cell| cell.borrow().as_ptr())
}

/// One diagnostic per line, in the CLI's `path:line:column: message`
/// shape (`print_error` in `src/main.rs`) so the same error reads the
/// same way in a terminal and in the page.
fn render(errors: &[crate::error::CanonError]) -> String {
    errors
        .iter()
        .map(|err| {
            let span = err.span();
            let path = match err {
                crate::error::CanonError::FormatError { path, .. } => path.as_str(),
                _ => ENTRY,
            };
            format!("{path}:{}:{}: {}", span.line, span.column, err.message())
        })
        .collect::<Vec<_>>()
        .join("\n")
}
