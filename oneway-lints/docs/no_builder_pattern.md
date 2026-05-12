# `oneway::no_builder_pattern`

**Severity:** warn
**Enforced by:** `oneway_lints` (dylint) — *not yet implemented*

Prefer struct literal construction over builder patterns. If a struct has too many fields for a comfortable literal, break it into smaller structs.

Builders are a workaround for languages without named arguments or default values. Rust's struct literals already provide both (`Struct { foo: x, ..Default::default() }`), so the indirection just adds API surface and obscures what fields exist.

## ❌ Bad

```rust
let server = ServerBuilder::new()
    .host("localhost")
    .port(8080)
    .max_connections(100)
    .timeout(Duration::from_secs(30))
    .build();
```

## ✅ Good

```rust
let server = ServerConfig {
    host: Host("localhost".into()),
    max_connections: MaxConnections(100),
    port: Port(8080),
    timeout: Timeout(Duration::from_secs(30)),
};
```
