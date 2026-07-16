//! Drift guard for the documentation examples.
//!
//! The docs site is a Canon web app that renders the `docs/src/*.md`
//! pages with the stdlib Markdown renderer, which has no `{{#include}}`
//! preprocessor. So the example pages inline the real program source
//! directly. This test is what mdBook's `{{#include}}` used to give us:
//! it fails if an inlined block drifts from the program it documents.
//! When an example changes, re-inline its source into the matching page.

use std::fs;
use std::path::Path;

fn assert_inlined(md: &str, src: &str) {
    let root = env!("CARGO_MANIFEST_DIR");
    let md_txt =
        fs::read_to_string(Path::new(root).join(md)).unwrap_or_else(|e| panic!("read {md}: {e}"));
    let src_txt =
        fs::read_to_string(Path::new(root).join(src)).unwrap_or_else(|e| panic!("read {src}: {e}"));
    let needle = src_txt.trim_end_matches('\n');
    assert!(
        md_txt.contains(needle),
        "{md} no longer contains the verbatim source of {src}.\n\
         The docs page has drifted from the program it documents — \
         re-inline {src} into {md}."
    );
}

#[test]
fn example_sources_are_inlined_verbatim() {
    assert_inlined(
        "docs/src/examples/multifile.md",
        "examples/multifile/src/greeter.can",
    );
    assert_inlined(
        "docs/src/examples/multifile.md",
        "examples/multifile/src/main.can",
    );
    assert_inlined(
        "docs/src/examples/notes-api.md",
        "examples/notes-api/src/main.can",
    );
    assert_inlined(
        "docs/src/examples/todolist.md",
        "examples/todolist-web/src/main.can",
    );
    assert_inlined(
        "docs/src/examples/fullstack.md",
        "examples/todo-fullstack/src/web.can",
    );
    assert_inlined(
        "docs/src/examples/fullstack.md",
        "examples/todo-fullstack/src/server.can",
    );
}
