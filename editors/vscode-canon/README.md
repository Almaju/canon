# Canon for Visual Studio Code

Language support for [Canon](https://github.com/Almaju/canon) — a programming
language that compiles directly to WebAssembly components.

## Features

- **Syntax highlighting** for `.can` files (TextMate grammar)
- **Diagnostics** as you type: lex errors, parse errors, sort-order
  violations, and type-check errors
- **Hover** documentation for built-in types and well-known constructors
- **Go to definition**
- **Formatting** via `canon fmt` (enable format-on-save for the `canon`
  language to get auto-sorted declarations)

All language features are powered by the compiler's built-in language server
(`canon lsp`) — there is no separate LSP binary to install.

## Getting the `canon` binary

The extension resolves the compiler in this order:

1. The `canon.serverPath` setting, if set
2. `canon` on your `PATH`
3. `~/.cargo/bin/canon`
4. A previously downloaded binary

If none is found, the extension offers to **download a prebuilt binary** from
GitHub releases automatically. You can also trigger this manually with the
**Canon: Download Language Server** command, or install the compiler yourself:

```sh
curl -fsSL https://raw.githubusercontent.com/Almaju/canon/main/install.sh | sh
```

## Settings

| Setting | Description |
|---|---|
| `canon.serverPath` | Explicit path to the `canon` binary |
| `canon.trace.server` | LSP trace level (`off` / `messages` / `verbose`) |

## Commands

| Command | Description |
|---|---|
| **Canon: Restart Language Server** | Restart `canon lsp` |
| **Canon: Download Language Server** | Fetch the latest prebuilt binary from GitHub releases |

## About Canon

Canon presents a small surface area — no `let`, no `if`/`else`, no comments,
no local variables. Branching is dispatch on a union. Effects are passed as
capabilities. Wherever ordering is discretionary, the compiler enforces
alphabetical order.

```
Bool = False + True
User = Birthday * Username

main = () -> Unit {
    "hello".print()
}
```

See the [language guide](https://github.com/Almaju/canon) for more.
