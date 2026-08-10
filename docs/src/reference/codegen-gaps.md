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
`Mapped` / `Filtered` / `Taken` chains work on all of them. `At(i)` and
`First` yield `Option<T>`, so a `String` element reaches the value level
through `?` (or a `Some<String>` arm) — the option carries the element's
`(ptr, len)` pair, not a scalar. Compound payloads — products, unions
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
inline, not imports). The restriction applies to the externs a handler can
*reach*: the loader is file-granular, but codegen compiles the reachable
set, so a binding's unreached siblings are neither linked nor reported. A
JSON literal with interpolation holes still trips this — a hole lowers
through the whole `Encoded` family, whose `Float` member is the
`canon:builtins/json` bridge — while `-> Json` over strings reaches no
such member and works. HTML and format-string interpolation lower without
a bridge and work in handlers.

## `Stream<T>` lowering and streaming response bodies

The checker supports `Stream<T>` as a type expression, but codegen drops
imports whose signatures mention it, so any program reaching a
`Stream`-shaped declaration is rejected (and the stdlib ships no `Stream`
bindings). The enabling move is routing Stream-using programs through
`wit_component::ComponentEncoder` instead of the hand-rolled `wasm-encoder`
type section.

A binding can carry a stream without saying so. Canon has no surface for
`stream` or `future`, so `wasi:cli/stdin`'s
`func() -> tuple<stream<u8>, future<result<_, error-code>>>` is spelled
`Unit => Result<Stdin, IoError>` and reads as an ordinary fallible string
constructor. The vendored WIT is the only place the real shape appears, so
the rejection consults it: any `wasi:*` binding whose WIT signature
mentions a `stream` or `future` in any position is a checker error. Without
that, codegen types the import from the Canon signature and the component
fails to *instantiate* against a host carrying the real shape — a program
that passed both `check` and `build`.

This gap gates the rest of the WASI migration: `wasi:filesystem`'s
`read-via-stream`, the `wasi:http` client's response bodies, and the
handler request body below all wait on the read half. Four things about
the encoder are worth knowing before starting, each of which costs an
afternoon to rediscover:

- **`run_core_fn` counts past the whole canon section.** `component.rs`
  indexes the aliased `run` export as `13 + externs.len()`, so every canon
  builtin added in §7c shifts it. A stale count fails validation as
  `lowered parameter types [] do not match parameter types [I32, I32, I32]
  of core function 14` — a numbered core function, nothing naming `run`.
  Adding the two read builtins makes it `15 + N`. This is the
  component-level twin of the `fn_user_start` hazard the three encoder
  modes carry.
- **The read builtins are free until used.** A synthetic core instance may
  export more than the user core module imports, so declaring
  `stream.read` / `stream.drop-readable` alongside the existing write pair
  costs two core function indices and does *not* shift
  `FIRST_EXTERN_IMPORT_FN`.
- **`error-code` cannot be a literal.** The instance type for a
  stream-returning import must define and export `error-code` before the
  `result` / `future` / `tuple` chain, following the `ScalarRecord`
  define-then-export-then-reference-the-alias discipline. `wasi:cli`'s
  enum is three cases and `wasi:filesystem`'s is far larger, so the cases
  have to come from `vendored_resolve()` via the URN.
- **A sync `stream.read` blocks; it does not return `BLOCKED`.** Declared
  with `Memory(0)` and no `Async` — as the existing `stream.write` is —
  the host blocks the caller, so draining needs no waitable-set. The
  result packs as `(count << 4) | code` with `COMPLETED = 0`,
  `DROPPED = 1`, `CANCELLED = 2` (`BLOCKED = 0xffff_ffff` only arises for
  an async lower). `DROPPED` ends the stream.

The Canon-visible shape is an ordinary fallible string return, so the
decode can build the same 12-byte `Result` struct `ResultStringString`
produces and reuse `?` and dispatch unchanged; the return area itself is
8 bytes, the readable handle at +0 and the completion future at +4.

The one part that cannot be added incrementally: the decode *calls*
`stream.read`, so the user core module has to import it, which moves
`FIRST_EXTERN_IMPORT_FN` from 5 to 7 and shifts the fixed import blocks
of all three encoder modes together. That is the same hazard a
newly defined helper carries, and it is why this gap closes as one PR
rather than a series: the shape, its
classification, the instance type, the decode, and the relaxation of the
binding rejection above are all unreachable until they land at once.

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
