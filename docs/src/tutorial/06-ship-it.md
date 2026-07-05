# Ship a Component

Everything so far ran under `canon run`. The last step is realizing
that `canon run` was never special: the thing it runs is a standard
artifact you can take anywhere.

## Build

```sh
$ canon build notes-api
Compiled to: notes-api/build/notes-api.wasm
WIT world : notes-api/build/notes-api.wit
```

Two files:

- **`notes-api.wasm`**: a WebAssembly **Component** (WASI Preview 3).
  Not a Canon-specific binary, but a self-describing component that
  imports only standard `wasi:*` interfaces and exports
  `wasi:http/handler#handle`.
- **`notes-api.wit`**: a human-readable description of the component's
  *world*, everything it imports and exports. Feed it to `wasm-tools`
  or any Component Model tooling; this file is the component's
  contract.

There is no step three. No Dockerfile, no target triples, no
cross-compilation matrix: the same `.wasm` runs on any host, any OS,
any architecture.

## Run It Anywhere

Any host that implements the `wasi:http/handler` contract for WASI
Preview 3 can serve the component. Canon's embedded wasmtime is the
reference host:

```sh
canon run notes-api --addr 0.0.0.0:9000
```

The WASI 0.3 release-candidate version is embedded in every interface
name inside the component, so a host either matches exactly or fails
loudly at instantiation; there is no silent version skew. As stock
hosts (e.g. `wasmtime serve`) pick up the same rc, the artifact carries
over unchanged; nothing in it is Canon-specific. The details live in
[Deploying Components](../reference/deploying.md).

## What You Built

Thirty-ish lines of Canon, and along the way, most of the language:

| You used | The idea |
|---|---|
| `Request => Response` | the entry-point rule: selected by signature, no name needed |
| `Request.path().( … )` | union dispatch, the only branching construct |
| `String.( * ("/notes") => … )` | literal dispatch with a mandatory catch-all: the route table |
| `Body`, `Status(404)`, `Note` | newtypes as documentation and access control |
| `-> Joined(…)` chains | no locals; data flows through the pipe |
| `Note` in its own file | file-based modules, one type per file, imports by reference |
| `() => TestResult` | signature-driven test discovery |
| `canon build` | one portable component, no toolchain |

## Where to Go Next

- The API can't yet read the request **method, headers, or body**: the
  `wasi:http` request-introspection surface beyond `.path()` is still
  being wired up, so POST/create-note is tomorrow's tutorial chapter,
  not today's. The [Serving HTTP](../tour/http.md) chapter tracks
  what's live.
- The [Tour](../tour/philosophy.md) covers the language systematically:
  traits, async, error handling with `?`.
- The [Specification](../spec/index.md) has the precise rules behind
  everything you just used.
- The repository's [`examples/`](../examples/index.md) directory holds
  the finished [notes-api](../examples/notes-api.md) plus CLI, file, and
  HTTP-client programs.
