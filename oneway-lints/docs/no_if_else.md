# `oneway::no_if_else`

**Severity:** warn
**Enforced by:** `oneway_lints` (dylint) — *not yet implemented*

Prefer `match` over `if`/`else` chains. Match is more explicit, forces you to handle all cases (exhaustiveness checking), and the arms can be sorted ([`unsorted_match_arms`](unsorted_match_arms.md)).

`if`/`else` chains tend to grow without anyone noticing. A `match` against an enum or `Ordering` makes "did you cover everything?" a compiler error.

## ❌ Bad

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

## ✅ Good

```rust
fn classify(n: i32) -> &'static str {
    match n.cmp(&0) {
        Ordering::Equal => "zero",
        Ordering::Greater => "positive",
        Ordering::Less => "negative",
    }
}
```

## ❌ Bad

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

## ✅ Good

```rust
fn describe(user: &User) -> String {
    match user.role() {
        Role::Admin => format!("Admin: {}", user.name()),
        Role::Moderator => format!("Mod: {}", user.name()),
        Role::User => format!("User: {}", user.name()),
    }
}
```
