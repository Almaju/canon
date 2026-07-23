# Known Codegen Gaps

A few features parse and type-check structurally but are not implemented by
the code generator yet. Accepting them would be a silent trap — the program
passes `canon check`, then fails (or miscompiles) at `canon build`. So the
checker rejects them: reaching one of these features from the entry point is
a hard error, the accepted language and the implemented language stay the
same set, and a clean check guarantees the build won't fail in code
generation. Each gap below is a self-contained future PR; closing one
deletes its error.

This page is the canonical list. The checker's `CODEGEN_GAPS` table
(`src/checker/mod.rs`) mirrors the rejected features, `canon install`'s skip
reasons (`src/bindgen/emit.rs`) mirror the unbindable WIT shapes, and tests
pin both to this page, so the list stays in one place.

## compound `List<T>` / `Option<T>` payloads

Scalar and `String` payloads lower fully: `List<String>` shares the
canonical layout, 64-bit scalars share Canon's 8-byte list stride, narrower
scalars from `wasi:` bindings (`list<u8>` from `wasi:random`, for example)
are read back per-width using the vendored WIT, and `At(i)` / `First` /
`Mapped` / `Filtered` / `Taken` chains work on all of them. Compound payloads — products, unions
(other than `Bool`, which erases to a scalar), and nested containers — do
not fit the 8-byte element slot, so declaring, constructing, or dispatching
on a `List` / `Option` of one is rejected wherever it appears: binding
returns, plain signatures, `List(…)` literals, `-> Some`, `Mapped` lambdas,
and `Some<T>` dispatch arms. Outside the `wasi:` namespace, narrow element
widths are unknowable at codegen time, so `canon install` also skips those
bindings.

## extern imports in the `wasi:http/service` world

An HTTP handler program (`Request => Response`) may import only
`wasi:http/types`; the `wasi:http/service` world has no host for anything
else (`Parallel` / `Race` still work — they are compiler builtins emitted
inline, not imports). The restriction applies to every *loaded* extern, not
just called ones — codegen links the whole import block — and a JSON
literal with interpolation holes loads the `canon:builtins/json` bridge, so
it trips this too. HTML and format-string interpolation lower without a
bridge and work in handlers.

## `Stream<T>` lowering and streaming response bodies

The checker supports `Stream<T>` as a type expression, but codegen drops
imports whose signatures mention it, so any program reaching a
`Stream`-shaped declaration is rejected (and the stdlib ships no `Stream`
bindings). The enabling move is routing Stream-using programs through
`wit_component::ComponentEncoder` instead of the hand-rolled `wasm-encoder`
type section.

## HTTP handler request headers and body

Not rejected — not expressible. `method()` and `path()` land, but the
stdlib exposes no accessor for the request headers or body, so no accepted
program can reach the missing lowering. The vendored WIT and the embedded
runtime already carry both (`get-headers`, `consume-body`); wiring them into
codegen and restoring a `body` binding in `canon/std`'s `wasi:http` wrapper
is the future PR.

## WIT shapes `canon install` skips

Some WIT shapes can't be spotted from Canon source at all — they depend on
type detail the checker never sees (Canon has only `Int`, not `u8`/`u16`/…)
or on handle types with no Canon-value lowering. These never enter the
accepted language: `canon install` refuses to bind them, reporting the
skip on stderr, so no generated declaration exists to reach. Each skip
reason is a gap in the WIT→Canon emitter:

- **resource method / handle in signature** — WIT `resource` methods,
  constructors, and statics, and any function whose signature transitively
  mentions an `own<T>` / `borrow<T>` handle. Bindgen still emits the
  resource *types* as `Foo = Handle` newtypes, and hand-written wrappers
  over them (as in `wasi:http`'s `types.can`) do compile.
- **bare `result` parameter** — a payloadless `result` as a *parameter*
  (see `wasi:cli/exit#exit`). A bare-`result` *return* decodes into an
  ordinary Canon `Result`.
- **sub-u64 integer inside a compound shape** — a `u8`/`u16`/`u32`/`s8`/
  `s16`/`s32` nested inside a `variant` or record parameter. Top-level
  scalar returns, record-of-scalars returns, and (for `wasi:*` imports,
  where the vendored WIT supplies the width) scalar `list` / `option`
  payloads are handled.
