# The Guide

Switching pages is a message handled by the Canon `update` function.

## The loop
The current page lives in the **model**:

- clicking a nav button sends a message
- `update` returns a new model
- the `view` re-renders

## The view
Each page is its own `.md` file, imported by name:

```
Page => Html {
    Page -> (
        * "guide" => Html { Guide() -> Html }
        * String => Html { Intro() -> Html }
    )
}
```

All of it - the renderer included - is **Canon compiled to WebAssembly**.
