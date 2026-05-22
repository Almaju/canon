# Standard Library

The Oneway standard library ships as `.ow` declarations backed by Rust FFI.
Import any module with `use ModuleName` at the top of your file.

---

## `clock`

Provides the current UTC time.

```oneway
use Datetime

now = (Clock) -> Datetime
```

`Clock` is a capability — it must be passed in through `main`.

**Example:**

```oneway
use Clock
use Datetime

main = (Clock * Stdout) -> Unit {
    Clock.now().toRfc3339().print(Stdout)
}
```

---

## `datetime`

The `Datetime` type and its formatting methods.

```oneway
Datetime

toRfc3339 = (Datetime) -> String
```

Backed by [`chrono`](https://docs.rs/chrono). `Datetime` represents a UTC
instant. `toRfc3339` formats it as an RFC 3339 string
(e.g. `"2026-05-22T10:00:00+00:00"`).

---

## `filesystem`

Async file I/O. Requires the `Filesystem` capability.

```oneway
use Path

IoError

read = (Filesystem * Path) -> Result<String, IoError>
```

**Example:**

```oneway
use Filesystem
use Path

main = (Filesystem * Stdout) -> Unit {
    Path("hello.txt").read(Filesystem).(
        Err(e) => e.print(Stdout),
        Ok(s)  => s.print(Stdout),
    )
}
```

---

## `http_client`

Async HTTP GET. Requires the `HttpClient` capability.

```oneway
use Url

HttpError

get = (HttpClient * Url) -> Result<String, HttpError>
```

**Example:**

```oneway
use HttpClient
use Url

main = (HttpClient * Stdout) -> Unit {
    Url("https://example.com").(
        Err(e) => e.print(Stdout),
        Ok(u)  => u.get(HttpClient).(
            Err(e) => e.print(Stdout),
            Ok(body) => body.print(Stdout),
        ),
    )
}
```

---

## `http_server`

HTTP server with method routing. Requires the `HttpServer` capability.

```oneway
HttpRequest  = String
HttpResponse = String
HttpRouter
IoError
Port       = Int
RoutePath  = String

get    = (HttpRouter * RoutePath * (HttpRequest) -> HttpResponse) -> HttpRouter
post   = (HttpRouter * RoutePath * (HttpRequest) -> HttpResponse) -> HttpRouter
router = (HttpServer) -> HttpRouter
serve  = (HttpRouter * Port) -> Result<Unit, IoError>
```

Build up a router by chaining `get` and `post` calls, then call `serve` to
start listening. Handler functions receive the raw request body as a `String`
and return a `String` response body.

**Example:**

```oneway
use HttpServer

main = (HttpServer * Stdout) -> Unit {
    HttpServer.router()
        .get(RoutePath("/"), (HttpRequest) -> HttpResponse {
            HttpResponse("hello")
        })
        .serve(Port(8080)).(
            Err(e) => e.print(Stdout),
            Ok(_)  => Unit,
        )
}
```

---

## `json`

Generic JSON parsing via the `Deserialize` trait.

```oneway
MalformedJson

parse = <T: Deserialize>(Json * String) -> Result<T, MalformedJson>
```

The type `T` must implement `Deserialize`. Backed by
[`serde_json`](https://docs.rs/serde_json).

**Example:**

```oneway
use Json

Name = String

main = (Stdout) -> Unit {
    Json.parse(String("{\"name\":\"Alice\"}")).(
        Err(e) => e.print(Stdout),
        Ok(n)  => n.print(Stdout),
    )
}
```

---

## `path`

A `Path` newtype over `String`. Used with `filesystem` and other file-related
capabilities.

```oneway
Path = String
```

---

## `url`

A `Url` type with a validated constructor that rejects malformed URLs.

```oneway
Url = String

InvalidUrl

Url = (String) -> Result<Url, InvalidUrl>
```

**Example:**

```oneway
use Url

main = (Stdout) -> Unit {
    Url("not-a-url").(
        Err(_) => "invalid url".print(Stdout),
        Ok(u)  => u.print(Stdout),
    )
}
```
