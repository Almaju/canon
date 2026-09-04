//! `canon doc` over the standard library — the generator's only corpus
//! big enough to have edge cases, and the one whose output ships.
//!
//! This replaces the hand-maintained Reserved Names appendix and the
//! line-scraping guard that policed it (`tests/stdlib_doc.rs`): the
//! generated index *is* the reserved-name list, derived from the same
//! parser the compiler uses, so the two cannot drift.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use canon::doc;

fn std_src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("packages/canon/src")
}

fn tmpdir(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target");
    p.push("doc-test-tmp");
    p.push(name);
    if p.exists() {
        fs::remove_dir_all(&p).expect("could not clean tmpdir");
    }
    fs::create_dir_all(&p).expect("could not create tmpdir");
    p
}

fn api() -> doc::Api {
    package_api("canon", &std_src())
}

fn package_api(name: &str, src: &Path) -> doc::Api {
    doc::build(name, src)
        .unwrap_or_else(|(path, err)| panic!("canon doc failed on {}: {:?}", path.display(), err))
}

#[test]
fn every_stdlib_type_gets_a_page() {
    let api = api();
    let names: BTreeSet<&str> = api.type_names().map(|s| s.as_str()).collect();

    assert!(
        names.len() > 150,
        "declaration extraction looks broken: {} names",
        names.len()
    );
    assert!(
        api.modules.len() > 30,
        "stdlib walk looks broken: {} modules",
        api.modules.len()
    );

    // A spread of shapes: an alias, a recursive union, a product, a
    // result newtype, a validated constructor's error, a binding-backed
    // type.
    for expected in [
        "Contents",
        "File",
        "IoError",
        "Json",
        "Map",
        "MalformedInt",
        "Node",
        "Now",
        "Program",
        "Random",
        "Set",
        "TestResult",
        "Url",
    ] {
        assert!(names.contains(expected), "no page for `{}`", expected);
    }

    // Tag-only union variants (`Map = Empty + Node` with no `Empty = …`
    // anywhere) are types you dispatch on, but they have no declaration
    // line — the blind spot that let them go missing from the appendix
    // the old guard checked.
    for variant in ["Absent", "Empty"] {
        assert!(
            names.contains(variant),
            "tag-only union variant `{}` has no page",
            variant
        );
    }

    // camelCase binding aliases are the FFI boundary, not types.
    for name in &names {
        assert!(
            name.starts_with(|c: char| c.is_ascii_uppercase()),
            "`{}` is not a type name and should not be documented",
            name
        );
    }
}

#[test]
fn a_type_page_carries_its_definition_constructors_and_pipe_menu() {
    let out = tmpdir("pages");
    let api = api();
    doc::render(&api, &out).expect("render");

    let map = fs::read_to_string(out.join("type/Map.html")).expect("Map page");
    assert!(
        map.contains("Map = <a href=\"../type/Empty.html\">Empty</a> + "),
        "Map's definition is missing"
    );
    assert!(map.contains("Variants"), "Map's variants are missing");
    // The payload: what a `Map` pipes into, in call form. No
    // hand-written page can keep this list honest.
    assert!(map.contains("Pipes into"), "Map has no pipe menu");
    for piped in ["Insert", "Keys", "Length", "Remove", "Values"] {
        assert!(
            map.contains(&format!(">{}</a>(", piped)) || map.contains(&format!(">{}</a>", piped)),
            "`Map -> {}` missing from Map's pipe menu",
            piped
        );
    }

    // A fallible constructor renders in both directions: declaration
    // form where it is declared, call form (with `?`) where it is
    // reached by a pipe.
    let read = fs::read_to_string(out.join("type/Read.html")).expect("Read page");
    assert!(
        read.contains("=&gt; Result&lt;"),
        "Read's fallible constructor is missing its declaration form"
    );
    let file = fs::read_to_string(out.join("type/File.html")).expect("File page");
    assert!(
        file.contains("-&gt; <a href=\"../type/Read.html\">Read</a>?"),
        "`File -> Read?` missing from File's pipe menu"
    );
}

#[test]
fn the_index_lists_every_type_and_module() {
    let out = tmpdir("index");
    let api = api();
    doc::render(&api, &out).expect("render");

    let index = fs::read_to_string(out.join("index.html")).expect("index");
    for name in api.type_names() {
        assert!(
            index.contains(&format!("type/{}.html", name)),
            "`{}` is missing from the index",
            name
        );
    }
    for module in &api.modules {
        assert!(
            index.contains(&format!("module/{}.html", module.path)),
            "module `{}` is missing from the index",
            module.path
        );
    }
}

/// Cross-links are total by construction — names are global and
/// unambiguous — so a dangling one means the renderer, not the source,
/// is wrong.
#[test]
fn no_generated_link_dangles() {
    let out = tmpdir("links");
    let api = api();
    doc::render(&api, &out).expect("render");

    let mut pages = Vec::new();
    collect(&out, "html", &mut pages);
    assert!(pages.len() > 100, "only {} pages rendered", pages.len());

    for page in &pages {
        let html = fs::read_to_string(page).expect("read page");
        let dir = page.parent().expect("page has a parent");
        for href in hrefs(&html) {
            let target = dir.join(&href);
            assert!(
                target.exists(),
                "{} links to `{}`, which was not generated",
                page.strip_prefix(&out).unwrap_or(page).display(),
                href
            );
        }
    }
}

fn collect(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, ext, out);
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

/// Every `href="…"` in `html`, skipping absolute URLs and fragments.
fn hrefs(html: &str) -> Vec<String> {
    html.split("href=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .filter(|h| !h.starts_with("http") && !h.starts_with('#'))
        .map(|h| h.split('#').next().unwrap_or(h).to_string())
        .collect()
}

/// A module page is the export list a reader scans to learn what a
/// file offers: every declaration in declaration form, each name a
/// link to the constructed type.
#[test]
fn a_module_page_lists_its_declarations() {
    let out = tmpdir("module");
    let api = api();
    doc::render(&api, &out).expect("render");

    let map = fs::read_to_string(out.join("module/map.html")).expect("map module page");
    assert!(map.contains("Declarations"), "no declaration list");
    assert!(
        map.contains("<a href=\"../type/Map.html\">Map</a> * <a href=\"../type/Key.html\">Key</a> * <a href=\"../type/Value.html\">Value</a> =&gt; <a href=\"../type/Inserted.html\">Inserted</a>"),
        "`Map * Key * Value => Inserted` is missing from map's declarations"
    );
}

/// `canon doc` over a directory of packages renders them as one site:
/// each under its own path with a link back up, an index listing them,
/// and one search over every package's types.
#[test]
fn a_directory_of_packages_renders_as_one_site() {
    let out = tmpdir("site");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("packages");
    let apis = vec![
        package_api("canon", &root.join("canon/src")),
        package_api("canon/ansi", &root.join("canon/ansi/src")),
    ];
    doc::render_site(&apis, &out).expect("render site");

    let index = fs::read_to_string(out.join("index.html")).expect("site index");
    assert!(
        index.contains("href=\"canon/index.html\""),
        "prelude missing from the index"
    );
    assert!(
        index.contains("href=\"canon/ansi/index.html\""),
        "canon/ansi missing from the index"
    );

    let styled = fs::read_to_string(out.join("canon/ansi/type/Styled.html")).expect("Styled page");
    assert!(
        styled.contains("href=\"../../../index.html\">&#8592; all packages"),
        "a package page links back to the site index"
    );
    assert!(
        styled.contains("<a href=\"../type/String.html\">String</a> * <a href=\"../type/Style.html\">Style</a> =&gt; <a href=\"../type/Styled.html\">Styled</a>")
            || styled.contains("String * <a href=\"../type/Style.html\">Style</a> =&gt; <a href=\"../type/Styled.html\">Styled</a>"),
        "Styled's constructor is missing:\n{styled}"
    );

    let search = fs::read_to_string(out.join("search.js")).expect("site search");
    assert!(
        search.contains("[\"canon/ansi\", \"Style\"]"),
        "search index lacks canon/ansi"
    );
    assert!(
        search.contains("[\"canon\", \"Map\"]"),
        "search index lacks the prelude"
    );
}
