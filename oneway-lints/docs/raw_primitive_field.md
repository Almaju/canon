# `oneway::raw_primitive_field`

**Severity:** warn
**Enforced by:** `oneway_lints` (dylint) — *not yet implemented*

Struct fields should use newtypes instead of raw primitives (`i32`, `i64`, `u64`, `f64`, `String`, `bool`).

Two reasons: code becomes self-documenting (`UserId` reads better than `u64`), and the type system catches accidental swaps between fields of the same primitive (e.g. two `u64` IDs).

## ❌ Bad

```rust
struct Order {
    price: f64,
    quantity: u32,
    user_id: u64,
}
```

## ✅ Good

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
