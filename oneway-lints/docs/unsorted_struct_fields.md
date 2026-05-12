# `oneway::unsorted_struct_fields`

**Severity:** deny
**Enforced by:** `oneway_lints` (dylint)

Struct fields must be in alphabetical order. Removes bikeshedding over field order, makes diffs cleaner, and lets readers locate a field by binary-searching the source.

## ❌ Bad

```rust
struct User {
    name: String,
    email: String,
    age: u32,
}
```

## ✅ Good

```rust
struct User {
    age: u32,
    email: String,
    name: String,
}
```
