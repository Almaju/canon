# `oneway::no_explicit_return`

**Severity:** warn
**Enforced by:** `clippy::needless_return`

Don't use the `return` keyword when the last expression in the block serves the same purpose. Rust is expression-oriented — let the expression carry the value.

## ❌ Bad

```rust
fn is_valid(age: u32) -> bool {
    if age >= 18 && age <= 120 {
        return true;
    }
    return false;
}
```

## ✅ Good

```rust
fn is_valid(age: u32) -> bool {
    age >= 18 && age <= 120
}
```
