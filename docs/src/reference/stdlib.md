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

Every type name `canon` declares is global — a type of the same
name in your own project is a compile error — so the complete set,
internal helpers included, is worth checking before you name a type.
That list is the index of the [generated API
reference](api/index.html), which `canon doc` derives from the
stdlib source itself and therefore cannot drift from it. This page is
the prose the generator has no way to write.

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
    Unix()
        -> Date
        -> Ymd
        -> Print
    Unix()
        -> Weekday
        -> Print
    Unix()
        -> Hour
        -> Print
}
```

`unix -> Date` converts a Unix reading to a `Civil = Day * Month * Year`
product (proleptic Gregorian, the same pure-Canon conversion `Now()`
formats with); declare a `Civil` receiver to read the parts back as
fields. `Weekday` is ISO — Monday is `1`, Sunday is `7`. `Hour` /
`Minute` / `Second` are the wall-clock time of day, in UTC like
everything else here.

## Standard input: `Stdin`

```canon
Unit => Result<Program, IoError> {
    Stdin()?
        -> Lines
        -> Length
        -> Print
    Unit() -> Ok
}
```

`Stdin()` reads standard input to its end and hands it back as one
string, so a filter is `Stdin()? -> Lines -> …` and the shell's pipe
does the streaming. It is the `wasi:cli/stdin` stream, drained at the
boundary: a binding whose WIT returns `tuple<stream<u8>, future<result<_,
error-code>>>` surfaces in Canon as an ordinary fallible string, which
is the one `Stream` shape the code generator lowers (see the [codegen
gaps](./codegen-gaps.md)).

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

```text
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
        -> Inserted(Key("b") * Value("2"))
        -> Inserted(Key("a") * Value("1"))
        -> Keys
        -> Json
        -> Print
    Map() -> Inserted(Key("k") * Value("v")) -> Value("k") -> (
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

```text
Int = (String) => Result<Int, MalformedInt>
```

`Byte = Int` picks the character reading of `String(...)`:
`String(42)` is `"42"`, `String(Byte(42))` is `"*"` — wrapping to mean
the other thing is what newtypes are for. `Uppercased` / `Lowercased`
map ASCII case.

## Slicing: `From`, `To`

```canon
Unit => Program {
    "canonical"
        -> Substring(From(1) * To(5))
        -> Print
}
```

`Substring`'s bounds are the `From` / `To` newtypes (both `= Int` —
same underlying type, so the values must be tagged), 1-based and
inclusive at both ends: this prints `canon`.

## Splitting: `Split`, `Lines`

```canon
Unit => Program {
    "a,b,c"
        -> Split(Separator(","))
        -> Length
        -> Print
    Lines("first\nsecond") -> Reversed -> First -> (
        * None { "none" -> Print }
        * Some<String> { String -> Print }
    )
}
```

`Split` cuts a string at every occurrence of its `Separator` into a
`List<String>` (both `= String`, so the separator is tagged); adjacent
separators leave an empty element, and a string with no separator is a
one-element list. `Lines` is `Split` at `"\n"`. Both are pure Canon over
`Substring`, so a very long input pays a quadratic copy — fine for
configuration files and wire formats, not for logs.

## Encodings: `Base64`, `Hex`

```canon
Unit => Result<Program, MalformedBase64> {
    Base64Encoded("Canon") -> Print
    Base64("Q2Fub24=")
        -> Base64Decoded?
        -> Print
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
    Url("https://example.com")?
        -> Fetched?
        -> Print
    Body("{\"q\":1}")
        -> Fetched(
            Method("POST")
            * RequestHeaders("content-type: application/json")
            * Url("https://example.com/search")?
        )?
        -> Print
}
```

`Url(s)` validates (scheme, non-empty host). `url -> Fetched?` is a
GET; the four-input form takes the `Method`, the `RequestHeaders` as
`name: value` lines, and the `Body`. HTTP and HTTPS both work. A 2xx
answers with the response body; any other status is the `HttpError`
(`HTTP 404 Not Found: …`), as is a transport failure. The request
blocks; async lowering arrives with the `wasi:http` client migration.

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
    Doc("[1, 2, 3]")
        -> Json?
        -> Print
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
