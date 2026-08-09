# Three Symbols

Three symbols carry the whole language, one job each:

| Symbol | Job |
|---|---|
| `=>` | **declares** — every constructor, lambda, and dispatch arm |
| `->` | **executes** — pipes a value through an operation |
| `.` | **reads** — field access, nothing else |

Every callable is a constructor named after the type it produces, so it
needs no name of its own: `Whisper => Loud` reads as *give it a
`Whisper`, get a `Loud`*.

A body is expressions separated by newlines, and the last one is the
return value — no `return`, no semicolons, and no local variables.
Values thread through the pipe instead.

Try adding a second line to the body and watch which one comes back.

```canon,run
Loud = String

Whisper = String

Whisper => Loud {
    Whisper
        -> Uppercased
        -> Loud
}

Unit => Program {
    Whisper("keep it down")
        -> Loud
        -> Print
}
```
