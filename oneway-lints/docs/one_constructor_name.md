# `oneway::one_constructor_name`

**Severity:** deny
**Enforced by:** `oneway_lints` (dylint) — *not yet implemented*

Constructors must be named `new`. Not `create`, `build`, `init`, `make`, `construct`, or `from_*` (except `From` trait impls).

A single canonical constructor name means readers and IDEs never have to guess. Variant constructors (`from_str`, `from_bytes`) are fine when there's a real `From<T>` semantic — what's banned is using `from_*` as a synonym for `new`.

## ❌ Bad

```rust
impl Server {
    fn create(config: ServerConfig) -> Self { ... }
}

impl Database {
    fn init(url: &str) -> Self { ... }
}

impl HttpClient {
    fn build() -> Self { ... }
}
```

## ✅ Good

```rust
impl Server {
    fn new(config: ServerConfig) -> Self { ... }
}

impl Database {
    fn new(url: &str) -> Self { ... }
}

impl HttpClient {
    fn new() -> Self { ... }
}
```
