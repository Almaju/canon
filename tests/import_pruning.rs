//! Codegen compiles the reachable set, so a program's host imports are
//! its actual dependencies — not everything the files it touched happened
//! to declare. The loader is file-granular: naming one binding pulls in
//! its whole file, siblings included. `checker::prune_to_reachable` is
//! what narrows that back down before codegen links the import block.
//!
//! Both directions matter, so both are pinned here: what pruning drops,
//! and what it must keep. The "keep" half is the sharp edge — a hole in a
//! literal and a `Print` of a non-string reach stdlib families that the
//! source never names, and dropping those is a codegen panic, not a
//! smaller binary.

use canon::ast::Item;
use canon::{checker, codegen, loader};
use std::path::Path;

/// Load, check, prune. Panics with the diagnostics if the program does
/// not check — a broken fixture should not read as a passing assertion.
fn pruned(src: &str) -> canon::ast::Module {
    let loaded = loader::load_text(Path::new("prune.can"), src).expect("program should load");
    let errors = checker::check_loaded(&loaded);
    assert!(
        errors.is_empty(),
        "fixture should check, got: {}",
        errors
            .iter()
            .map(|e| e.message())
            .collect::<Vec<_>>()
            .join("; ")
    );
    checker::prune_to_reachable(&loaded.module, loaded.entry_items_start)
}

/// Every `extern Wasm` path that survives the prune — the program's
/// import block, in effect.
fn pruned_externs(src: &str) -> Vec<String> {
    pruned(src)
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(f) => f.extern_wasm.as_ref().map(|e| e.path.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn json_call_drops_the_unreached_float_bridge() {
    // `-> Json` over a list of strings loads `canon/std/Json`, whose
    // float member is host-backed. No call site can reach that member,
    // so the bridge has no business in the component.
    let externs =
        pruned_externs("Unit => Program {\n    Args()\n        -> Json\n        -> Print\n}\n");
    assert!(
        !externs.iter().any(|p| p.contains("canon:builtins/json")),
        "the float bridge is unreachable from a string encode, got: {externs:?}"
    );
}

#[test]
fn json_literal_hole_keeps_the_float_encoder() {
    // A hole lowers to `-> Encoded`, and the family is selected by the
    // hole's runtime type — so every member stays reachable, the
    // host-backed `Float` one included. This is the residue that
    // retiring `from-float` removes; until then, pin it honestly.
    let externs = pruned_externs(
        "Labeled = Json\n\nInt => Labeled {\n    {\"answer\":Int}\n}\n\n\
         Unit => Program {\n    Labeled(42) -> Print\n}\n",
    );
    assert!(
        externs.iter().any(|p| p.contains("canon:builtins/json")),
        "an interpolating literal reaches the whole `Encoded` family, got: {externs:?}"
    );
}

#[test]
fn print_of_an_int_keeps_the_string_renderer() {
    // `Print` renders through the `String` family without naming it, so
    // a purely syntactic reachability walk drops the renderer and
    // codegen panics looking it up. Generating is the assertion.
    let module = pruned("Unit => Program {\n    5\n        -> Sum(1)\n        -> Print\n}\n");
    assert!(
        !codegen::generate(&module).is_empty(),
        "pruned module should still generate a component"
    );
}
