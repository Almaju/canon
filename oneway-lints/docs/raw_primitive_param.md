# `oneway::raw_primitive_param`

**Severity:** warn
**Enforced by:** `oneway_lints` (dylint) — *not yet implemented*

Function parameters should use newtypes instead of raw primitives. This prevents accidentally swapping arguments of the same primitive type at the call site.

## ❌ Bad

```rust
fn transfer(from: u64, to: u64, amount: f64) {
    // Easy to accidentally swap `from` and `to`
}
```

## ✅ Good

```rust
fn transfer(from: AccountId, to: AccountId, amount: Amount) {
    // Types prevent misuse
}
```
