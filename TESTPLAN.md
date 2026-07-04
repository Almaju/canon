# Canon Test Plan — Coverage Matrix

Status: **living document** (V1 M4). Maps every language construct and
stdlib surface to the test that pins it, and names the holes
explicitly. The test-layer taxonomy and how-to-add instructions live
in `CLAUDE.md` §Testing; this file is the *what's covered* view.

Conventions: `runtime/x` = `tests/runtime/x.can` (+ `.stdout` golden),
`ok/x` / `fail/x` = `tests/checker/{ok,fail}/x.can`, `canon/x` =
`tests/canon/x_test.can`, bare `x.rs` = `tests/x.rs`.

## Language core

| Construct | Positive coverage | Negative coverage |
|---|---|---|
| Literals (int, float, hex, string, escapes) | `runtime/literals`, `runtime/arithmetic`, `runtime/float_print` | `fail/string_invalid_escape` |
| Explicit primitive constructors `Int(1)` / `Float(x)` / `String(s)` | `runtime/int_float_explicit_constructors` | — |
| Int arithmetic + comparison chains | `runtime/arithmetic`, `canon/arithmetic` | — |
| Float arithmetic, print rendering, NaN/Inf | `runtime/float_print` | — |
| Bool composition (`and`/`or`/`not`), bool print | `runtime/bool_chains` | — |
| String primitives (`length`, `byteAt`, `substring`, `eq`, `concat`) | `runtime/string_primitives`, `runtime/string_concat`, `canon/string` | — |
| Newtypes (declare, construct, `.Underlying` unwrap) | `runtime/newtype_unwrap`, `runtime/field-access` | `fail/unknown_newtype_field` |
| Products (construct, field access, nested constructors) | `runtime/product_three_field`, `runtime/product_nested_constructor` | `fail/unknown_field_access` |
| Unions + dispatch (2-variant) | `runtime/match`, `runtime/variant_payload_extraction` | `fail/dispatch_arm_wrong_variant`, `fail/wrong_arm_pattern`, `fail/union_variants_out_of_order` |
| Unions + dispatch (3+, mixed payload types incl. Float) | `runtime/three_variant_dispatch`, `runtime/union_four_variants_mixed` | — |
| Dispatch through newtype-wrapped unions | `runtime/dispatch_through_newtype`, `ok/dispatch_through_newtype_option` | — |
| Arm payload binding (incl. after `concat` in arm body) | `runtime/variant_payload_extraction`, `runtime/arm_payload_after_concat` | — |
| Option construct/extract (Int and String payloads) | `runtime/option`, `runtime/option_string_payload`, `runtime/option_with_string` | — |
| Result + `?` (payload extraction, Err short-circuit) | `runtime/result_with_string`, `runtime/try_short_circuit` | — |
| Lists (literal, `length`, `first`, `get`, `map` incl. cross-type) | `runtime/list`, `runtime/list_of_strings`, `runtime/list_map_get`, `runtime/list_map_strings`, `canon/list` | — |
| Free functions, commutative method-call dispatch | `runtime/commutative`, `canon/commutative` | `fail/free_function_ordering_violation` |
| Methods on receivers + alias-chain lookup | `runtime/methods`, `runtime/tojson_newtype_receiver` | `fail/method_ordering_violation`, `fail/unknown_method`, `fail/unknown_receiver_type` |
| Recursion / loops-by-recursion | `runtime/loop`, `runtime/tree`, `canon/recursive_type` | — |
| Traits (function-typed type defs) | `runtime/traits`, `canon/traits` | — |
| Entry-point rules (main / HTTP entry / mixed / ambiguous) | `ok/wasi_http_handler` | `fail/missing_main`, `fail/duplicate_main`, `fail/mixed_worlds_main_and_handler`, `fail/ambiguous_http_entry` |
| Alphabetical ordering (types, functions, variants, arms) | (implicit in every `ok/` fixture) | `fail/type_definition_ordering_violation`, `fail/free_function_ordering_violation`, `fail/method_ordering_violation`, `fail/union_variants_out_of_order` |
| Capabilities | `runtime/hello` (Stdout) | `fail/conjured_capability` |
| Async externs + auto-await | `runtime/async_echo`, `runtime/async_slow_echo` | — |
| `parallel` / `race` combinators | `runtime/parallel_two_echoes`, `runtime/race_two_echoes` | — |

## Stdlib & WASI surface

| Surface | Coverage |
|---|---|
| JSON (literals, parse, roundtrip, field extract, typed ctors, pure-Canon parser) | `runtime/json_*` (9 fixtures) |
| `List<String>.Json()` (né `toJsonArray`) | `runtime/list_to_json_array` |
| `canon/std/cli` — `Args()`, `Cwd()` (list<string> / option<string> externs) | `runtime/cli_args_cwd` |
| `canon/std/cli` — `exit` (narrow-int u8 extern, exit-code mapping) | `exit_code_test.rs::exit_code_propagates` |
| `canon test` framework (assert, FAIL banner, exit codes) | `canon_tests.rs` (runs all `tests/canon/`), `exit_code_test.rs::canon_test_exit_codes` |
| `canon/std/random`, `canon/std/time` (monotonic) | `examples/random`, `examples/now`, `examples/clock` (smoke only — **hole H4**) |
| `canon/std/fs` read | `examples/read-file` (smoke only — **hole H4**) |
| HTTP handler world (status, body, routing, `request.path()`) | `wasi_http_service_test.rs` (end-to-end over TCP) |
| Legacy dynamic handler (`handleRequest`) + SSE prefix | `http_handler_test.rs`, `runtime/handle_request_export` |
| Streams (checker surface only) | `ok/stream_compose` |

## Compiler machinery

| Area | Coverage |
|---|---|
| `wit-component` encoder round-trips (http-like, streams) | `wit_component_prototype.rs`, `wit_component_stream_prototype.rs` |
| Checker internals under synthetic input | `checker_api.rs` |
| Golden error messages (all `fail/` fixtures) | `checker_fixtures.rs` harness |
| Golden program output (all `runtime/` fixtures) | `runtime_fixtures.rs` harness |

## Known holes (ranked)

- **H1 — `List<String>.get`/`first` type confusion.** The checker
  accepts it; the runtime returns a type-confused payload. Tracked in
  the CLAUDE.md gap table (needs `Ty::List` element tracking). No
  fixture can pin the *correct* behaviour until it exists; add
  `runtime/list_get_strings` with the fix.
- **H2 — nested `.map` clobbers the outer element binding.** Same
  tracking; add a fixture with the fix.
- **H3 — HTTP handler negative paths.** No `fail/` fixture for a
  handler using unsupported extern imports (the `new_http` rejection
  path prints to stderr and exits; needs a harness that captures it).
- **H4 — clock/random/fs stdlib wrappers rely on examples as smoke.**
  Examples are not part of `cargo test`. Non-deterministic outputs
  don't fit golden fixtures; the right shape is `canon/` tests with
  tautological assertions (e.g. `Random().ge(0)` — needs `ge`), or a
  Rust integration test asserting exit 0.
- **H5 — formatter has no fixture layer.** `canon fmt` correctness is
  only enforced transitively (every fixture must be canonically
  formatted). A `fmt`-idempotence sweep over `tests/**/*.can` in a
  Rust test would pin it cheaply.
- **H6 — `just examples` is not in CI.** The V1 M4 checklist tracks
  promoting it (or a subset) into a `tests/*.rs` harness.

## Invariants the harnesses enforce

- Every `.can` fixture must be canonically formatted (`canon test`
  and the fixture harnesses run the format check).
- Every `fail/` fixture's `.stderr` golden is byte-exact; every
  `runtime/` fixture's `.stdout` golden is byte-exact
  (`just update-fixtures` regenerates both; the git diff is the
  review surface).
- `cargo test` is the single CI entry point; anything not reachable
  from it is smoke, not coverage.
