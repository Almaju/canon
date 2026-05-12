# `oneway::inline_format_args`

**Severity:** deny
**Enforced by:** `clippy::uninlined_format_args`

Use inline variable capture in format strings. Don't pass variables as separate arguments when the captured form works.

Inline captures keep the value adjacent to its placeholder — you can read the format string left-to-right and see exactly what each slot contains, instead of counting commas.

## ❌ Bad

```rust
let message = format!("Hello, {}! You are {} years old.", name, age);
log::info!("Processing order {} for user {}", order_id, user_id);
```

## ✅ Good

```rust
let message = format!("Hello, {name}! You are {age} years old.");
log::info!("Processing order {order_id} for user {user_id}");
```
