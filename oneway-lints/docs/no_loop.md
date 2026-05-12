# `oneway::no_loop`

**Severity:** deny
**Enforced by:** `oneway_lints` (dylint) — *not yet implemented*

Don't use `loop`, `while`, or `for` with manual iteration. Use iterators and functional combinators instead.

Imperative loops mix *what* (the transformation) with *how* (mutable state, index arithmetic, early exits). Combinator pipelines isolate the transformation, make intermediate types visible, and compose better.

## ❌ Bad

```rust
let mut total = 0;
for item in &items {
    if item.is_active() {
        total += item.price();
    }
}
```

## ✅ Good

```rust
let total: u64 = items
    .iter()
    .filter(|item| item.is_active())
    .map(|item| item.price())
    .sum();
```

## ❌ Bad

```rust
let mut result = Vec::new();
let mut i = 0;
while i < items.len() {
    result.push(items[i].transform());
    i += 1;
}
```

## ✅ Good

```rust
let result: Vec<_> = items.iter().map(|item| item.transform()).collect();
```
