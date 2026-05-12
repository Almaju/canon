# `oneway::one_public_type_per_file`

**Severity:** warn
**Enforced by:** `oneway_lints` (dylint) — *not yet implemented*

Each file should export at most one primary public type (struct/enum). Related types (newtypes, error types) are fine as supporting cast.

This forces module boundaries to follow the type system. Finding a type becomes a matter of finding a filename, and grep stays useful.

## ❌ Bad — three unrelated types in one file

```rust
pub struct User { ... }
pub struct Order { ... }
pub struct Product { ... }
```

## ✅ Good — split by primary type, supporting newtypes live with their owner

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
