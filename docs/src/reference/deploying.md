# Deploying Canon Components

`canon build` produces a standard **WASI Preview 3 component** —
deployment is whatever your component host of choice does with a
`.wasm` file. Canon has no runtime of its own to ship.

## Build output

```sh
$ canon build my-app
Compiled to: my-app/build/my-app/my-app.wasm
WIT world : my-app/build/my-app/my-app.wit
```

The `.wit` sidecar documents the component's world — feed it to
`wasm-tools`, `wit-bindgen`, or a host's tooling to see exactly what
the component imports and exports.

## The two worlds

| Your entry | World | Exports |
|---|---|---|
| `main = () -> Unit` | `wasi:cli/command` | `wasi:cli/run@0.3.0-rc-2026-03-15` |
| `f = (Request) -> Response` | `wasi:http/service` | `wasi:http/handler@0.3.0-rc-2026-03-15#handle` |

## Running on the embedded host

`canon run` embeds wasmtime 45 with the standard WASI 0.3 linkers:

```sh
canon run my-app                      # CLI world: runs to completion
canon run my-app                      # HTTP world: serves on 127.0.0.1:8080
canon run my-app --addr 0.0.0.0:9000  # HTTP world: explicit address
```

Exit codes are real: a guest `Exit(3).exit()` terminates the process
with status 3, and `canon test` exits nonzero on failure — both safe
to wire into CI and shell scripts.

## Running on other hosts

The component targets the `0.3.0-rc-2026-03-15` WASI Preview 3
release candidate — the same rc `wasmtime-wasi` 45 implements. Any
host with matching p3 support can instantiate it:

- **HTTP components** need a host that serves `wasi:http/handler` —
  the contract `wasmtime serve` implements for its supported preview
  versions. Until stock CLI hosts ship this rc, `canon run --addr` is
  the reference host; the component itself contains nothing
  Canon-specific.
- **CLI components** need `wasi:cli/run` plus the standard cli /
  clocks / random interfaces.

One caveat during the transition: programs using `canon:builtins/*`
bridge interfaces (currently the legacy HTTP-client/filesystem/JSON
host helpers — the skip list in `canon install` and the At-a-Glance
table mark them) run only under `canon run` until their `wasi:*`
replacements land. Programs that stick to `canon/std` cli, clocks,
random, and the HTTP handler surface are fully portable.

## Version pinning

The WASI rc version is embedded in every interface name, so a
component either matches its host exactly or fails loudly at
instantiation — there is no silent skew. When the vendored WIT is
bumped (`wit-vendor/wasi/`), rebuild and redeploy.
