# `oneway::no_glob_imports`

**Severity:** deny
**Enforced by:** `clippy::wildcard_imports`

No wildcard imports. Every imported symbol must be named explicitly.

`use foo::*` makes the symbol soup at the top of a file invisible to grep and unstable across upstream changes — adding a new public item upstream can silently shadow a local binding.

## ❌ Bad

```rust
use std::collections::*;
use crate::models::*;
```

## ✅ Good

```rust
use std::collections::HashMap;
use crate::models::User;
```
