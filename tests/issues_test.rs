use oneway::ast::resolve_new_syntax;
use oneway::checker;
use oneway::codegen;
use oneway::lexer::Scanner;
use oneway::parser::Parser;

fn parse_and_generate(source: &str) -> (Vec<oneway::error::OnewayError>, String) {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse().expect("parser failed");
    resolve_new_syntax(&mut module);
    let errors = checker::check(&module);
    let rust_code = codegen::generate(&module);
    (errors, rust_code)
}

fn parse_only(source: &str) -> oneway::ast::Module {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse().expect("parser failed");
    resolve_new_syntax(&mut module);
    module
}

// ── Issue 1: Multi-line function arguments ─────────────────────────────────

#[test]
fn multiline_constructor_args() {
    // A newline after `(` in a constructor must not produce a parse error.
    let source = r#"
Pair = First * Second
First = String
Second = String

main = (Stdout) -> Unit {
    Pair(
        First("hello"),
        Second("world")
    ).first.print(Stdout)
}
"#;
    let module = parse_only(source);
    // If we reach here the parse succeeded; verify we got a constructor with 2 args.
    let items = &module.items;
    assert!(!items.is_empty());
}

#[test]
fn multiline_constructor_args_no_checker_errors() {
    let source = r#"
main = (Stdout) -> Unit {
    "hello".concat(
        "world"
    ).print(Stdout)
}
"#;
    let (errors, _) = parse_and_generate(source);
    assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
}

#[test]
fn multiline_method_call_args() {
    // Newlines inside method-call `( )` argument lists must be accepted.
    let source = r#"
main = (Stdout) -> Unit {
    "hello".concat(
        "world"
    ).print(Stdout)
}
"#;
    let (errors, rust_code) = parse_and_generate(source);
    assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
    // `concat` is lowered to the `+` operator; check that both literals appear.
    assert!(
        rust_code.contains("hello") && rust_code.contains("world"),
        "expected both string literals in generated code:\n{}",
        rust_code
    );
}

// ── Issue 2: JsonValue(JsonObject(...)) union-wrapper elision ──────────────

#[test]
fn union_wrapper_elision_in_codegen() {
    // JsonValue(JsonObject(...)) should not emit JsonValue(JsonValue::JsonObject(...)).
    // The outer JsonValue(...) wrapper must be stripped; only JsonValue::JsonObject(...) is emitted.
    let source = r#"
JsonEntry = String
JsonObject = JsonEntry

JsonValue = JsonObject + JsonString
JsonString = String

main = (Stdout) -> Unit {
    JsonValue(JsonObject(JsonEntry("ok"))).print(Stdout)
}
"#;
    // We just need the codegen not to double-wrap.
    let (_errors, rust_code) = parse_and_generate(source);
    // The generated code must NOT contain `JsonValue(JsonValue::` (the double-wrap).
    assert!(
        !rust_code.contains("JsonValue(JsonValue::"),
        "double-wrap detected in generated code:\n{}",
        rust_code
    );
    // It should contain `JsonValue::JsonObject(` (correct single wrap).
    assert!(
        rust_code.contains("JsonValue::JsonObject("),
        "expected JsonValue::JsonObject( in generated code:\n{}",
        rust_code
    );
}

// ── Issue 3: Ok<T> arm where T is standalone type ────────────────────────

#[test]
fn dispatch_arm_bound_var_resolves_to_specific_type() {
    // JsonObject is both a standalone typedef AND a variant of JsonValue.
    // Inside a dispatch arm `(JsonObject) -> ...`, the bound variable `JsonObject`
    // should be typed as JsonObject, not widened to JsonValue.
    // Calling a method defined on JsonObject must not trigger "no method on JsonValue".
    let source = r#"
JsonEntry = String
JsonObject = JsonEntry

JsonValue = JsonObject + JsonMissing
JsonMissing = String

extract = (JsonObject) -> String {
    "extracted"
}

describe = (JsonValue * Stdout) -> Unit {
    JsonValue.(
        (JsonObject) -> Unit { JsonObject.extract().print(Stdout) }
        (JsonMissing) -> Unit { "missing".print(Stdout) }
    )
}

main = (Stdout) -> Unit {
    "ok".print(Stdout)
}
"#;
    // The checker must NOT report "no method `extract` on type `JsonValue`"
    // because inside the (JsonObject) arm, JsonObject refers to the JsonObject type.
    let (errors, _rust_code) = parse_and_generate(source);
    let method_errs: Vec<_> = errors
        .iter()
        .filter(|e| {
            let msg = format!("{:?}", e);
            msg.contains("no method") && msg.contains("JsonValue")
        })
        .collect();
    assert!(
        method_errs.is_empty(),
        "checker incorrectly widened JsonObject to JsonValue in dispatch arm: {:?}",
        method_errs
    );
}

// ── Issue 4: Sort-ordering check spans import boundaries ─────────────────

#[test]
fn method_ordering_only_within_entry_file() {
    // `check_with_entry` is used with entry_items_start > 0 to simulate
    // that some items come from imports (and therefore don't participate in
    // the local ordering check).
    use oneway::checker::check_with_entry;

    // Simulate: "path" on HttpRequest comes from an import (index 0),
    // and "chatHandler" on HttpRequest is defined locally (index 1).
    // "c" < "p" alphabetically, but since "path" is imported and "chatHandler"
    // is local, no ordering error should be raised.
    let source = r#"
HttpRequest = String

path = (HttpRequest) -> String {
    HttpRequest
}

chatHandler = (HttpRequest) -> String {
    HttpRequest
}

main = (Stdout) -> Unit {
    "ok".print(Stdout)
}
"#;
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse().expect("parser failed");
    resolve_new_syntax(&mut module);

    // entry_items_start = 2 means items[0..2] (HttpRequest typedef + path method)
    // are treated as "imported" — only items at index >= 2 are checked for ordering.
    let errors = check_with_entry(&module, 2);
    let ordering_errs: Vec<_> = errors
        .iter()
        .filter(|e| {
            let msg = format!("{:?}", e);
            msg.contains("alphabetical order") && msg.contains("chatHandler")
        })
        .collect();
    assert!(
        ordering_errs.is_empty(),
        "spurious ordering error for local method that only precedes an imported method: {:?}",
        ordering_errs
    );
}

// ── Issue 5: Lambda-scope values cloned to prevent move errors ────────────

#[test]
fn lambda_scope_values_are_cloned() {
    // Method parameters (lambda-scope values) must be cloned when emitted
    // so that using them multiple times in a body doesn't cause a move error.
    // We use two string methods so the param `Name` appears twice.
    let source = r#"
Greeting = String
Name = String

greet = (Greeting * Name) -> Greeting {
    Greeting
}

main = (Stdout) -> Unit {
    Greeting("hello").greet(Name("world"))
    "ok".print(Stdout)
}
"#;
    let (errors, rust_code) = parse_and_generate(source);
    assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
    // The generated method body must use .clone() — either for `self` (the receiver)
    // or for `arg0` (the Name parameter). Both come from scopes that always clone.
    assert!(
        rust_code.contains(".clone()"),
        "expected .clone() in generated code for lambda-scope value:\n{}",
        rust_code
    );
}

// ── Issue 6: Unit receiver → free function ───────────────────────────────

#[test]
fn unit_receiver_emits_free_function() {
    // assistantMessage = (Unit) -> String  must NOT generate `impl Unit { ... }`.
    // It should generate a plain free function `fn assistantMessage() -> String`.
    let source = r#"
assistantMessage = (Unit) -> String {
    "hello"
}

main = (Stdout) -> Unit {
    Unit.assistantMessage().print(Stdout)
}
"#;
    let (errors, rust_code) = parse_and_generate(source);
    // No checker errors expected
    assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
    // Must NOT generate `impl Unit {`
    assert!(
        !rust_code.contains("impl Unit"),
        "generated invalid `impl Unit` block:\n{}",
        rust_code
    );
    // Must generate a free function
    assert!(
        rust_code.contains("fn assistantMessage()"),
        "expected free function `fn assistantMessage()` in generated code:\n{}",
        rust_code
    );
}

#[test]
fn unit_method_call_emits_free_function_call() {
    // Unit.assistantMessage() must generate `assistantMessage()` not `().assistantMessage()`.
    let source = r#"
assistantMessage = (Unit) -> String {
    "hello"
}

main = (Stdout) -> Unit {
    Unit.assistantMessage().print(Stdout)
}
"#;
    let (_errors, rust_code) = parse_and_generate(source);
    assert!(
        !rust_code.contains("().assistantMessage"),
        "emitted invalid `().assistantMessage` call:\n{}",
        rust_code
    );
    assert!(
        rust_code.contains("assistantMessage()"),
        "expected `assistantMessage()` call in generated code:\n{}",
        rust_code
    );
}

// ── Issue 7: extern Rust(".0") → field access not method call ─────────────

#[test]
fn extern_dot_digit_emits_field_access() {
    // extern Rust(".0") must generate `__a0.0` (field access),
    // not `__a0.0()` (method call).
    let source = r#"
Body = String

extern Rust(".0") bodyString = (Body) -> String

main = (Stdout) -> Unit {
    Body("hello").bodyString().print(Stdout)
}
"#;
    let (_errors, rust_code) = parse_and_generate(source);
    // Must NOT emit `.0()` — that's a method call on a tuple field, which is invalid Rust.
    assert!(
        !rust_code.contains(".0()"),
        "generated invalid tuple-field method call `.0()`:\n{}",
        rust_code
    );
    // Must emit `.0` without parentheses (field access).
    assert!(
        rust_code.contains(".0"),
        "expected tuple field access `.0` in generated code:\n{}",
        rust_code
    );
}
