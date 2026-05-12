# `oneway::unsorted_impl_methods`

**Severity:** deny
**Enforced by:** `oneway_lints` (dylint)

Methods within an `impl` block must appear in this order:

1. **Static methods / constructors** — anything without a `self` receiver
2. **Public methods** — `self` receiver with `pub` (or `pub(crate)`, `pub(super)`, …)
3. **Private methods** — `self` receiver with no visibility modifier

Within each group, methods must be alphabetically sorted.

The grouping reflects how a reader scans a type: first, "how do I make one?" (constructors), then "what can I do with it from outside?" (public), then "implementation detail" (private). The alphabetical pin within each group eliminates ordering debates.

## ❌ Bad — public method appears before constructor

```rust
impl User {
    pub fn name(&self) -> &str { &self.name }
    pub fn new(name: String) -> Self { Self { name } }
}
```

## ❌ Bad — within the public group, methods are not alphabetical

```rust
impl User {
    pub fn new(name: String) -> Self { Self { name } }
    pub fn name(&self) -> &str { &self.name }
    pub fn email(&self) -> &str { &self.email }
}
```

## ✅ Good

```rust
impl User {
    pub fn from_email(email: String) -> Self { /* ... */ }
    pub fn new(name: String) -> Self { /* ... */ }

    pub fn email(&self) -> &str { &self.email }
    pub fn name(&self) -> &str { &self.name }

    fn cache_key(&self) -> String { /* ... */ }
    fn validate(&self) -> bool { /* ... */ }
}
```
