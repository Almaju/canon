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

## binding declarations returning `list<T>` for non-string `T`

*Warns.* The byte-packed canonical-ABI element layout needs per-width
read-back before the generator can decode a returned list. `List<String>`
returns already work; other element types (`List<Int>`, lists of records)
do not.

## sub-`u64` integers inside a compound WIT shape

*Not detected* (Canon source has only `Int`). A `u8`/`u16`/`u32`/`s8`/`s16`/`s32`
nested inside an `option` / `list` / `variant` / record parameter isn't lowered
yet. Top-level scalar returns and record-of-scalars returns are handled.

## WIT `result` with no payloads in binding declarations

*Not detected.* The bare `result;` form lowers to a discriminant-only shape the
generator currently renders as `u32`, so a binding declared over it decodes
incorrectly.

## non-string `option<T>` extern returns

*Warns.* Returning `option<T>` for a non-string `T` needs indirect-return
decoding that isn't implemented. `option<string>` returns work.

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
