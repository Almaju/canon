//! Tests for Phase 5 async infrastructure:
//!
//! - `Future<T>` and `Stream<T>` are recognised as built-in generic types.
//! - `auto_await::transform` rewrites Future-typed receivers as
//!   `Expr::Await(receiver)` before the checker runs.
//! - `async_analysis::analyse` correctly identifies the set of suspending
//!   functions via direct triggers + bottom-up call-graph propagation.

use canon::ast::{resolve_new_syntax, Expr, Item, Module};
use canon::checker::{self, auto_await};
use canon::codegen::async_analysis;
use canon::lexer::Scanner;
use canon::parser::Parser;

fn parse(source: &str) -> Module {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse().expect("parser failed");
    // Mirror the loader pipeline for a vendored binding file: seed the
    // rewrite with a synthetic path-derived URN so the string-anchored
    // constructors in the test source produce FunctionDefs with
    // `extern_wasm` populated — the rest of the test infrastructure
    // (async_analysis, auto_await) expects that shape. As in the loader,
    // the rewrite runs before `resolve_new_syntax`.
    canon::loader::apply_bindings(&mut module.items, Some("canon:builtins/x@0.1.0"));
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
FutureString = String

Unit => Future<FutureString> {
    "future-string"
}

Unit => Unit {
    FutureString().print()
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
    // `Stream<Int>` must be *recognised* as a type expression — the only
    // acceptable diagnostic is the codegen-gap rejection (`Stream<T>`
    // lowering is unimplemented), never an "unknown type" error.
    let source = r#"
Ticked = String

(Stream<Int>) => Ticked {
    "ticked"
}

Unit => Unit {
    Ticked("done").print()
}
"#;
    let m = parse_and_transform(source);
    let errors = checker::check(&m);
    assert!(
        errors
            .iter()
            .all(|e| e.message().contains("codegen-gaps.md")),
        "Stream<Int> should be recognised as a type expression (only the \
         codegen-gap rejection may fire); got errors: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| e.message().contains("Stream<T>")),
        "reachable Stream use should be rejected as a codegen gap; got: {:?}",
        errors
    );
}

#[test]
fn auto_await_wraps_future_receiver_in_method_call() {
    // A function whose return type is `Future<T>` is async at the
    // canonical-ABI level. When that value is the receiver of a method
    // call, the auto-await transform should wrap the receiver in
    // `Expr::Await`.
    let source = r#"
Waited = String

Network => Future<Waited> {
    "wait"
}

main = (Network) => Unit {
    Waited(Network).print()
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
Greet = (String) => String {
    "hi"
}

main = () => Unit {
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
fn auto_await_wraps_future_operand_of_try() {
    // A `?` operand whose static type is `Future<Result<T, E>>` should be
    // auto-awaited: the source `slowRead(Filesystem)?` becomes
    // `Await(slowRead(Filesystem))?` so the `?` peels the Result against
    // the awaited payload. This is the implicit-await rule applied at the
    // `?` position (mirroring how it already fires at method-receiver
    // positions).
    let source = r#"
SlowRead = String

Filesystem => Future<Result<SlowRead, String>> {
    "slow-read"
}

main = (Filesystem) => Unit {
    SlowRead(Filesystem)?.print()
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

    // Expected post-transform shape:
    //   MethodCall { receiver = Try { inner = Await { inner = Constructor("slowRead", …) } }, method = "print" }
    let call = main.body.exprs.first().expect("empty main body");
    let try_expr = match call {
        Expr::MethodCall { receiver, .. } => receiver.as_ref(),
        other => panic!("expected MethodCall, got {:?}", other),
    };
    let inner = match try_expr {
        Expr::Try { inner, .. } => inner.as_ref(),
        other => panic!("expected Try, got {:?}", other),
    };
    assert!(
        matches!(inner, Expr::Await { .. }),
        "expected `?` operand to be auto-awaited, got: {:?}",
        inner
    );
}

#[test]
fn auto_await_does_not_wrap_sync_try_operand() {
    // A `?` whose operand is a sync `Result<T, E>`-returning call should
    // NOT gain an Await — only `Future<Result<…>>` triggers the rewrite.
    let source = r#"
MyError = String

parse = (String) => Result<Int, MyError> {
    Ok(0)
}

main = () => Unit {
    parse("42")?.print()
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
    fn count_awaits(e: &Expr) -> usize {
        match e {
            Expr::Await { .. } => 1,
            Expr::MethodCall { receiver, args, .. } => {
                count_awaits(receiver) + args.iter().map(count_awaits).sum::<usize>()
            }
            Expr::Constructor { args, .. } => args.iter().map(count_awaits).sum(),
            Expr::Try { inner, .. } => count_awaits(inner),
            _ => 0,
        }
    }
    let total: usize = main.body.exprs.iter().map(count_awaits).sum();
    assert_eq!(total, 0, "sync `?` operand should not gain any Await nodes");
}

#[test]
fn auto_await_wraps_future_argument_at_method_call() {
    // A method-call arg whose static type is `Future<T>` and whose
    // corresponding parameter declared type is `T` should be auto-awaited.
    // The user wrote `target.method(slowFetch())` and the language
    // semantics say `method` expects `T`, so we await first.
    //
    // We use a method on String whose parameter type is exactly the
    // extern's minted result type. `Fetched()` returns `Future<Fetched>`
    // (after the loader's wrap rule), the param is `Fetched`, so the arg
    // gets wrapped.
    let source = r#"
Fetched = String

Unit => Future<Fetched> {
    "fetched"
}

append = (String * Fetched) => String {
    String
}

main = () => Unit {
    "prefix:".append(Fetched()).print()
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

    // Find the `.append(Fetched())` MethodCall and check its args[0]
    // is wrapped in Await.
    fn find_method_call<'a>(e: &'a Expr, name: &str) -> Option<&'a Expr> {
        match e {
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } if method.name == name => Some(e),
            Expr::MethodCall { receiver, args, .. } => find_method_call(receiver, name)
                .or_else(|| args.iter().find_map(|a| find_method_call(a, name))),
            _ => None,
        }
    }
    let call = main
        .body
        .exprs
        .iter()
        .find_map(|e| find_method_call(e, "append"))
        .expect("`.append(...)` call not found");
    let arg0 = match call {
        Expr::MethodCall { args, .. } => args.first().expect("append should have one arg"),
        _ => unreachable!(),
    };
    assert!(
        matches!(arg0, Expr::Await { .. }),
        "expected `append`'s first arg to be auto-awaited; got: {:?}",
        arg0
    );
}

#[test]
fn auto_await_does_not_wrap_arg_when_param_expects_future() {
    // When the callee's parameter is declared as `Future<T>` (not `T`),
    // the auto-await rule must NOT fire — the parameter is asking for
    // the future directly. This is the conservative-match property.
    // `noAwait` declares its parameter as `Future<Fetched>` (not `Fetched`),
    // so passing `Fetched()` — also `Future<Fetched>` — should NOT trigger
    // auto-await: the callee is asking for the unforced future. This is the
    // conservative-match property of `future_inner_matches`.
    let source = r#"
Fetched = String

Unit => Future<Fetched> {
    "fetched"
}

noAwait = (Future<Fetched>) => Unit {
    "side".print()
}

main = () => Unit {
    noAwait(Fetched())
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
    fn count_awaits(e: &Expr) -> usize {
        match e {
            Expr::Await { .. } => 1,
            Expr::MethodCall { receiver, args, .. } => {
                count_awaits(receiver) + args.iter().map(count_awaits).sum::<usize>()
            }
            Expr::Constructor { args, .. } => args.iter().map(count_awaits).sum(),
            Expr::Try { inner, .. } => count_awaits(inner),
            _ => 0,
        }
    }
    let total: usize = main.body.exprs.iter().map(count_awaits).sum();
    assert_eq!(
        total, 0,
        "arg passed where param expects `Future<T>` must NOT gain an Await"
    );
}

#[test]
fn async_analysis_seeds_extern_async_functions() {
    // A function whose return is `Future<T>` is a direct async trigger
    // — its caller must become suspending too.
    let source = r#"
SlowRead = String

Filesystem => Future<SlowRead> {
    "slow-read"
}

main = (Filesystem) => Unit {
    SlowRead(Filesystem).print()
}
"#;
    let m = parse(source);
    let set = async_analysis::analyse(&m);
    // The string-anchored binding is normalised into a `Self`-constructor
    // on its minted result type, so the function-table key is
    // `(Some("SlowRead"), "Self")`. `main` is special-cased by
    // the parser and keeps `(None, "main")` regardless of its params.
    assert!(
        set.contains(&(Some("SlowRead".to_string()), "Self".to_string())),
        "SlowRead (returning Future) should be in the async set; got: {:?}",
        set.iter().collect::<Vec<_>>()
    );
    assert!(
        set.contains(&(None, "main".to_string())),
        "main should be in the async set (transitively calls SlowRead); got: {:?}",
        set.iter().collect::<Vec<_>>()
    );
}

#[test]
fn async_analysis_propagates_through_call_graph() {
    // a → b → Fetched, where Fetched is extern async. All three should be
    // suspending.
    let source = r#"
Fetched = String

Filesystem => Future<Fetched> {
    "fetched"
}

b = (Filesystem) => String {
    Fetched(Filesystem)
}

a = (Filesystem) => String {
    b(Filesystem)
}

main = (Filesystem) => Unit {
    a(Filesystem).print()
}
"#;
    let m = parse(source);
    let set = async_analysis::analyse(&m);
    // The extern's key is its `Self`-constructor `(Some("Fetched"), "Self")`;
    // each bodied non-`main` function takes `Filesystem` as its first
    // param, so its key is `(Some("Filesystem"), <name>)`. `main` keeps
    // `(None, ...)`.
    assert!(
        set.contains(&(Some("Fetched".to_string()), "Self".to_string())),
        "Fetched (returning Future) should be in the async set; got set: {:?}",
        set.iter().collect::<Vec<_>>()
    );
    for name in &["a", "b"] {
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
double = (Int) => Int {
    Int.mul(2)
}

main = (Stdout) => Unit {
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
SyncRead = String

Filesystem => SyncRead {
    "sync-read"
}

main = (Filesystem) => Unit {
    SyncRead(Filesystem).print()
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
Wait = (Network) => Future<String> {
    "hello"
}

main = (Network) => Unit {
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
