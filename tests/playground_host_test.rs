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
    "canon:async/waitable.join",
    "canon:async/waitable.set-drop",
    "canon:async/waitable.set-new",
    "canon:async/waitable.set-wait",
    "canon:async/waitable.subtask-cancel",
    "canon:async/waitable.subtask-drop",
    "canon:async/waitable.task-return",
    "canon:builtins/json.from-float",
    "wasi:cli/environment.get-arguments",
    "wasi:cli/environment.get-initial-cwd",
    "wasi:cli/exit.exit-with-code",
    "wasi:cli/stderr.future-drop-readable",
    "wasi:cli/stderr.stream-drop-writable",
    "wasi:cli/stderr.stream-new",
    "wasi:cli/stderr.stream-write",
    "wasi:cli/stderr.write-via-stream",
    "wasi:cli/stdout.future-drop-readable",
    "wasi:cli/stdout.stream-drop-readable",
    "wasi:cli/stdout.stream-drop-writable",
    "wasi:cli/stdout.stream-new",
    "wasi:cli/stdout.stream-read",
    "wasi:cli/stdout.stream-write",
    "wasi:cli/stdout.write-via-stream",
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

/// The host identifies the two modules by what they say about
/// themselves: the provider owns memory, the program imports it and
/// exports `run`.
#[test]
fn component_has_a_memory_provider_and_a_program() {
    let component = compile("Unit => Program {\n    \"hi\" -> Print\n}\n");
    let mods = core_modules(&component);
    assert_eq!(mods.len(), 2, "expected exactly two core modules");

    let provider = &mods[0];
    assert!(imports(provider).is_empty(), "the provider imports nothing");
    assert!(exports(provider).contains(&"memory".to_string()));
    assert!(exports(provider).contains(&"bump_ptr".to_string()));

    let program = &mods[1];
    assert!(exports(program).contains(&"run".to_string()));
    let env: Vec<String> = imports(program)
        .into_iter()
        .filter(|(m, _)| m == "env")
        .map(|(_, n)| n)
        .collect();
    assert_eq!(env, vec!["memory".to_string(), "bump_ptr".to_string()]);
}

/// Every import outside `env` has to be one the browser host answers.
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
        let program = core_modules(&component).remove(1);
        for (module, name) in imports(program) {
            if module == "env" {
                continue;
            }
            let full = format!("{module}.{name}");
            assert!(
                hosted.contains(full.as_str()),
                "{full} is imported but docs/assets/canon-play.js does not implement it",
            );
        }
    }
}
