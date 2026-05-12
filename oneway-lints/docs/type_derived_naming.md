# `oneway::type_derived_naming`

**Severity:** deny
**Enforced by:** `oneway_lints` (dylint) — *not yet implemented*

Variable names must be the `snake_case` version of their type name. This eliminates bikeshedding and makes every binding instantly recognizable at the call site. When multiple variables of the same type coexist, add a descriptive prefix.

## ❌ Bad

```rust
let id = UserId(42);
let db = Database::connect();
let u = User::find(id);
```

## ✅ Good

```rust
let user_id = UserId(42);
let database = Database::connect();
let user = User::find(user_id);
```

## ❌ Bad — two of the same type without disambiguation

```rust
let src = AccountId(1);
let dst = AccountId(2);
```

## ✅ Good

```rust
let sender_account_id = AccountId(1);
let receiver_account_id = AccountId(2);
```
