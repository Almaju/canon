# `oneway::unsorted_match_arms`

**Severity:** deny
**Enforced by:** `oneway_lints` (dylint)

Match arms must be sorted by pattern text. The wildcard `_` arm must always be last. Same reasoning as struct fields and enum variants: arm order has no semantic effect, so pin it to one canonical order.

## ❌ Bad

```rust
match color {
    Color::Red => "red",
    Color::Blue => "blue",
    Color::Green => "green",
}
```

## ✅ Good

```rust
match color {
    Color::Blue => "blue",
    Color::Green => "green",
    Color::Red => "red",
}
```
