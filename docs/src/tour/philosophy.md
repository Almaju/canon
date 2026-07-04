# Philosophy

> *There is one way to do everything.*

Most languages give you ten ways to do the same thing and ask you to
pick. Canon picks for you. If there is a best practice, it is the *only*
practice, and the compiler enforces it.

## Alphabetical Order, Everywhere

The most pervasive rule: wherever ordering is discretionary,
declarations must be alphabetical. This applies to:

- Components of a product type: `User = Birthday * Username`
- Variants of a union type: `Bool = False + True`
- Multiple methods on a type (declared top-to-bottom alphabetically)
- Arms of a dispatch (in the order of the union's variants)
- Trait composition: `Show = Debug * PrintString`
- Error unions inside `Result`: `Result<T, IoError + NotFound>`
- Multiple `use` statements at the top of a file

Reordering is never a meaningful change. Diffs that only reshuffle a list
do not exist. Two programmers writing the same code produce the same
bytes.

## Types Are the Documentation

Canon has **no local variables, no `let`**, and **no parameter names**.
The shape of a function is described entirely by its types.

```canon
compare = (OtherUser * User) -> Ord {
    User.Birthday.compare(OtherUser.Birthday)
}
```

The receiver is referred to as `User`, its type. The parameter is
referred to as `OtherUser`, its type. To disambiguate two parameters of
the same type, define a newtype; the newtype becomes the documentation:

```canon
OtherUser = User

User = Birthday * Username
```

Names lie. Types don't. Forcing every value through a named type makes
the data flow explicit and the documentation structural.

## Effects Are Honest

A function's signature does not lie about what it does. Reading a file
requires a `File` value; making an HTTP request requires a `Url`; running
an HTTP server requires an `HttpServer`. The values that carry the effect
are ordinary arguments:

```canon
get = (Url) -> Result<String, HttpError>

read = (File) -> Result<String, IoError>
```

There is no `unsafe`, no global mutable state, no service locator. A
function that doesn't take a `File` cannot read one: constructing a
`File` requires a `Path`, which requires a `String`. The type chain *is*
the access control. See [Effects and Values](./effects.md) for the full
story.

## No Comments

There are no comments. Code must speak through types and naming. The
urge to write a comment is usually the urge to introduce a newtype or
rename a method.

## Batteries-Included

Canon ships opinionated stdlib modules for the major application domains
(`HttpServer`, `File`, `Url`, `Clock`, `Random`, and more), each backed
by a standard `wasi:*` interface or, where that interface's canonical ABI
isn't ready yet, by a temporary `canon:builtins/*` host bridge. The user
gets a single curated import per domain (`use canon/std/http/HttpServer`,
`use canon/std/fs/File`); the community is free to publish additional
bindings under any path.

Every stdlib module is written in ordinary Canon on top of
[`extern Wasm`](./extern.md) declarations. There is no privileged path.
Anyone can write the same bindings; Canon ships them so users don't have
to.
