# `oneway::unsorted_enum_variants`

**Severity:** deny
**Enforced by:** `oneway_lints` (dylint)

Enum variants must be in alphabetical order. The order in which variants are declared has no semantic meaning, so freezing it alphabetically prevents arbitrary churn.

## ❌ Bad

```rust
enum Color {
    Red,
    Blue,
    Green,
}
```

## ✅ Good

```rust
enum Color {
    Blue,
    Green,
    Red,
}
```
