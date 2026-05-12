# `oneway::prefer_functional_iteration`

**Severity:** warn
**Enforced by:** `clippy::manual_filter_map` (partial) + `oneway_lints` (dylint, planned)

Prefer `.iter().map().filter().collect()` over manual `for` loops with `push`. If the loop body is just building a collection, use functional style.

This is a more specific case of [`no_loop`](no_loop.md): the "build a collection from another collection" pattern is the most common reason to reach for a loop, and the most mechanical to rewrite.

## ❌ Bad

```rust
fn get_adult_names(users: &[User]) -> Vec<String> {
    let mut names = Vec::new();
    for user in users {
        if user.age >= 18 {
            names.push(user.name.clone());
        }
    }
    names
}
```

## ✅ Good

```rust
fn get_adult_names(users: &[User]) -> Vec<String> {
    users
        .iter()
        .filter(|u| u.age >= 18)
        .map(|u| u.name.clone())
        .collect()
}
```
