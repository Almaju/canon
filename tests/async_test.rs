//! Tests for Phase 5 async infrastructure:
//!
//! - `Future<T>` and `Stream<T>` are recognised as built-in generic types.
//! - `auto_await::transform` rewrites Future-typed receivers as
//!   `Expr::Await(receiver)` before the checker runs.
//! - `async_analysis::analyse` correctly identifies the set of suspending
//!   functions via direct triggers + bottom-up call-graph propagation.

use oneway::ast::{resolve_new_syntax, Expr, Item, Module};
use oneway::checker::{self, auto_await};
use oneway::codegen::async_analysis;
use oneway::lexer::Scanner;
use oneway::parser::Parser;

fn parse(source: &str) -> Module {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse().expect("parser failed");
    resolve_new_syntax(&mut module);
    module
}

fn parse_and_transform(source: &str) -> Module {
    let mut m = parse(source);
    auto_await::transform(&mut m);
    m
}

#[test]
fn future_is_a_known_generic_type_in_extern_decl() {
    // `Future<T>` and `Stream<T>` are never written by user code in normal
    // function definitions — they're an internal compile-time wrapping the
    // checker applies to async externs. But the checker MUST accept them
    // as valid type expressions wherever a type is permitted (e.g. inside
    // an extern declaration that explicitly returns one, or in a
    // type-alias). This test asserts that recognising the name no longer
    // produces an "unknown type" error.
    let source = r#"
extern Wasm("oneway:builtins/x@0.1.0#future-string")
futureString = () -> Future<String>

main = () -> Unit {
    "hello".print()
}
"#;
    let m = parse_and_transform(source);
    let errors = checker::check(&m);
    assert!(
        errors.is_empty(),
        "Future<String> should be accepted as a type expression; got errors: {:?}",
        errors
    );
}

#[test]
fn stream_is_a_known_generic_type_in_extern_decl() {
    let source = r#"
extern Wasm("oneway:builtins/x@0.1.0#tick")
tick = () -> Stream<Int>

main = () -> Unit {
    "hello".print()
}
"#;
    let m = parse_and_transform(source);
    let errors = checker::check(&m);
    assert!(
        errors.is_empty(),
        "Stream<Int> should be accepted as a type expression; got errors: {:?}",
        errors
    );
}

#[test]
fn auto_await_wraps_future_receiver_in_method_call() {
    // An `extern Wasm.async` function is implicitly typed `Future<T>` at
    // call sites. When that value is the receiver of a method call, the
    // auto-await transform should wrap the receiver in `Expr::Await`.
    let source = r#"
extern Wasm.async("oneway:builtins/x@0.1.0#wait")
wait = (Network) -> String

main = (Network) -> Unit {
    wait(Network).print()
}
"#;
    let module = parse_and_transform(source);

    // Find `main`'s body and inspect the `.print()` call.
    let main = module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Function(f) if f.name.name == "main" => Some(f),
            _ => None,
        })
        .expect("main function not found");
    let call = main.body.exprs.first().expect("empty main body");
    let receiver = match call {
        Expr::MethodCall { receiver, .. } => receiver.as_ref(),
        other => panic!("expected MethodCall, got {:?}", other),
    };
    assert!(
        matches!(receiver, Expr::Await { .. }),
        "expected method receiver to be auto-awaited, got: {:?}",
        receiver
    );
}

#[test]
fn auto_await_does_not_wrap_sync_receiver() {
    // A method whose receiver is a sync constructor call should NOT be
    // wrapped — only `Future<T>` triggers the rewrite.
    let source = r#"
Greet = (String) -> String {
    "hi"
}

main = () -> Unit {
    "name".Greet().print()
}
"#;
    let module = parse_and_transform(source);
    let main = module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Function(f) if f.name.name == "main" => Some(f),
            _ => None,
        })
        .expect("main not found");
    // Traverse: `"name".Greet().print()` — the outermost call's receiver
    // is `"name".Greet()`, whose own receiver is `"name"` (a StringLit).
    // None of them should be wrapped in `Expr::Await`.
    fn count_awaits(e: &Expr) -> usize {
        match e {
            Expr::Await { .. } => 1,
            Expr::MethodCall { receiver, args, .. } => {
                count_awaits(receiver) + args.iter().map(count_awaits).sum::<usize>()
            }
            Expr::Constructor { args, .. } => args.iter().map(count_awaits).sum(),
            _ => 0,
        }
    }
    let total: usize = main.body.exprs.iter().map(count_awaits).sum();
    assert_eq!(total, 0, "sync call chain should not gain any Await nodes");
}

#[test]
fn async_analysis_seeds_extern_async_functions() {
    // An `extern Wasm.async("...")` is a direct trigger — its caller must
    // become suspending too.
    let source = r#"
extern Wasm.async("wasi:filesystem/types@0.3.0#read-via-stream")
slowRead = (Filesystem) -> String

main = (Filesystem) -> Unit {
    slowRead(Filesystem).print()
}
"#;
    let m = parse(source);
    let set = async_analysis::analyse(&m);
    // `slowRead = (Filesystem) -> String` is normalised by the parser so
    // that `Filesystem` becomes the receiver; the function-table key is
    // therefore `(Some("Filesystem"), "slowRead")`. `main` is special-cased
    // by the parser and keeps `(None, "main")` regardless of its params.
    assert!(
        set.contains(&(Some("Filesystem".to_string()), "slowRead".to_string())),
        "slowRead (extern Wasm.async) should be in the async set; got: {:?}",
        set.iter().collect::<Vec<_>>()
    );
    assert!(
        set.contains(&(None, "main".to_string())),
        "main should be in the async set (transitively calls slowRead); got: {:?}",
        set.iter().collect::<Vec<_>>()
    );
}

#[test]
fn async_analysis_propagates_through_call_graph() {
    // a → b → c, where c is extern async. All three should be suspending.
    let source = r#"
extern Wasm.async("wasi:filesystem/types@0.3.0#read-via-stream")
c = (Filesystem) -> String

b = (Filesystem) -> String {
    c(Filesystem)
}

a = (Filesystem) -> String {
    b(Filesystem)
}

main = (Filesystem) -> Unit {
    a(Filesystem).print()
}
"#;
    let m = parse(source);
    let set = async_analysis::analyse(&m);
    // Each non-`main` function takes `Filesystem` as its first param, so
    // its key is `(Some("Filesystem"), <name>)`. `main` keeps `(None, ...)`.
    for name in &["a", "b", "c"] {
        let key = (Some("Filesystem".to_string()), name.to_string());
        assert!(
            set.contains(&key),
            "{} should be suspending (transitive call to extern async); got set: {:?}",
            name,
            set.iter().collect::<Vec<_>>()
        );
    }
    assert!(
        set.contains(&(None, "main".to_string())),
        "main should be suspending (transitive call to extern async); got set: {:?}",
        set.iter().collect::<Vec<_>>()
    );
}

#[test]
fn async_analysis_leaves_sync_functions_alone() {
    // No async triggers anywhere → empty async set.
    let source = r#"
double = (Int) -> Int {
    Int.mul(2)
}

main = (Stdout) -> Unit {
    "hello".print(Stdout)
}
"#;
    let m = parse(source);
    let set = async_analysis::analyse(&m);
    assert!(
        set.is_empty(),
        "no extern async / Future / Stream in the program — async set should be empty; got: {:?}",
        set.iter().collect::<Vec<_>>()
    );
}

#[test]
fn async_analysis_with_no_extern_async_returns_empty_for_sync_extern() {
    // A non-`.async` extern (synchronous) should not poison the async set.
    let source = r#"
extern Wasm("oneway:builtins/x@0.1.0#sync-read")
syncRead = (Filesystem) -> String

main = (Filesystem) -> Unit {
    syncRead(Filesystem).print()
}
"#;
    let m = parse(source);
    let set = async_analysis::analyse(&m);
    assert!(
        set.is_empty(),
        "sync externs should not trigger async inference; got: {:?}",
        set.iter().collect::<Vec<_>>()
    );
}

#[test]
fn auto_await_is_idempotent() {
    // Running the transform twice should not double-wrap Future receivers.
    let source = r#"
Wait = (Network) -> Future<String> {
    "hello"
}

main = (Network) -> Unit {
    Wait(Network).print()
}
"#;
    let mut m = parse(source);
    auto_await::transform(&mut m);
    let after_one = format!("{:?}", m);
    auto_await::transform(&mut m);
    let after_two = format!("{:?}", m);
    assert_eq!(
        after_one, after_two,
        "auto_await::transform must be idempotent"
    );
}
