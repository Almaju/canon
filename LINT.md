# Oneway Lint Rules for Rust

> Enforce the Oneway philosophy in your Rust codebase. These rules steer code toward consistency, clarity, and the "one way to do it" mindset — without fighting Rust's core design.

## Sorting

### `oneway::unsorted_struct_fields`
**Severity:** deny

Struct fields must be in alphabetical order.

❌ Bad:
```rust
struct User {
    name: String,
    email: String,
    age: u32,
}
```

✅ Good:
```rust
struct User {
    age: u32,
    email: String,
    name: String,
}
```

### `oneway::unsorted_enum_variants`
**Severity:** deny

Enum variants must be in alphabetical order.

❌ Bad:
```rust
enum Color {
    Red,
    Blue,
    Green,
}
```

✅ Good:
```rust
enum Color {
    Blue,
    Green,
    Red,
}
```

### `oneway::unsorted_match_arms`
**Severity:** deny

Match arms must be sorted by pattern text. Wildcard `_` must always be last.

❌ Bad:
```rust
match color {
    Color::Red => "red",
    Color::Blue => "blue",
    Color::Green => "green",
}
```

✅ Good:
```rust
match color {
    Color::Blue => "blue",
    Color::Green => "green",
    Color::Red => "red",
}
```

### `oneway::unsorted_imports`
**Severity:** deny

`use` statements must be in alphabetical order within each group.

❌ Bad:
```rust
use std::io;
use std::collections::HashMap;
use std::fmt;
```

✅ Good:
```rust
use std::collections::HashMap;
use std::fmt;
use std::io;
```

### `oneway::unsorted_impl_methods`
**Severity:** deny

Methods within an `impl` block must be alphabetically sorted.

❌ Bad:
```rust
impl User {
    fn name(&self) -> &str { &self.name }
    fn age(&self) -> u32 { self.age }
    fn email(&self) -> &str { &self.email }
}
```

✅ Good:
```rust
impl User {
    fn age(&self) -> u32 { self.age }
    fn email(&self) -> &str { &self.email }
    fn name(&self) -> &str { &self.name }
}
```

## Function Discipline

### `oneway::too_many_params`
**Severity:** deny

Functions must have at most 3 parameters (including `&self`/`&mut self`). Use a struct for more.

❌ Bad:
```rust
fn send_email(to: &str, from: &str, subject: &str, body: &str, cc: &[&str]) {
    // ...
}
```

✅ Good:
```rust
struct Email {
    body: String,
    cc: Vec<String>,
    from: String,
    subject: String,
    to: String,
}

fn send_email(email: &Email) {
    // ...
}
```

### `oneway::no_nested_functions`
**Severity:** warn

Don't define functions inside other functions. Extract them to module level.

❌ Bad:
```rust
fn process(items: &[Item]) -> Vec<Result> {
    fn transform(item: &Item) -> Result {
        // ...
    }
    items.iter().map(transform).collect()
}
```

✅ Good:
```rust
fn transform(item: &Item) -> Result {
    // ...
}

fn process(items: &[Item]) -> Vec<Result> {
    items.iter().map(transform).collect()
}
```

## Newtype Discipline

### `oneway::raw_primitive_field`
**Severity:** warn

Struct fields should use newtypes instead of raw primitives (`i32`, `i64`, `u64`, `f64`, `String`, `bool`). This makes code self-documenting and prevents mixing up fields of the same type.

❌ Bad:
```rust
struct Order {
    price: f64,
    quantity: u32,
    user_id: u64,
}
```

✅ Good:
```rust
struct Price(f64);
struct Quantity(u32);
struct UserId(u64);

struct Order {
    price: Price,
    quantity: Quantity,
    user_id: UserId,
}
```

### `oneway::raw_primitive_param`
**Severity:** warn

Function parameters should use newtypes instead of raw primitives. This prevents accidentally swapping arguments of the same type.

❌ Bad:
```rust
fn transfer(from: u64, to: u64, amount: f64) {
    // Easy to accidentally swap `from` and `to`
}
```

✅ Good:
```rust
fn transfer(from: AccountId, to: AccountId, amount: Amount) {
    // Types prevent misuse
}
```

## Error Handling

### `oneway::no_unwrap`
**Severity:** deny

Never use `.unwrap()` or `.expect()` in non-test code. Use `?` or explicit `match`.

❌ Bad:
```rust
fn read_config() -> Config {
    let content = std::fs::read_to_string("config.toml").unwrap();
    toml::from_str(&content).expect("invalid config")
}
```

✅ Good:
```rust
fn read_config() -> Result<Config, ConfigError> {
    let content = std::fs::read_to_string("config.toml")?;
    Ok(toml::from_str(&content)?)
}
```

### `oneway::no_panic`
**Severity:** deny

Never use `panic!`, `todo!`, `unimplemented!`, or `unreachable!` in non-test code. Return `Result` or handle the case.

❌ Bad:
```rust
fn divide(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        panic!("division by zero");
    }
    a / b
}
```

✅ Good:
```rust
fn divide(a: f64, b: f64) -> Result<f64, DivisionError> {
    match b == 0.0 {
        true => Err(DivisionError::DivideByZero),
        false => Ok(a / b),
    }
}
```

## Immutability

### `oneway::prefer_immutable`
**Severity:** warn

Prefer immutable bindings. Flag `let mut` when the variable could be refactored to avoid mutation (e.g., using iterators, `map`, `fold` instead of a mutable accumulator).

❌ Bad:
```rust
fn sum_positives(numbers: &[i64]) -> i64 {
    let mut total = 0;
    for n in numbers {
        if *n > 0 {
            total += n;
        }
    }
    total
}
```

✅ Good:
```rust
fn sum_positives(numbers: &[i64]) -> i64 {
    numbers.iter().filter(|n| **n > 0).sum()
}
```

### `oneway::no_mut_param`
**Severity:** warn

Avoid `&mut` parameters when you can return a new value instead. Prefer functional transformation over in-place mutation.

❌ Bad:
```rust
fn normalize_name(name: &mut String) {
    *name = name.trim().to_lowercase();
}
```

✅ Good:
```rust
fn normalize_name(name: &str) -> String {
    name.trim().to_lowercase()
}
```

## Iteration Style

### `oneway::prefer_functional_iteration`
**Severity:** warn

Prefer `.iter().map().filter().collect()` over manual `for` loops with `push`. If the loop body is just building a collection, use functional style.

❌ Bad:
```rust
fn get_adult_names(users: &[User]) -> Vec<String> {
    let mut names = Vec::new();
    for user in users {
        if user.age >= 18 {
            names.push(user.name.clone());
        }
    }
    names
}
```

✅ Good:
```rust
fn get_adult_names(users: &[User]) -> Vec<String> {
    users
        .iter()
        .filter(|u| u.age >= 18)
        .map(|u| u.name.clone())
        .collect()
}
```

## Return Style

### `oneway::no_explicit_return`
**Severity:** warn

Don't use the `return` keyword when the last expression in the block serves the same purpose.

❌ Bad:
```rust
fn is_valid(age: u32) -> bool {
    if age >= 18 && age <= 120 {
        return true;
    }
    return false;
}
```

✅ Good:
```rust
fn is_valid(age: u32) -> bool {
    age >= 18 && age <= 120
}
```

### `oneway::no_early_return_in_match`
**Severity:** warn

Don't use `return` inside match arms. Let the match expression be the return value.

❌ Bad:
```rust
fn describe(n: i32) -> &'static str {
    match n.cmp(&0) {
        Ordering::Less => return "negative",
        Ordering::Equal => return "zero",
        Ordering::Greater => return "positive",
    }
}
```

✅ Good:
```rust
fn describe(n: i32) -> &'static str {
    match n.cmp(&0) {
        Ordering::Equal => "zero",
        Ordering::Greater => "positive",
        Ordering::Less => "negative",
    }
}
```

Note: the arms are also alphabetically sorted.

## Struct Construction

### `oneway::no_builder_pattern`
**Severity:** warn

Prefer struct literal construction over builder patterns. If a struct has too many fields for a comfortable literal, break it into smaller structs.

❌ Bad:
```rust
let server = ServerBuilder::new()
    .host("localhost")
    .port(8080)
    .max_connections(100)
    .timeout(Duration::from_secs(30))
    .build();
```

✅ Good:
```rust
let server = ServerConfig {
    host: Host("localhost".into()),
    max_connections: MaxConnections(100),
    port: Port(8080),
    timeout: Timeout(Duration::from_secs(30)),
};
```

### `oneway::no_default_trait`
**Severity:** warn

Avoid `impl Default` — it hides what values are being set. Be explicit with struct literals.

❌ Bad:
```rust
let config = Config {
    port: 3000,
    ..Default::default()
};
```

✅ Good:
```rust
let config = Config {
    host: Host("127.0.0.1".into()),
    port: Port(3000),
    workers: Workers(4),
};
```

## Naming

### `oneway::inconsistent_naming`
**Severity:** warn

Variable names should clearly reflect their type. When a variable holds a value of type `UserId`, name it `user_id`, not `id` or `uid` or `x`.

❌ Bad:
```rust
fn find_user(id: UserId, db: &Database) -> Option<User> {
    db.query(id)
}
```

✅ Good:
```rust
fn find_user(user_id: UserId, database: &Database) -> Option<User> {
    database.query(user_id)
}
```

## Module Organization

### `oneway::one_public_type_per_file`
**Severity:** warn

Each file should export at most one primary public type (struct/enum). Related types (newtypes, error types) are fine as supporting cast.

❌ Bad (in a single file):
```rust
pub struct User { ... }
pub struct Order { ... }
pub struct Product { ... }
```

✅ Good:
```
// user.rs
pub struct User { ... }
pub struct UserId(u64);

// order.rs
pub struct Order { ... }
pub struct OrderId(u64);

// product.rs
pub struct Product { ... }
pub struct ProductId(u64);
```

---

## Summary

| # | Lint | Severity | One-liner |
|---|------|----------|-----------|
| 1 | `unsorted_struct_fields` | deny | Struct fields must be alphabetical |
| 2 | `unsorted_enum_variants` | deny | Enum variants must be alphabetical |
| 3 | `unsorted_match_arms` | deny | Match arms sorted, `_` last |
| 4 | `unsorted_imports` | deny | `use` statements alphabetical |
| 5 | `unsorted_impl_methods` | deny | Methods in `impl` alphabetical |
| 6 | `too_many_params` | deny | Max 3 params per function |
| 7 | `no_nested_functions` | warn | Extract inner functions to module level |
| 8 | `raw_primitive_field` | warn | Use newtypes for struct fields |
| 9 | `raw_primitive_param` | warn | Use newtypes for function params |
| 10 | `no_unwrap` | deny | No `.unwrap()` / `.expect()` outside tests |
| 11 | `no_panic` | deny | No `panic!` / `todo!` / `unimplemented!` outside tests |
| 12 | `prefer_immutable` | warn | Avoid `let mut` when functional style works |
| 13 | `no_mut_param` | warn | Return new values instead of `&mut` params |
| 14 | `prefer_functional_iteration` | warn | Use `.iter().map().filter()` over `for` + `push` |
| 15 | `no_explicit_return` | warn | Last expression is the return value |
| 16 | `no_early_return_in_match` | warn | Let match be the return expression |
| 17 | `no_builder_pattern` | warn | Use struct literals, not builders |
| 18 | `no_default_trait` | warn | Be explicit, don't hide values behind `Default` |
| 19 | `inconsistent_naming` | warn | Variable names should reflect their type |
| 20 | `one_public_type_per_file` | warn | One primary pub type per file |

---

*These lints are inspired by the [Oneway language design](DESIGN.md). They can be implemented as [dylint](https://github.com/trailofbits/dylint) rules or as a custom Clippy lint set.*
