# Branching Is Dispatch

There is no `if`, no `else`, and no `switch`. To branch, pipe a union
value into a group of arms — one per variant, in the union's
(alphabetical) order, **with no wildcard**.

Each arm is a lambda for one variant, and the whole dispatch is an
expression, so every arm produces the same type. When a variant carries
data, the arm names the payload type and the body sees it under that
name.

Exhaustiveness is the entire point: add a variant to a union and every
dispatch that forgot it stops compiling. Delete the `Warn` arm below
and run it — the error tells you exactly what you dropped.

```canon,run
Level = Debug
  + Error
  + Warn

Line = String

Level => Line {
    Level -> (
        * Debug { Line("debug") }
        * Error { Line("error") }
        * Warn { Line("warn") }
    )
}

Unit => Program {
    Warn()
        -> Line
        -> Print
}
```
