# Standard Library

The standard library is embedded in the compiler: nothing to install,
nothing to import — referencing a stdlib type by name (`File`, `Now`,
`Random`) loads its module automatically, and a name that clashes with
one of your own types is a compile error.

Each module exposes a single primary type, written in ordinary Canon
over [binding-file](../spec/compilation.md) declarations against
standard [WASI](https://github.com/WebAssembly/WASI) interfaces or
temporary `canon:builtins/*` host bridges — see
[Using WASI Interfaces](./wasi.md) for the layering. Idiomatic code
only ever reaches the wrappers below.

## At a Glance

| Module (`canon/std/...`) | Type | Backing binding | Notes |
|---|---|---|---|
| `time/Mark` | `Mark = Int` | `wasi/clocks/monotonic_clock` | `Mark()` reads the monotonic clock (nanoseconds) |
| `Random` | `Random = Int` | `wasi/random/random` | `Random()` returns a fresh cryptographically-secure `Int` |
| `time/Now` | `Now = String` | pure Canon over `time/Unix` | RFC 3339 wall-clock time |
| `time/Date` | `Date`, `Weekday`, `Hour`, `Minute`, `Second` | pure Canon over `time/Unix` | calendar components: `Unix() -> Date` is a `Day * Month * Year` product |
| `fs/Path` | `Path = String` | none | filesystem path newtype |
| `fs/File` | `File` | `canon:builtins/filesystem` | `File`, `Read`, `Written` |
| `fs/Contents` | `Contents = String` | none | file-contents newtype (the `Written` receiver) |
| `IoError` | `IoError = String` | none | filesystem error newtype |
| `Map` | `Map = Empty + Node` | pure Canon | sorted key->value map (`String` keys/values) |
| `Set` | `Set = Absent + Entry` | pure Canon | sorted string set; `set -> List` = members, alphabetically |
| `Int` | `Int = (String) => Result<Int, MalformedInt>` | pure Canon | the fallible parse constructor: `Int("42")?` |
| `MalformedInt` | `MalformedInt = String` | none | `Int(String)`'s error newtype |
| `Byte` | `Byte = Int` | none | picks the byte->character reading of `String(...)`: `String(Byte(65))` is `"A"` |
| `Case` | `Lowercased`, `Uppercased` | pure Canon | ASCII case mapping: `"Hi" -> Uppercased` is `"HI"` |
| `http/Url` | `Url`, `Fetched`, `InvalidUrl` | pure Canon (validation) + `canon:builtins/http` (fetch) | `Url`, `Fetched` (blocking GET) |
| `Base64` | `Base64 = String`, `Base64Encoded`, `Base64Decoded` | pure Canon | RFC 4648 base64: `Base64Encoded("hi")`, `Base64("aGk=") -> Base64Decoded?` |
| `Hex` | `Hex = String`, `HexEncoded`, `HexDecoded` | pure Canon | lowercase hex octets: `HexEncoded("hi")`, `Hex("6869") -> HexDecoded?` |
| `http/HttpError` | `HttpError = String` | none | HTTP-client error newtype |
| `Json` | `Json = String`, `MalformedJson` | pure Canon (`from-float` excepted) | `Json` (validate), the `Encoded` family, `Field`, `Decoded` |
| `Markdown` | `Markdown = String` | pure Canon | `Markdown -> Html` renders to HTML; see [Markdown](./markdown-renderer.md) |
| `web/Html` | `Html = String`, `Escaped` | pure Canon | HTML element vocabulary + escaping; see [The Web Target](./web-target.md) |
| `TestResult` | `TestResult = Fail + Pass` | pure Canon | for `canon test`; see [Testing](../learn/testing.md) |
| `cli/Exit` | `Exit = Int`, `Exited` | `wasi/cli/exit` | the CLI entry's return world; `3 -> Exited` hard-terminates with that code |
| `cli/Args` | `Args = List<String>` + `Args()` accessor | `wasi/cli/environment` | the program's argv -- the CLI entry's `Args` input, or `Args()` from any code |
| `cli/Cwd` | `Cwd = String`, `Unit => Option<Cwd>` | `wasi/cli/environment` | initial working directory, when the host provides one |
| `time/Unix` | `Unix = Int`, `Unix()` | `wasi/clocks/system_clock` | wall-clock Unix seconds |
| `http/Request`, `http/Response`, `http/Body`, `http/Headers`, `http/Status` | resource handles + newtypes | `wasi/http/types` | the `wasi:http/service` world |

Anything not listed is third-party territory: a library to be
published, or future stdlib work.

---

## Clocks and Randomness

```canon
Unit => Program {
    Mark() -> Print
    Random() -> Print
    Now() -> Print
}
```

`Mark()` reads the monotonic clock (nanoseconds, an `Int` newtype, so
arithmetic and comparison work directly — the name is the WASI
interface's own `mark` type). `Random()` draws from the WASI CSPRNG.
`Now()` is the RFC 3339 wall-clock time, formatted by a calendar
conversion written in pure Canon — the host provides only the `Unix()`
clock reading.

## Dates: `Date`, `Weekday`, `Hour`, `Minute`, `Second`

```canon
Ymd = String

Civil => Ymd {
    `{Civil.Year -> String}/{Civil.Month -> String}/{Civil.Day -> String}`
}

Unit => Program {
    Unix() -> Date -> Ymd -> Print
    Unix() -> Weekday -> Print
    Unix() -> Hour -> Print
}
```

`unix -> Date` converts a Unix reading to a `Civil = Day * Month * Year`
product (proleptic Gregorian, the same pure-Canon conversion `Now()`
formats with); declare a `Civil` receiver to read the parts back as
fields. `Weekday` is ISO — Monday is `1`, Sunday is `7`. `Hour` /
`Minute` / `Second` are the wall-clock time of day, in UTC like
everything else here.

## Files: `File`, `Path`, `Contents`, `IoError`

```canon
Unit => Program {
    Contents("hello from canon")
        -> Written(Path("/tmp/greeting.txt"))?
        -> Path
        -> File?
        -> Read?
        -> Print
}
```

```canon
File = (Path) => Result<File, IoError>

Read = String

File => Result<Read, IoError>

Written = Path

Contents * Path => Result<Written, IoError>
```

`path -> File?` opens; `file -> Read?` reads the whole contents;
`contents -> Written(path)?` creates or truncates and returns the path
as evidence — so a write chains straight into a re-open, as above.

## Map and Set

Sorted, immutable collections in **pure Canon** — recursive unions
walked by dispatch and recursion (`String` keys and values until
stdlib generics land). Every operation is a constructor named after
what it produces; iteration order is alphabetical by key, whatever the
insertion order (of course it is).

```canon
Unit => Program {
    Map()
        -> Inserted("b" * "2")
        -> Inserted("a" * "1")
        -> Keys
        -> Json
        -> Print
    Map() -> Inserted("k" * "v") -> Value("k") -> (
        * None => Unit { "absent" -> Print }
        * Some<Value> => Unit { Value -> Print }
    )
    Set()
        -> Added("b")
        -> Added("a")
        -> Added("b")
        -> Length
        -> Print
}
```

Map: `Inserted`, `Removed`, `Value` (lookup, `Option`), `Contains`,
`Keys`, `Values`, `Length`. Set: `Added`, `Dropped`, `Contains`,
`Length`, `List` (members, alphabetically). Both double as reference
code for [recursive types](../spec/types.md#recursive-types).

## Conversions: `Int`, `Byte`, `Case`

The infallible directions are pure Canon in `string.can` — `String(42)`
is `"42"` by digit recursion, and `String(2.5)` / `String(True())`
render the same way (`Print` goes through the same constructors); the
fallible direction is a validated constructor in pure Canon:

```canon
Int = (String) => Result<Int, MalformedInt>
```

`Byte = Int` picks the character reading of `String(...)`:
`String(42)` is `"42"`, `String(Byte(42))` is `"*"` — wrapping to mean
the other thing is what newtypes are for. `Uppercased` / `Lowercased`
map ASCII case.

## Encodings: `Base64`, `Hex`

```canon
Unit => Result<Program, MalformedBase64> {
    Base64Encoded("Canon") -> Print
    Base64("Q2Fub24=") -> Base64Decoded? -> Print
    HexEncoded("Canon") -> Print
    Unit() -> Ok
}
```

`Base64Encoded` / `HexEncoded` encode a string's bytes — RFC 4648
base64 with padding, lowercase hex octets — in pure Canon. Decoding is
the validating direction: tag the received text (`Base64(s)` /
`Hex(s)`) and pipe `-> Base64Decoded?` / `-> HexDecoded?`; bad length,
characters outside the alphabet, or padding before the end are the
module's `MalformedBase64` / `MalformedHex` error. Uppercase hex
digits decode fine; encoding always emits lowercase.

## HTTP Client: `Url`, `Fetched`

```canon
Unit => Program {
    Url("http://example.com")?
        -> Fetched?
        -> Print
}
```

`Url(s)` validates (scheme, non-empty host); `url -> Fetched?` is a
blocking GET returning the body. TLS and async lowering arrive with
the `wasi:http/outgoing-handler` migration.

## `Json`

`Json = String`: JSON-encoded text. Object and array **literals are
first-class expressions**, part of the prelude — nothing to import:

```canon
Doc = String

Labeled = Json

Int => Labeled {
    {"answer":Int,"doubled":Int -> Product(2),"ok":True()}
}

Unit => Result<Program, MalformedJson> {
    Doc("[1, 2, 3]") -> Json? -> Print
    Encoded(42) -> Print
    {"a":1,"b":[true,false,null]} -> Print
    Labeled(42) -> Print
    Unit() -> Ok
}
```

- **Static** literal members are baked into a constant at parse time
  and work in every world, including HTTP handlers.
- **Interpolated** members convert at runtime via `-> Encoded`
  (`Encoded = Json`, family members for `Bool`, `Float`, `Int`,
  `String`; newtype chains follow to their base member). The `Float`
  member is host-backed, which the HTTP world can't satisfy yet.
- `Json("…")` validates a *runtime-built* string (full JSON grammar,
  pure Canon); feeding it a static literal the literal form can
  express is a checker error — the literal is the one spelling.
- Read back with `json -> Field("key")` (the raw text of an object
  field) and `json -> Decoded` (a JSON string's contents, escapes
  handled).
