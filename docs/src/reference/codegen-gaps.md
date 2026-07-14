# Known Codegen Gaps

The checker deliberately accepts more than the code generator implements.
A handful of features parse and type-check but do not run yet: a program
using one passes `canon check`, then fails at `canon build` when it reaches
code generation. Each gap is a self-contained future PR, and each is pinned
where it can be — some by `tests/checker/ok/` fixtures that prove the feature
type-checks today.

To keep that acceptance honest as *feedback*, the checker emits a non-fatal
**warning** when a program reaches, from its entry point, a declaration that
relies on one of the statically-detectable gaps below. The program still
type-checks; the warning is a heads-up that the build will fail, delivered
before the build step instead of as a bare codegen error afterward. The
warning names the gap and points back to this page.

Not every gap can be spotted from Canon source alone — several depend on WIT
type detail the checker never sees (Canon has only `Int`, not `u8`/`u16`/…),
or on runtime-value types it doesn't track. Those are listed here for tracking
but do not warn. Each entry notes whether it warns.

This page is the canonical list. The checker's `CODEGEN_GAPS` table
(`src/checker/mod.rs`) mirrors it, and a test pins the two together, so the
list stays in one place.

## binding declarations returning `list<T>` for compound `T`

*Warns.* String and scalar elements decode: `List<String>` shares the
canonical layout directly, 64-bit scalars (`u64`/`s64`/`f64`) share
Canon's 8-byte list stride, and narrower scalars (`list<u8>` from
`wasi:random`'s `get-random-bytes`, for example) are read back per-width
using the vendored WIT and widened into a fresh Canon list. Lists of
*compound* elements (records, variants, nested lists) are the remaining
gap — and outside the `wasi:` namespace, narrow element widths are
unknowable at codegen time, so `canon install` skips those bindings.

## sub-`u64` integers inside a compound WIT shape

*Not detected* (Canon source has only `Int`). A `u8`/`u16`/`u32`/`s8`/`s16`/`s32`
nested inside a `variant` / record parameter isn't lowered yet. Top-level
scalar returns, record-of-scalars returns, and (for `wasi:*` imports,
where the vendored WIT supplies the width) scalar `list` / `option`
returns are handled.

## WIT `result` with no payloads as a binding parameter

*Not detected.* A bare `result;` *return* now decodes into an ordinary Canon
`Result` (the discriminant flips into Canon's alphabetical Err/Ok tags), but
the same shape as a *parameter* (see `wasi:cli/exit#exit`) has no Canon-value
lowering yet; `canon install` skips such functions.

## binding declarations returning `option<T>` for compound `T`

*Warns.* String and scalar payloads decode into an ordinary Canon
`Option` value, so `(None, Some<Int>)` dispatch and `?` work on the
result. Compound payloads (`option<instant>`, option-of-variant) are the
remaining gap, and narrow scalar widths outside `wasi:` are skipped by
`canon install` for the same width-unknowable reason as lists.

## WIT `resource` / `own<T>` / `borrow<T>` in binding signatures

*Not detected.* Bindgen emits the resource *types* as `Foo = Handle` newtypes
but skips every method / constructor / static — and any function whose
signature transitively mentions a handle. Because the offending functions are
skipped, they rarely survive as declarations to warn about, and hand-written
wrappers that supersede them (as in `wasi:http`'s `types.can`) do compile, so
the checker stays quiet here to avoid flagging working code.

## `At(i)` / `First` on `List<String>` and nested `Mapped`

*Not detected* (a runtime-value concern). `Ty::List` erases the element type at
codegen, so indexing into a `List<String>` or nesting `Mapped` doesn't lower.
Threading the element type through codegen is the enabling refactor.

## HTTP handler request headers and body

*Not detected.* The handler body compiles fully — dynamic status, dispatch,
string bodies — and `method()` / `path()` land, but reading the request
*headers* and *body* is not wired up. HTTP programs also can't use
non-`wasi:http` extern imports: the `wasi:http/service` world can't satisfy the
`canon:builtins/*` bridges.

## `Stream<T>` lowering and streaming response bodies

*Warns.* The stdlib combinator surface and the checker support `Stream<T>`, but
codegen drops imports whose signatures mention it, so such programs fail to
link. The enabling move is routing Stream-using programs through
`wit_component::ComponentEncoder` instead of the hand-rolled `wasm-encoder`
type section.
