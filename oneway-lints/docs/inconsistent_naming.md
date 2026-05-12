# `oneway::inconsistent_naming`

**Severity:** warn
**Enforced by:** `oneway_lints` (dylint) — *not yet implemented*

Function parameter names should match their type. When a parameter is of type `UserId`, name it `user_id`, not `id` or `uid`.

The function body reads the parameter by name; if the name doesn't echo the type, the reader has to look back at the signature. Companion rule to [`type_derived_naming`](type_derived_naming.md).

## ❌ Bad

```rust
fn find_user(id: UserId, db: &Database) -> Option<User> {
    db.query(id)
}
```

## ✅ Good

```rust
fn find_user(user_id: UserId, database: &Database) -> Option<User> {
    database.query(user_id)
}
```
