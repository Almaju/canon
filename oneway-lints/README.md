# Oneway Lint Rules for Rust

> Enforce the Oneway philosophy in your Rust codebase. These rules steer code toward consistency, clarity, and the "one way to do it" mindset — without fighting Rust's core design.

## Documentation

### `oneway::no_comments`
**Severity:** deny

Non-doc comments are forbidden — line (`//`) and block (`/* */`). Doc comments (`///`, `//!`, `/** */`, `/*! */`) are allowed because they ship to docs.rs and describe a public API contract. If you feel the need to write a narrating comment:

- Rename the binding or function to be more descriptive
- Extract a well-named helper function
- Use a more precise newtype

❌ Bad:
```rust
// Apply 10% discount for premium users
let price = base * 0.9;
```

✅ Good:
```rust
let discounted_price = apply_premium_discount(base_price);
```

✅ Good — doc comment documenting a public API:
```rust
/// The user's account ID, unique across all tenants.
pub struct AccountId(u64);
```

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

Functions must have at most 2 parameters (including `&self`). A function is either:
- `fn name()` — 0 params
- `fn name(&self)` — receiver only
- `fn name(&self, input: T)` — receiver + one input

Anything more should use a struct.

❌ Bad:
```rust
fn send_email(to: &str, from: &str, subject: &str, body: &str) {
    // ...
}
```

✅ Good:
```rust
struct Email {
    body: String,
    from: String,
    subject: String,
    to: String,
}

fn send_email(email: &Email) {
    // ...
}
```

❌ Bad:
```rust
impl Wallet {
    fn transfer(&self, to: &Account, amount: Amount, memo: &str) { ... }
}
```

✅ Good:
```rust
struct Transfer {
    amount: Amount,
    memo: Memo,
    to: AccountId,
}

impl Wallet {
    fn transfer(&self, transfer: Transfer) { ... }
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

## Control Flow

### `oneway::no_loop`
**Severity:** deny

Don't use `loop`, `while`, or `for` with manual iteration. Use iterators and functional combinators instead.

❌ Bad:
```rust
let mut total = 0;
for item in &items {
    if item.is_active() {
        total += item.price();
    }
}
```

✅ Good:
```rust
let total: u64 = items
    .iter()
    .filter(|item| item.is_active())
    .map(|item| item.price())
    .sum();
```

❌ Bad:
```rust
let mut result = Vec::new();
let mut i = 0;
while i < items.len() {
    result.push(items[i].transform());
    i += 1;
}
```

✅ Good:
```rust
let result: Vec<_> = items.iter().map(|item| item.transform()).collect();
```

### `oneway::no_if_else`
**Severity:** warn

Prefer `match` over `if`/`else` chains. Match is more explicit, forces you to handle all cases, and arms can be sorted.

❌ Bad:
```rust
fn classify(n: i32) -> &'static str {
    if n < 0 {
        "negative"
    } else if n == 0 {
        "zero"
    } else {
        "positive"
    }
}
```

✅ Good:
```rust
fn classify(n: i32) -> &'static str {
    match n.cmp(&0) {
        Ordering::Equal => "zero",
        Ordering::Greater => "positive",
        Ordering::Less => "negative",
    }
}
```

❌ Bad:
```rust
fn describe(user: &User) -> String {
    if user.is_admin() {
        format!("Admin: {}", user.name())
    } else if user.is_moderator() {
        format!("Mod: {}", user.name())
    } else {
        format!("User: {}", user.name())
    }
}
```

✅ Good:
```rust
fn describe(user: &User) -> String {
    match user.role() {
        Role::Admin => format!("Admin: {}", user.name()),
        Role::Moderator => format!("Mod: {}", user.name()),
        Role::User => format!("User: {}", user.name()),
    }
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

## Naming

### `oneway::type_derived_naming`
**Severity:** deny

Variable names must be the `snake_case` version of their type name. This eliminates bikeshedding and makes every binding instantly recognizable. When multiple variables of the same type exist, add a descriptive prefix.

❌ Bad:
```rust
let id = UserId(42);
let db = Database::connect();
let u = User::find(id);
```

✅ Good:
```rust
let user_id = UserId(42);
let database = Database::connect();
let user = User::find(user_id);
```

❌ Bad:
```rust
let src = AccountId(1);
let dst = AccountId(2);
```

✅ Good:
```rust
let sender_account_id = AccountId(1);
let receiver_account_id = AccountId(2);
```

### `oneway::inconsistent_naming`
**Severity:** warn

Function parameter names should match their type. When a parameter is of type `UserId`, name it `user_id`, not `id` or `uid`.

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

## One Way to Write It

### `oneway::no_glob_imports`
**Severity:** deny

No wildcard imports. Every imported symbol must be named explicitly.

❌ Bad:
```rust
use std::collections::*;
use crate::models::*;
```

✅ Good:
```rust
use std::collections::HashMap;
use crate::models::User;
```

### `oneway::sorted_derives`
**Severity:** deny

`#[derive(...)]` attributes must list traits in alphabetical order.

❌ Bad:
```rust
#[derive(Debug, Clone, Serialize, PartialEq)]
struct User {
    name: Name,
}
```

✅ Good:
```rust
#[derive(Clone, Debug, PartialEq, Serialize)]
struct User {
    name: Name,
}
```

### `oneway::inline_format_args`
**Severity:** deny

Use inline variable capture in format strings. Don't pass variables as separate arguments.

❌ Bad:
```rust
let message = format!("Hello, {}! You are {} years old.", name, age);
log::info!("Processing order {} for user {}", order_id, user_id);
```

✅ Good:
```rust
let message = format!("Hello, {name}! You are {age} years old.");
log::info!("Processing order {order_id} for user {user_id}");
```

### `oneway::no_turbofish`
**Severity:** deny

Don't use turbofish syntax (`::<>`). Annotate the binding instead — it's easier to read.

❌ Bad:
```rust
let names = users.iter().map(|u| u.name.clone()).collect::<Vec<String>>();
let parsed = "42".parse::<i32>()?;
```

✅ Good:
```rust
let names: Vec<String> = users.iter().map(|u| u.name.clone()).collect();
let parsed: i32 = "42".parse()?;
```

### `oneway::prefer_combinators`
**Severity:** warn

Use `Option`/`Result` combinators instead of `match` for simple transforms. If you're just mapping, filtering, or providing a default, use the combinator.

❌ Bad:
```rust
let display_name = match user.nickname {
    Some(nick) => nick,
    None => user.name.clone(),
};

let upper = match value {
    Some(s) => Some(s.to_uppercase()),
    None => None,
};

let count = match result {
    Ok(items) => items.len(),
    Err(_) => 0,
};
```

✅ Good:
```rust
let display_name = user.nickname
    .unwrap_or_else(|| user.name.clone());

let upper = value.map(|s| s.to_uppercase());

let count = result
    .map(|items| items.len())
    .unwrap_or(0);
```

### `oneway::one_constructor_name`
**Severity:** deny

Constructors must be named `new`. Not `create`, `build`, `init`, `make`, `construct`, or `from_*` (except `From` trait impls).

❌ Bad:
```rust
impl Server {
    fn create(config: ServerConfig) -> Self { ... }
}

impl Database {
    fn init(url: &str) -> Self { ... }
}

impl HttpClient {
    fn build() -> Self { ... }
}
```

✅ Good:
```rust
impl Server {
    fn new(config: ServerConfig) -> Self { ... }
}

impl Database {
    fn new(url: &str) -> Self { ... }
}

impl HttpClient {
    fn new() -> Self { ... }
}
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
| 6 | `too_many_params` | deny | Max 2 params: self + one input |
| 7 | `no_nested_functions` | warn | Extract inner functions to module level |
| 8 | `raw_primitive_field` | warn | Use newtypes for struct fields |
| 9 | `raw_primitive_param` | warn | Use newtypes for function params |
| 10 | `no_unwrap` | deny | No `.unwrap()` / `.expect()` outside tests |
| 11 | `no_panic` | deny | No `panic!` / `todo!` / `unimplemented!` outside tests |
| 12 | `no_loop` | deny | No `loop`/`while`/`for` — use iterators |
| 13 | `no_if_else` | warn | Use `match` instead of `if`/`else` chains |
| 14 | `prefer_functional_iteration` | warn | Use `.iter().map().filter()` over manual loops |
| 15 | `no_explicit_return` | warn | Last expression is the return value |
| 16 | `no_early_return_in_match` | warn | Let match be the return expression |
| 17 | `no_builder_pattern` | warn | Use struct literals, not builders |
| 18 | `type_derived_naming` | deny | Variable name must be snake_case of its type |
| 19 | `inconsistent_naming` | warn | Param names should match their type |
| 20 | `one_public_type_per_file` | warn | One primary pub type per file |
| 21 | `no_glob_imports` | deny | No `use foo::*` — name every import |
| 22 | `sorted_derives` | deny | `#[derive()]` traits in alphabetical order |
| 23 | `inline_format_args` | deny | `format!("{x}")` not `format!("{}", x)` |
| 24 | `no_turbofish` | deny | Annotate the binding, not the call site |
| 25 | `prefer_combinators` | warn | `.map()` / `.unwrap_or()` over `match` on Option/Result |
| 26 | `one_constructor_name` | deny | Constructors must be called `new` |
| 27 | `no_comments` | deny | No `//` or `/* */` — only doc comments (`///`, `//!`, `/** */`, `/*! */`) are allowed |

---

*These lints are inspired by the [Oneway language design](DESIGN.md). They can be implemented as [dylint](https://github.com/trailofbits/dylint) rules or as a custom Clippy lint set.*
