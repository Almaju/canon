# `oneway::no_unwrap`

**Severity:** deny
**Enforced by:** `clippy::unwrap_used` + `clippy::expect_used`

Never use `.unwrap()` or `.expect()` in non-test code. Use `?` or explicit `match`.

`unwrap` is a hidden `panic!`. Each call site is a runtime crash waiting on user input. Surfacing the error in the type system forces callers to handle it.

## ❌ Bad

```rust
fn read_config() -> Config {
    let content = std::fs::read_to_string("config.toml").unwrap();
    toml::from_str(&content).expect("invalid config")
}
```

## ✅ Good

```rust
fn read_config() -> Result<Config, ConfigError> {
    let content = std::fs::read_to_string("config.toml")?;
    Ok(toml::from_str(&content)?)
}
```
