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

#[test]
fn commutative_call_through_canonical_receiver() {
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
    assert!(
        rust_code.contains("impl Greeting"),
        "expected impl Greeting in generated code:\n{}",
        rust_code
    );
    assert!(
        rust_code.contains("fn greet("),
        "expected fn greet in generated code:\n{}",
        rust_code
    );
}

#[test]
fn commutative_call_through_non_canonical_receiver() {
    let source = r#"
Greeting = String
Name = String

greet = (Greeting * Name) -> Greeting {
    Greeting
}

main = (Stdout) -> Unit {
    Name("world").greet(Greeting("hello"))
    "ok".print(Stdout)
}
"#;
    let (errors, rust_code) = parse_and_generate(source);
    assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
    assert!(
        rust_code.contains("impl Greeting"),
        "expected impl Greeting in generated code:\n{}",
        rust_code
    );
}

#[test]
fn commutative_checker_accepts_both_directions() {
    let source = r#"
Greeting = String
Name = String

greet = (Greeting * Name) -> Greeting {
    Greeting
}

main = (Stdout) -> Unit {
    Greeting("hello").greet(Name("world"))
    Name("world").greet(Greeting("hello"))
    "ok".print(Stdout)
}
"#;
    let (errors, _) = parse_and_generate(source);
    assert!(
        errors.is_empty(),
        "checker should accept commutative calls in both directions, got errors: {:?}",
        errors
    );
}

#[test]
fn param_names_resolve_in_method_body() {
    let source = r#"
Greeting = String
Name = String

greet = (Greeting * Name) -> Name {
    Name
}

main = (Stdout) -> Unit {
    Greeting("hello").greet(Name("world"))
    "ok".print(Stdout)
}
"#;
    let (errors, rust_code) = parse_and_generate(source);
    assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
    assert!(
        rust_code.contains("arg0"),
        "expected param name 'arg0' in generated method body:\n{}",
        rust_code
    );
}
