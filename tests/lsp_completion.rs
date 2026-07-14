//! Drives the LSP completion provider directly (no JSON-RPC
//! transport): the discovery feature of the `->` / `.` operator split
//! (docs/src/spec/types-only.md § The One-Operator Endgame). `->`
//! offers declarations whose input product contains the piped value's
//! type plus the applicable builtin pipe vocabulary; `.` offers the
//! left value's product components; unresolvable positions degrade
//! gracefully instead of erroring.

use canon::lsp::completion::{completion_items, CompletionItem, CompletionKind};

/// The buffer path only roots import-closure resolution; it need not
/// exist on disk.
const BUFFER: &str = "/no-such-project/buffer.can";

fn labels(items: &[CompletionItem]) -> Vec<&str> {
    items.iter().map(|i| i.label.as_str()).collect()
}

// ---------------------------------------------------------------------------
// `->` — functions whose input product contains the left value's type
// ---------------------------------------------------------------------------

const STRING_PIPE_SRC: &str = "Loud = String\n\nGreeting = String\n\nGreeting => Loud {\n    Greeting -> Joined(\"!\")\n}\n\nUnit => Program {\n    \"hi\" -> \n}\n";

#[test]
fn arrow_after_string_offers_user_and_stdlib_functions_taking_string() {
    // Cursor at the end of `    \"hi\" -> ` (line 9, character 12).
    let items = completion_items(STRING_PIPE_SRC, BUFFER, 9, 12);
    let labels = labels(&items);
    // User constructor whose input is `Greeting = String` — a piped
    // String reaches it through the alias chain.
    assert!(labels.contains(&"Loud"), "user constructor: {labels:?}");
    // Stdlib `String => Uppercased`, discoverable without a reference.
    assert!(
        labels.contains(&"Uppercased"),
        "stdlib constructor: {labels:?}"
    );
    // Builtin pipe vocabulary on String.
    assert!(labels.contains(&"Print"), "builtin: {labels:?}");
    assert!(labels.contains(&"Joined"), "builtin: {labels:?}");
    // Int/Float-only builtins are gated out — the left value is a String.
    assert!(!labels.contains(&"Sum"), "Sum is not on String: {labels:?}");
    let loud = items.iter().find(|i| i.label == "Loud").unwrap();
    assert_eq!(loud.kind, CompletionKind::Constructor);
    assert_eq!(loud.detail, "Greeting => Loud");
}

#[test]
fn arrow_completion_survives_a_partially_typed_name() {
    // Same buffer, but the user has typed `-> Lo` already (the editor
    // filters on the prefix; the provider must still fire).
    let src = STRING_PIPE_SRC.replace("\"hi\" -> \n", "\"hi\" -> Lo\n");
    let items = completion_items(&src, BUFFER, 9, 14);
    assert!(items.iter().any(|i| i.label == "Loud"));
}

#[test]
fn arrow_after_int_offers_int_builtins_not_string_ones() {
    let src = "Unit => Program {\n    1 -> \n}\n";
    let items = completion_items(src, BUFFER, 1, 9);
    let labels = labels(&items);
    assert!(labels.contains(&"Sum"), "{labels:?}");
    assert!(labels.contains(&"Eq"), "{labels:?}");
    assert!(!labels.contains(&"Substring"), "{labels:?}");
}

#[test]
fn arrow_types_the_whole_chain_left_of_the_cursor() {
    // `1 -> Sum(2)` is an Int, so the second arrow offers Int builtins.
    let src = "Unit => Program {\n    1 -> Sum(2) -> \n}\n";
    let items = completion_items(src, BUFFER, 1, 19);
    let labels = labels(&items);
    assert!(labels.contains(&"Sum"), "{labels:?}");
    assert!(!labels.contains(&"Substring"), "{labels:?}");
}

#[test]
fn continuation_line_resolves_the_chain_from_the_previous_line() {
    // Canon chains wrap with the operator opening the next line; the
    // provider walks back to type the value.
    let src = "Unit => Program {\n    \"hi\"\n        -> \n}\n";
    let items = completion_items(src, BUFFER, 2, 11);
    let labels = labels(&items);
    assert!(labels.contains(&"Print"), "{labels:?}");
    assert!(!labels.contains(&"Sum"), "{labels:?}");
}

// ---------------------------------------------------------------------------
// `.` — the left value's fields/components
// ---------------------------------------------------------------------------

#[test]
fn dot_after_product_value_offers_component_names() {
    let src = "Birthday = Int\n\nUsername = String\n\nUser = Birthday * Username\n\nUser => Shown {\n    User.\n}\n";
    let items = completion_items(src, BUFFER, 7, 9);
    assert_eq!(labels(&items), vec!["Birthday", "Username"]);
    assert!(items.iter().all(|i| i.kind == CompletionKind::Field));
}

#[test]
fn dot_after_repeated_component_product_offers_positional_indexes() {
    // Both components are `Int`, so only position distinguishes them —
    // the 1-based indexes are offered alongside the type name.
    let src = "Pair = Int * Int\n\nPair => Shown {\n    Pair.\n}\n";
    let items = completion_items(src, BUFFER, 3, 9);
    assert_eq!(labels(&items), vec!["Int", "1", "2"]);
}

#[test]
fn dot_after_newtype_offers_the_unwrap_component() {
    // A newtype is a 1-component product; `.String` unwraps it.
    let src = "Greeting = String\n\nGreeting => Shown {\n    Greeting.\n}\n";
    let items = completion_items(src, BUFFER, 3, 13);
    assert_eq!(labels(&items), vec!["String"]);
}

// ---------------------------------------------------------------------------
// Graceful degradation
// ---------------------------------------------------------------------------

#[test]
fn unresolvable_arrow_degrades_to_the_full_declaration_list() {
    let src = "Unit => Program {\n    mystery -> \n}\n";
    let items = completion_items(src, BUFFER, 1, 15);
    assert!(!items.is_empty());
    // The full list still surfaces stdlib declarations and builtins.
    assert!(items.iter().any(|i| i.label == "Uppercased"));
    assert!(items.iter().any(|i| i.label == "Print"));
}

#[test]
fn unresolvable_dot_returns_empty() {
    let src = "Unit => Program {\n    mystery.\n}\n";
    assert!(completion_items(src, BUFFER, 1, 12).is_empty());
}

#[test]
fn position_without_trigger_returns_empty() {
    let src = "Unit => Program {\n    \"hi\" -> Print\n}\n";
    assert!(completion_items(src, BUFFER, 0, 5).is_empty());
    // Out-of-range positions degrade to empty, never panic.
    assert!(completion_items(src, BUFFER, 99, 0).is_empty());
    assert!(completion_items(src, BUFFER, 2, 999).is_empty());
}

#[test]
fn gt_that_is_not_a_pipe_arrow_does_not_complete() {
    // The `>` trigger character requires the preceding `-`: the closing
    // angle of a generic never opens completion.
    let src = "Wrapped = Option<Int>\n";
    assert!(completion_items(src, BUFFER, 0, 21).is_empty());
}
