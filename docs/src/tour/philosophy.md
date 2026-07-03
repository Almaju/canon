# Philosophy

> *There is one way to do everything.*

Most modern languages give you ten ways to do the same thing and then ask
you to pick. Canon picks for you. If there's a best practice, it's the
*only* practice — and the compiler enforces it.

## Alphabetical Order, Everywhere

The single most pervasive rule. Whenever ordering is discretionary,
declarations must be in alphabetical order. This applies to:

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

The receiver is referred to as `User` (its type). The parameter is referred
to as `OtherUser` (its type). If you need to disambiguate two parameters of
the same type, you define a newtype — that newtype becomes the
documentation:

```canon
OtherUser = User

User = Birthday * Username
```

The principle: **names lie, types don't**. Forcing every value through a
named type makes the data flow explicit and the documentation structural.

## Effects Are Honest

A function's signature should not lie about what it does. Reading a file
requires a `File` value; making an HTTP request requires a `Url`; running
an HTTP server requires an `HttpServer`. The values that carry the effect
are ordinary arguments:

```canon
get = (Url) -> Result<String, HttpError>

read = (File) -> Result<String, IoError>
```

There is no `unsafe`, no global mutable state, no service locator. A
function that doesn't take a `File` cannot read one — because constructing
a `File` requires a `Path`, which requires a `String`. The type chain *is*
the access control. See [Effects and Values](./effects.md) for the full
story.

## No Comments

There are no comments. Code must speak for itself through types and naming.
If you find yourself wanting to write a comment, the right answer is
usually to introduce a newtype or rename a method.

## Batteries-Included

Canon ships opinionated stdlib modules for the major application
domains — `HttpServer`, `File`, `Url`, `Clock`, `Random`, and more —
each backed by a standard `wasi:*` interface or, where that interface's
canonical ABI isn't ready yet, by a temporary `canon:builtins/*` host
bridge. The user gets a single curated import per domain
(`HttpServer`, `File`, … resolve by name); the community is free to
publish additional bindings under any path.

Under the hood, every stdlib module is written in ordinary Canon on top
of [`extern Wasm`](./extern.md) declarations. There is no privileged
path — anyone can write the same bindings; Canon just ships them so
users don't have to.
