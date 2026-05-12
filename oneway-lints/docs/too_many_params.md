# `oneway::too_many_params`

**Severity:** deny
**Enforced by:** `clippy::too_many_arguments` (configured via `clippy.toml`: `too-many-arguments-threshold = 2`)

Functions must have at most 2 parameters (including `&self`). The shape of a function is one of:

- `fn name()` — 0 params
- `fn name(input: T)` or `fn name(&self)` — single value
- `fn name(&self, input: T)` — receiver + one input

Anything more must be packed into a struct. Long argument lists invite call-site bugs (swapping two parameters of the same type) and signal that the function is doing too much.

## ❌ Bad

```rust
fn send_email(to: &str, from: &str, subject: &str, body: &str) {
    // ...
}
```

## ✅ Good

```rust
struct Email {
    body: String,
    from: String,
    subject: String,
    to: String,
}

fn send_email(email: &Email) {
    // ...
}
```

## ❌ Bad — methods too

```rust
impl Wallet {
    fn transfer(&self, to: &Account, amount: Amount, memo: &str) { ... }
}
```

## ✅ Good

```rust
struct Transfer {
    amount: Amount,
    memo: Memo,
    to: AccountId,
}

impl Wallet {
    fn transfer(&self, transfer: Transfer) { ... }
}
```
