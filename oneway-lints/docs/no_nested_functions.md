# `oneway::no_nested_functions`

**Severity:** warn
**Enforced by:** `oneway_lints` (dylint) — *not yet implemented*

Don't define functions inside other functions. Extract them to module level.

Nested functions can't capture state (use a closure if you need that), they hide from the outline view, and they make the outer function look longer than it is. If the helper is worth naming, it's worth lifting out.

## ❌ Bad

```rust
fn process(items: &[Item]) -> Vec<Result> {
    fn transform(item: &Item) -> Result {
        // ...
    }
    items.iter().map(transform).collect()
}
```

## ✅ Good

```rust
fn transform(item: &Item) -> Result {
    // ...
}

fn process(items: &[Item]) -> Vec<Result> {
    items.iter().map(transform).collect()
}
```
