# `oneway::prefer_combinators`

**Severity:** warn
**Enforced by:** `clippy::single_match` + `clippy::manual_map` + `clippy::manual_unwrap_or`

Use `Option`/`Result` combinators instead of `match` for simple transforms. If you're just mapping, filtering, or providing a default, use the combinator.

Combinators name the intent (`map`, `unwrap_or`, `and_then`) instead of spelling out the case analysis. A `match` should be reserved for the cases where the two arms do meaningfully different work.

## ❌ Bad

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

## ✅ Good

```rust
let display_name = user.nickname
    .unwrap_or_else(|| user.name.clone());

let upper = value.map(|s| s.to_uppercase());

let count = result
    .map(|items| items.len())
    .unwrap_or(0);
```
