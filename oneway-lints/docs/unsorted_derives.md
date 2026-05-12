# `oneway::unsorted_derives`

**Severity:** deny
**Enforced by:** `oneway_lints` (dylint)

`#[derive(...)]` attributes must list traits in alphabetical order. The derive list has no semantic ordering, so pin it alphabetically to keep diffs and reviews quiet.

## ❌ Bad

```rust
#[derive(Debug, Clone, Serialize, PartialEq)]
struct User {
    name: Name,
}
```

## ✅ Good

```rust
#[derive(Clone, Debug, PartialEq, Serialize)]
struct User {
    name: Name,
}
```
