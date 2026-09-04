//! Pins the contract between the compiler and the docs playground's
//! browser host (`docs/assets/canon-play.js`).
//!
//! The playground runs a compiled program by instantiating the
//! component's core modules directly — the browser has no component
//! runtime, so the host stands in for the canonical-ABI wrapper. That
//! only works while the emitted component keeps the shape the host
//! expects, and while it imports nothing the host hasn't implemented.
//! Both are checked here, because neither is visible from the JS side
//! until the page is already broken.
//!
//! A failure here is not necessarily a codegen bug: it means
//! `canon-play.js` needs to learn about whatever changed.

use canon::{checker, codegen, formatter, loader};
use std::collections::BTreeSet;
use std::path::Path;
use wasmparser::{Imports, Parser, Payload};

/// Every core-level import `canon-play.js` implements. Anything outside
/// this set reaches the host's fallback stub, which throws in the
/// reader's face instead of running.
const HOSTED: &[&str] = &[
    "$root.[subtask-cancel]",
    "$root.[subtask-drop]",
    "$root.[waitable-join]",
    "$root.[waitable-set-drop]",
    "$root.[waitable-set-new]",
    "$root.[waitable-set-wait]",
    "[export]wasi:cli/run@0.3.0-rc-2026-03-15.[task-return]run",
    "wasi:cli/environment@0.3.0-rc-2026-03-15.get-arguments",
    "wasi:cli/environment@0.3.0-rc-2026-03-15.get-initial-cwd",
    "wasi:cli/exit@0.3.0-rc-2026-03-15.exit-with-code",
    "wasi:cli/stdin@0.3.0-rc-2026-03-15.[future-drop-readable-1]read-via-stream",
    "wasi:cli/stdin@0.3.0-rc-2026-03-15.[stream-drop-readable-0]read-via-stream",
    "wasi:cli/stdin@0.3.0-rc-2026-03-15.[stream-read-0]read-via-stream",
    "wasi:cli/stdin@0.3.0-rc-2026-03-15.read-via-stream",
    "wasi:cli/stdout@0.3.0-rc-2026-03-15.[future-drop-readable-1]write-via-stream",
    "wasi:cli/stdout@0.3.0-rc-2026-03-15.[stream-drop-writable-0]write-via-stream",
    "wasi:cli/stdout@0.3.0-rc-2026-03-15.[stream-new-0]write-via-stream",
    "wasi:cli/stdout@0.3.0-rc-2026-03-15.[stream-write-0]write-via-stream",
    "wasi:cli/stdout@0.3.0-rc-2026-03-15.write-via-stream",
];

/// Format then compile — the playground's own path, so these sources
/// stay readable here instead of being pre-canonicalised by hand.
fn compile(source: &str) -> Vec<u8> {
    let canonical = formatter::format(source).expect("format failed");
    let loaded = loader::load_text(Path::new("playground.can"), &canonical).expect("load failed");
    let errors = checker::check_loaded(&loaded);
    assert!(
        errors.is_empty(),
        "checker rejected the program: {errors:?}"
    );
    codegen::generate(&loaded.module)
}

/// The nested core modules, in the order the component declares them —
/// the same walk `coreModules` does in canon-play.js.
fn core_modules(component: &[u8]) -> Vec<&[u8]> {
    let mut mods = Vec::new();
    for payload in Parser::new(0).parse_all(component) {
        if let Payload::ModuleSection {
            unchecked_range, ..
        } = payload.expect("malformed component")
        {
            mods.push(&component[unchecked_range]);
        }
    }
    mods
}

fn imports(module: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for payload in Parser::new(0).parse_all(module) {
        if let Payload::ImportSection(reader) = payload.expect("malformed core module") {
            for group in reader {
                // Codegen emits one entry per import; the compact
                // encodings are a wasmparser-side possibility we never
                // produce, and the host's flattening assumes as much.
                let Imports::Single(_, import) = group.expect("malformed import") else {
                    panic!("compact import encoding: canon-play.js flattens imports itself");
                };
                out.push((import.module.to_string(), import.name.to_string()));
            }
        }
    }
    out
}

fn exports(module: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for payload in Parser::new(0).parse_all(module) {
        if let Payload::ExportSection(reader) = payload.expect("malformed core module") {
            for export in reader {
                out.push(export.expect("malformed export").name.to_string());
            }
        }
    }
    out
}

/// The program is the one core module that owns memory (wit-component
/// adds import shims beside it); it is self-contained — its own memory
/// and allocator — and exports the async-stackful `run`.
fn program_module(component: &[u8]) -> &[u8] {
    core_modules(component)
        .into_iter()
        .find(|m| exports(m).contains(&"memory".to_string()))
        .expect("one core module exports memory")
}

#[test]
fn the_program_module_is_self_contained() {
    let component = compile("Unit => Program {\n    \"hi\" -> Print\n}\n");
    let program = program_module(&component);
    let exported = exports(program);
    assert!(exported.contains(&"cabi_realloc".to_string()));
    assert!(
        exported.contains(&"[async-lift-stackful]wasi:cli/run@0.3.0-rc-2026-03-15#run".to_string())
    );
    assert!(
        !imports(program).iter().any(|(m, _)| m == "env"),
        "the program imports no memory"
    );
}

/// Every import has to be one the browser host answers.
#[test]
fn program_imports_stay_within_the_browser_host() {
    let hosted: BTreeSet<&str> = HOSTED.iter().copied().collect();
    // Three programs: the first prints and nothing else, the second
    // also reaches for arguments (`Args()`) and the exit code
    // (`Exited`). The third encodes a float, the one thing JSON still
    // needs a host for.
    let sources = [
        "Unit => Program {\n    \"hi\" -> Print\n}\n",
        "Unit => Program {\n    Args() -> Length -> Print\n    Exited(0)\n}\n",
        "Unit => Program {\n    1.5 -> Encoded -> String -> Print\n}\n",
    ];
    for source in sources {
        let component = compile(source);
        let program = program_module(&component);
        for (module, name) in imports(program) {
            let full = format!("{module}.{name}");
            assert!(
                hosted.contains(full.as_str()),
                "{full} is imported but docs/assets/canon-play.js does not implement it",
            );
        }
    }
}
