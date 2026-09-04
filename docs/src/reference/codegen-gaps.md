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

## compound `List<T>` / `Option<T>` payloads in bindings

Inside a Canon program every payload lowers: a product, union, or nested
container is one pointer (or a `(ptr, len)` pair) in the 8-byte element
slot, so `List<Todo>`, `Option<Todo>`, `List<List<Int>>`, `Mapped` lambdas
producing products, and `Some<Todo>` dispatch arms all build and run. What
does not lower is the canonical-ABI side: a WIT `list<record>` or
`option<variant>` lays its payload out inline, and the binding decoder only
reads back scalar and `String` payloads (64-bit scalars share Canon's
8-byte stride; narrower scalars from `wasi:` bindings, `list<u8>` from
`wasi:random` for example, are read per-width using the vendored WIT). So a
binding whose signature carries a compound `List` / `Option` payload —
directly or behind its minted result newtype — is rejected. Outside the
`wasi:` namespace, narrow element widths are unknowable at codegen time, so
`canon install` also skips those bindings.

## extern imports in the `wasi:http/service` world

An HTTP handler program (`Request => Response`) may import only
`wasi:http/types`; the `wasi:http/service` world has no host for anything
else (`Parallel` / `Race` still work — they are compiler builtins emitted
inline, not imports). The restriction applies to the externs a handler can
*reach*: the loader is file-granular, but codegen compiles the reachable
set, so a binding's unreached siblings are neither linked nor reported. A
JSON literal with interpolation holes still trips this — a hole lowers
through the whole `Encoded` family, whose `Float` member is the
`canon:builtins/json` bridge — while `-> Json` over strings reaches no
such member and works. HTML and format-string interpolation lower without
a bridge and work in handlers.

## `Stream<T>` lowering and streaming response bodies

One stream shape lowers: a `wasi:*` binding whose WIT returns
`tuple<stream<u8>, future<result<_, error-code>>>` and takes no stream
or future — `wasi:cli/stdin`'s `read-via-stream`, and the filesystem and
socket functions of the same shape. Canon has no surface for `stream` or
`future`, so the binding is spelled as an ordinary fallible string
(`Unit => Result<Stdin, IoError>`); the code generator imports the
canonical-ABI `stream.read` and `stream.drop-readable` builtins, drains
the stream to its end into one contiguous string at the call boundary,
drops both handles, and hands back the same `Result` struct a
`result<string, string>` return produces. `?` and dispatch never see a
stream.

`wasi:http/client`'s `send` is the other lowering, fused into one round
trip: the stdlib binding takes the request as strings (`Authority *
Body * Method * PathWithQuery * RequestHeaders * Scheme`), codegen
builds the `request` resource through `wasi:http/types`, writes the
body into its stream while the async `send` is in flight, and drains
the response body the same way `Stdin` drains — so `Url -> Fetched?`
imports only the standard interfaces.

`wasi:filesystem`'s `read-via-stream` and `write-via-stream` are fused
the same way: the stdlib binding takes the path (and the contents),
codegen opens the file under the first preopened directory (`open-at`,
async), then drains the read stream or writes the contents into a
fresh stream and reads the completion future.

Everything else about streams is still the gap: a `stream<T>` of any
other element type, a stream or future in a *parameter* of a binding
spelled by hand (`wasi:cli/stdout`'s `write-via-stream`), a `future`
returned on its own,
`Stream<T>` written in a Canon signature, and streaming rather than
draining — which is what the handler request body below waits on. Any
such binding is a checker error; `canon install` skips the WIT shapes
it cannot spell. Draining was chosen over a `Stream<T>` value because
every consumer the stdlib has wants the whole input as a string, and
the drain costs nothing the consumer would not have paid.

## HTTP handler request headers and body

Not rejected — not expressible. `method()` and `path()` land, but the
stdlib exposes no accessor for the request headers or body, so no accepted
program can reach the missing lowering. The vendored WIT and the embedded
runtime already carry both (`get-headers`, `consume-body`); wiring them into
codegen and restoring a `body` binding in `canon`'s `wasi:http` wrapper
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
