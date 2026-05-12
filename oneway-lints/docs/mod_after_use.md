# `oneway::mod_after_use`

**Severity:** deny
**Enforced by:** `oneway_lints` (dylint)

Every `mod` declaration in a module must appear before any `use` statement. `cargo fmt` already orders `use` statements alphabetically, but it does not enforce the mod/use split — this lint does.

The reason for the split: `mod` declarations define what *exists* in the crate; `use` statements bring symbols into scope. Reading top-to-bottom, structure should come before consumption.

## ❌ Bad

```rust
use std::collections::HashMap;

mod parser;

use std::collections::BTreeMap;

mod printer;
```

## ✅ Good

```rust
mod parser;
mod printer;

use std::collections::BTreeMap;
use std::collections::HashMap;
```
