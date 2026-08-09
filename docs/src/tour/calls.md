# Calls With More Than One Input

A constructor's input is a **product of distinct types**, so an argument
is identified by its type rather than by a position. `Greeting * Name`
is not "first the greeting, then the name" — it is "a greeting and a
name", and the body reads each by its type name.

There is exactly one canonical spelling, enforced like all formatting:
**values flow through pipes, literals are born in the parens.** The
value that already exists leads, and the rest ride along:

```text
Greeting("hi ") -> Line(Name("ada"))
```

Because the components are distinct types, nothing here is a
"receiver": `Line` belongs to `Greeting` and `Name` equally, so an
operation is never trapped inside the one type it happens to start
from.

```canon,run
Greeting = String

Line = String

Name = String

Greeting * Name => Line {
    Greeting
        -> Joined(Name)
        -> Line
}

Unit => Program {
    Greeting("hi ")
        -> Line(Name("ada"))
        -> Print
    Greeting("hello ")
        -> Line(Name("grace"))
        -> Print
}
```
