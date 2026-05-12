# `oneway::no_early_return_in_match`

**Severity:** warn
**Enforced by:** `oneway_lints` (dylint) — *not yet implemented*

Don't use `return` inside match arms. Let the match expression itself be the return value.

A `return` inside every arm is a sign that the match is being used for control flow when it should be used as an expression.

## ❌ Bad

```rust
fn describe(n: i32) -> &'static str {
    match n.cmp(&0) {
        Ordering::Less => return "negative",
        Ordering::Equal => return "zero",
        Ordering::Greater => return "positive",
    }
}
```

## ✅ Good

```rust
fn describe(n: i32) -> &'static str {
    match n.cmp(&0) {
        Ordering::Equal => "zero",
        Ordering::Greater => "positive",
        Ordering::Less => "negative",
    }
}
```

Note: the arms are also alphabetically sorted (see [`unsorted_match_arms`](unsorted_match_arms.md)).
