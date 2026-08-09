# Dispatch on Literals

Strings and integers dispatch by equality. Literal arms can never cover
every value, so the last arm is a **mandatory catch-all** naming the
scrutinee's type — the compiler will not let you forget the default.

This one construct is Canon's route table, its switch statement, and
its parser. An HTTP handler is a `Route => Body` arrow with the URL
path piped into arms exactly like these; nothing registers a route,
because the dispatch *is* the routing table.

```canon,run
Body = String

Route = String

Route => Body {
    Route -> (
        * "/notes" => Body { Body("every note") }
        * "/notes/1" => Body { Body("the first note") }
        * String => Body { Body("not found") }
    )
}

Unit => Program {
    Route("/notes/1")
        -> Body
        -> Print
    Route("/nope")
        -> Body
        -> Print
}
```
