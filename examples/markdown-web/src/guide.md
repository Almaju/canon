# The Guide

Switching pages is a message handled by the Canon `update` function.

## The loop
The current page lives in the *model*:

1. clicking a nav button sends a message
2. `update` returns a new model
3. the `view` re-renders

## A note
> Every page here is its own `.md` file, imported by name and rendered to
> HTML by the standard library - no JavaScript, no bundler.

## The view
Content is chosen by dispatching on the model:

```
Page => Html {
    Page -> (
        * "guide" => Html { Guide() -> Html }
        * String => Html { Intro() -> Html }
    )
}
```

All of it - **renderer included** - is Canon compiled to WebAssembly.
