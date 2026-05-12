# `oneway::no_turbofish`

**Severity:** deny
**Enforced by:** `oneway_lints` (dylint) — *not yet implemented*

Don't use turbofish syntax (`::<>`). Annotate the binding instead — it's easier to read and easier to skim.

A turbofish hides the resulting type inside an expression; a binding annotation puts it where the reader expects to find type information: next to the name.

## ❌ Bad

```rust
let names = users.iter().map(|u| u.name.clone()).collect::<Vec<String>>();
let parsed = "42".parse::<i32>()?;
```

## ✅ Good

```rust
let names: Vec<String> = users.iter().map(|u| u.name.clone()).collect();
let parsed: i32 = "42".parse()?;
```
