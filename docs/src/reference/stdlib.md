# Standard Library

The Oneway standard library is **embedded into the compiler**: there are
no files to install. Import any module with `use std/TypeName` at the top
of your file.

Each module exposes a single primary type. Behind the scenes the modules
are written in ordinary Oneway on top of [`extern
Wasm`](../tour/extern.md) declarations against either standard
[WASI](https://github.com/WebAssembly/WASI) interfaces or temporary
`oneway:builtins/*` host bridges. The `oneway run` runtime fulfils both
sets, so you don't need to think about the boundary.

## At a Glance

| `use std/…` | Type | Backing interface | Notes |
|---|---|---|---|
| `Clock` | — (free functions) | `wasi:clocks/monotonic-clock` | `nowNanos`, `resolutionNanos` |
| `Random` | — (free function) | `wasi:random/random` | `randomInt` |
| `Now` | `Now` | `oneway:builtins/clock` | RFC 3339 wall-clock time |
| `Path` | `Path = String` | — | filesystem path newtype |
| `File` | `File` | `oneway:builtins/filesystem` | `File`, `read` |
| `IoError` | `IoError = String` | — | filesystem error newtype |
| `Url` | `Url`, `InvalidUrl` | `oneway:builtins/url` + `oneway:builtins/http` | `Url`, `get` |
| `HttpError` | `HttpError = String` | — | HTTP-client error newtype |
| `HttpServer` | `HttpServer<S>` | `oneway:builtins/http-server` | stub server, see [Status](#status) |
| `Request`, `RoutePath`, `Port`, `Body`, `HttpResponseBody`, `HttpStatus` | various | — | HTTP-server helpers |
| `Json` | `Json = String` | — | placeholder; parser TBD |
| `TestResult` | `TestResult = Fail + Pass`, `assert` | pure Oneway | for `oneway test` |

---

## `Clock`

Monotonic clock access. The methods are free functions — there is no
wrapper type.

```oneway
use std/Clock

main = () -> Unit {
    nowNanos().print()
    resolutionNanos().print()
}
```

```oneway
nowNanos        = () -> Int
resolutionNanos = () -> Int
```

Backed by `wasi:clocks/monotonic-clock@0.3.0-rc-2026-03-15`.

## `Random`

Cryptographic-quality random integers.

```oneway
use std/Random

main = () -> Unit {
    randomInt().print()
}
```

```oneway
randomInt = () -> Int
```

Backed by `wasi:random/random@0.3.0-rc-2026-03-15`.

## `Now`

Current UTC wall-clock time, formatted as an RFC 3339 string. Useful for
log lines.

```oneway
use std/Now

main = () -> Unit {
    Now().print()      # e.g. 2026-05-23T22:30:35Z
}
```

```oneway
Now = String
Now = () -> Now
```

Currently backed by `oneway:builtins/clock` (the host formats the time);
will move to `wasi:clocks/wall-clock` once that interface's canonical-ABI
shape lands.

## `File`, `Path`, `IoError`

Synchronous file I/O.

```oneway
use std/File
use std/Path

main = () -> Unit {
    Path("./Cargo.toml").File()?.read()?.print()
}
```

```oneway
Path = String

File = String
File = (Path) -> Result<File, IoError>
read = (File) -> Result<String, IoError>
```

`Path("…").File()` opens the file (returning a `File` handle or an
`IoError`); `.read()` reads the entire contents as a `String`. Backed by
`oneway:builtins/filesystem`; will move to the async
`wasi:filesystem/types` interface once Phase 5 lands.

## `Url`, `HttpError`

URL parsing + blocking HTTP GET.

```oneway
use std/HttpError
use std/Url

main = () -> Unit {
    Url("http://example.com")?.get()?.print()
}
```

```oneway
InvalidUrl = String
Url        = String
HttpError  = String

Url = (String) -> Result<Url, InvalidUrl>
get = (Url) -> Result<String, HttpError>
```

`Url(s)` is a validated constructor — it rejects malformed inputs.
`.get()` performs a blocking HTTP GET and returns the response body. TLS
(`https://`) and async lowering arrive with the
`wasi:http/outgoing-handler` migration.

## HTTP Server

Build up a router by chaining `.get(…)` / `.post(…)` and call `.serve()`
to start listening.

```oneway
use std/Body
use std/HttpResponseBody
use std/HttpServer
use std/HttpStatus
use std/IoError
use std/Port
use std/Request
use std/RoutePath

State = Unit

main = () -> Result<Unit, IoError> {
    "Starting server on port 3000...".print()
    Port(3000)
        .HttpServer(State(Unit()))
        .get(RoutePath("/"),
             (Request * State) -> HttpResponseBody {
                 HttpResponseBody(Body("Hello from Oneway!") * HttpStatus(200))
             })
        .serve()
}
```

```oneway
HttpServer<S> = String

HttpServer<S> = (Port * S) -> HttpServer<S>
get<S>        = (HttpServer<S> * RoutePath * (Request * S) -> HttpResponseBody) -> HttpServer<S>
post<S>       = (HttpServer<S> * RoutePath * (Request * S) -> HttpResponseBody) -> HttpServer<S>
serve<S>      = (HttpServer<S>) -> Result<Unit, IoError>    # async
```

### Status

`HttpServer` is a **stub**: the program checks, builds, and runs to the
"Starting server…" banner, but `.serve()` currently returns
immediately — no socket is actually opened. Real serve semantics need
host-driven invocation of guest handler lambdas (function-table indirect
calls or resource-keyed handler tables), which is Phase-5 work. The user
API is pinned now so existing programs survive the swap.

## `TestResult`

The Oneway-language testing primitive. See the
[testing notes](https://github.com/Almaju/oneway/blob/main/CLAUDE.md#testing)
for the full convention.

```oneway
use std/TestResult

testAddPositive = () -> TestResult {
    1.add(2).eq(3).assert("1 + 2 != 3")
}
```

```oneway
Fail = String
Pass = Unit

TestResult = Fail + Pass

assert = (Bool * String) -> TestResult
```

`oneway test <file>` discovers every `() -> TestResult` function in the
entry file and runs them, printing `[ ok ] testName` or
`[FAIL] testName: message` per test.

## Not Yet Available

The following modules appear in `examples/` but **do not compile yet** —
they're pinned as a target for upcoming work:

- `JsonValue`, `JsonArray`, `JsonObject`, `MalformedJson` — pure-Oneway
  JSON parser; see `examples/parse-json.ow`, `examples/json-literal.ow`.

Anything not listed in *At a Glance* above is third-party territory —
either a library to be published under any path, or future stdlib work.
