# `oneway::no_comments`

**Severity:** deny
**Enforced by:** `oneway_lints` (dylint)

Non-doc comments must declare *why* they exist. A comment that just narrates the next line is forbidden; a comment that records something the code can't say is allowed.

A comment passes if **any** of the following is true:

- It's a doc comment (`///`, `//!`, `/** */`, `/*! */`) — those describe a public API contract and ship to docs.rs.
- It starts with one of these uppercase labels followed by `:` —
  - `WHY:` — the rationale for a non-obvious choice
  - `SAFETY:` — invariants required by `unsafe` code (Rust ecosystem convention)
  - `NOTE:` — a non-obvious aside worth keeping
  - `HACK:` — a deliberate shortcut, ideally with a plan to remove
  - `TODO:` — work to do later
  - `FIXME:` — known broken behavior
  - `PERF:` — performance-motivated weirdness
- It contains a link (`http://` or `https://`) — pointer to context too long to embed.
- It contains a ticket reference matching `#\d+` (e.g. `#1234`).

For a run of consecutive `//` lines, the whole group passes if *any* line in the group carries a label, link, or ticket — so multi-line explanations only need to be marked at the top.

## ❌ Bad — narrating comment

```rust
// increment by 1
number.add(1)
```

## ✅ Good — same intent, no comment needed

```rust
number.add_one()
```

## ✅ Good — non-obvious choice, labeled

```rust
// WHY: the upstream API silently truncates above 64KB, see #4521
// so we chunk the buffer here even when it looks redundant.
send_chunked(&buffer);
```

## ✅ Good — safety invariant for `unsafe`

```rust
// SAFETY: `ptr` was returned by `Box::into_raw` above and not aliased.
let value = unsafe { *ptr };
```

## ❌ Bad — case-sensitive labels

```rust
// safety: lowercase doesn't count
let x = unsafe { *ptr };
```

## ✅ Good — link or ticket alone is enough

```rust
// see https://internals.rust-lang.org/t/...
// fixes #1234
```
