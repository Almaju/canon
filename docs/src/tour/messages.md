# Commands Are Messages

What do you call an operation that takes a `Map` and returns a `Map`?
In Canon, nothing — functions have no names, and the types of such an
operation cannot name it either: insert, remove, and update all share
`Map * String => Map`. So a **command** — an arrow that returns one of
its own inputs — takes exactly one other input, its **message**: a type
declared for the operation, holding whatever it needs.

```text
Insert = Key * Value

Remove = String

Map * Insert => Map { ... }

Map * Remove => Map { ... }
```

A command is applied by piping the value into its message. The message
is built from what rides in the parentheses, and the result is the
receiver's type, so chaining is free:

```canon,run
Unit => Program {
    Map()
        -> Insert(Key("b") * Value("2"))
        -> Insert(Key("a") * Value("1"))
        -> Remove("b")
        -> Keys
        -> Json
        -> Print
}
```

The message names what the command does, and it is data: a `Remove` can
be built, stored in a list, and applied later, which is what makes a web
app's update a fold over its messages
([Worlds](./worlds.md)). A message with no payload is a `Unit` newtype
(`Clear = Unit`, applied as `-> Clear`), and a message is never a *part*
of the value it applies to — `Key` is a part of `Map`, so `Map * Key =>
Map` is rejected: the name must be the operation's own.

Operations that produce something *new* keep naming it by its type, as
every other constructor does: `Map => Length`, `Map * Key =>
Option<Value>`. Shared vocabulary needs no coordination either way —
`Map`, `Set`, `String`, and `List` each declare the same `Length = Int`
and contribute their own arrow to it, and `Map` and `Set` share the one
`Remove = String` message.
