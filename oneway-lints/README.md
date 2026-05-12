# Oneway Lint Rules for Rust

> Enforce the Oneway philosophy in your Rust codebase. These rules steer code toward consistency, clarity, and the "one way to do it" mindset — without fighting Rust's core design.

Each rule has a dedicated page under [`docs/`](docs/) with rationale and bad/good examples.

## Quick start

The easiest way to use this lint set is via the [`cargo-oneway`](../cargo-oneway/) wrapper — one command that runs rustfmt, clippy (with the Oneway config), and `oneway-lints` (via dylint):

```sh
cargo install cargo-dylint dylint-link
cargo install cargo-oneway --git https://github.com/Almaju/oneway
cargo oneway          # check formatting + run both lint passes
cargo oneway fmt      # apply formatting
cargo oneway lint     # lint only
```

If you want to drive `dylint` directly:

```sh
cargo dylint --git https://github.com/Almaju/oneway --pattern oneway-lints --lib oneway_lints
```

## How rules are enforced

The Oneway lint suite is a partnership between three tools:

| Tool | What it covers |
|------|----------------|
| **dylint** (`oneway_lints`, this crate) | Custom rules with no upstream equivalent — sorting, comment policy, etc. |
| **clippy** | Rules that clippy already implements; we just enable them with the right severity and (where relevant) threshold |
| **rustfmt** | Whitespace, line wrapping, `use` ordering — fully delegated |

The "Tool" column on every rule below tells you which one fires. `cargo oneway` runs them all in sequence.

---

## Sorting

| Lint | Severity | Tool | One-liner |
|------|----------|------|-----------|
| [`unsorted_struct_fields`](docs/unsorted_struct_fields.md) | deny | dylint | Struct fields must be alphabetical |
| [`unsorted_enum_variants`](docs/unsorted_enum_variants.md) | deny | dylint | Enum variants must be alphabetical |
| [`unsorted_match_arms`](docs/unsorted_match_arms.md) | deny | dylint | Match arms sorted, `_` last |
| [`mod_after_use`](docs/mod_after_use.md) | deny | dylint | `mod` declarations must come before `use` statements |
| [`unsorted_impl_methods`](docs/unsorted_impl_methods.md) | deny | dylint | `impl` methods grouped (static, public, private), alphabetical within group |
| [`unsorted_derives`](docs/unsorted_derives.md) | deny | dylint | `#[derive()]` traits in alphabetical order |

## Function Discipline

| Lint | Severity | Tool | One-liner |
|------|----------|------|-----------|
| [`too_many_params`](docs/too_many_params.md) | deny | `clippy::too_many_arguments` (threshold = 2) | Max 2 params: self + one input |
| [`no_nested_functions`](docs/no_nested_functions.md) | warn | dylint (planned) | Extract inner functions to module level |
| [`one_constructor_name`](docs/one_constructor_name.md) | deny | dylint (planned) | Constructors must be called `new` |

## Newtype Discipline

| Lint | Severity | Tool | One-liner |
|------|----------|------|-----------|
| [`raw_primitive_field`](docs/raw_primitive_field.md) | warn | dylint (planned) | Use newtypes for struct fields |
| [`raw_primitive_param`](docs/raw_primitive_param.md) | warn | dylint (planned) | Use newtypes for function params |

## Error Handling

| Lint | Severity | Tool | One-liner |
|------|----------|------|-----------|
| [`no_unwrap`](docs/no_unwrap.md) | deny | `clippy::unwrap_used` + `clippy::expect_used` | No `.unwrap()` / `.expect()` outside tests |
| [`no_panic`](docs/no_panic.md) | deny | `clippy::panic` + `todo` + `unimplemented` + `unreachable` | No `panic!` / `todo!` / `unimplemented!` outside tests |

## Control Flow

| Lint | Severity | Tool | One-liner |
|------|----------|------|-----------|
| [`no_loop`](docs/no_loop.md) | deny | dylint (planned) | No `loop`/`while`/`for` — use iterators |
| [`no_if_else`](docs/no_if_else.md) | warn | dylint (planned) | Use `match` instead of `if`/`else` chains |
| [`prefer_functional_iteration`](docs/prefer_functional_iteration.md) | warn | `clippy::manual_filter_map` (partial) + dylint (planned) | Use `.iter().map().filter()` over manual loops |

## Return Style

| Lint | Severity | Tool | One-liner |
|------|----------|------|-----------|
| [`no_explicit_return`](docs/no_explicit_return.md) | warn | `clippy::needless_return` | Last expression is the return value |
| [`no_early_return_in_match`](docs/no_early_return_in_match.md) | warn | dylint (planned) | Let match be the return expression |

## Construction & Naming

| Lint | Severity | Tool | One-liner |
|------|----------|------|-----------|
| [`no_builder_pattern`](docs/no_builder_pattern.md) | warn | dylint (planned) | Use struct literals, not builders |
| [`type_derived_naming`](docs/type_derived_naming.md) | deny | dylint (planned) | Variable name must be snake_case of its type |
| [`inconsistent_naming`](docs/inconsistent_naming.md) | warn | dylint (planned) | Param names should match their type |

## Module Organization

| Lint | Severity | Tool | One-liner |
|------|----------|------|-----------|
| [`one_public_type_per_file`](docs/one_public_type_per_file.md) | warn | dylint (planned) | One primary pub type per file |

## Imports & Style

| Lint | Severity | Tool | One-liner |
|------|----------|------|-----------|
| [`no_glob_imports`](docs/no_glob_imports.md) | deny | `clippy::wildcard_imports` | No `use foo::*` — name every import |
| [`inline_format_args`](docs/inline_format_args.md) | deny | `clippy::uninlined_format_args` | `format!("{x}")` not `format!("{}", x)` |
| [`no_turbofish`](docs/no_turbofish.md) | deny | dylint (planned) | Annotate the binding, not the call site |
| [`prefer_combinators`](docs/prefer_combinators.md) | warn | `clippy::single_match` + `manual_map` + `manual_unwrap_or` | `.map()` / `.unwrap_or()` over `match` on Option/Result |

## Documentation

| Lint | Severity | Tool | One-liner |
|------|----------|------|-----------|
| [`no_comments`](docs/no_comments.md) | deny | dylint | Non-doc comments must carry a label (`SAFETY:`, `TODO:`, …), link, or `#1234` ticket ref |

---

*These lints are inspired by the [Oneway language design](../DESIGN.md). The dylint-implemented rules live in this crate; clippy and rustfmt rules are configured in [`../cargo-oneway/`](../cargo-oneway/).*
