# Tutorial: A JSON API

You will build a small JSON API: a notes service with an index route, a
per-note route, and a 404 fallback, shipped as a portable WebAssembly
component. Every chapter ends with a program you can run and `curl`.

By the end you'll have:

```sh
$ curl localhost:8080/notes
[{"title":"ship canon v1"},{"title":"write the docs"}]

$ curl localhost:8080/notes/1
{"title":"ship canon v1"}

$ curl -i localhost:8080/other | head -1
HTTP/1.1 404 Not Found
```

Roughly thirty lines of Canon serve that. No framework, no router DSL,
no server boilerplate, and a `.wasm` artifact at the end that runs on
any WASI Preview 3 host.

Along the way you'll meet most of the language: the entry-point rule,
dispatch, newtypes, method chaining, modules, testing, and the
compilation model. Each chapter introduces exactly one idea:

1. **[A Service in Five Lines](./01-hello-service.md)**: the entry-point
   rule. A function returning `Response` *is* an HTTP service.
2. **[Routing Is Dispatch](./02-routing.md)**: no route DSL; routing is
   the same union dispatch used everywhere else.
3. **[JSON, Without a Framework](./03-json.md)**: JSON bodies are
   JSON literals; no serializer, no middleware.
4. **[Growing Into Modules](./04-modules.md)**: a real package, with a
   `Note` type in its own file, loaded automatically by reference.
5. **[Testing the API](./05-testing.md)**: `canon test` and the
   `TestResult` type.
6. **[Ship a Component](./06-ship-it.md)**: `canon build`, the `.wit`
   world, and what "portable" means.

## Prerequisites

A working `canon` binary ([Installation](../getting-started/installation.md))
and `curl`. Canon has no other toolchain.

The [Tour](../tour/philosophy.md) is not a prerequisite. The tutorial
explains what it uses as it goes, and links into the Tour and
[Specification](../spec/index.md) for depth.
