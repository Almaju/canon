# Sort Order

The guiding rule: wherever ordering is discretionary, the compiler
enforces alphabetical order.

## Where It Applies

| Construct                              | Order                                       |
|----------------------------------------|---------------------------------------------|
| Components of a product type           | Alphabetical                                |
| Variants of a union type               | Alphabetical                                |
| Function declarations in a file        | Alphabetical                                |
| Trait composition (`Show = A * B`)     | Alphabetical                                |
| Error union inside `Result<T, E>`      | Alphabetical                                |
| `use` statements at the top of a file  | Alphabetical                                |
| Arms of a dispatch                     | Order of the union's variants (alphabetical) |

## Examples

A product type:

```canon
User = Birthday * Username
```

A union type:

```canon
Ord = Equal + Greater + Less
```

Function declarations in a file:

```canon
add    = (User * ...) -> ...
export = (User * ...) -> ...
remove = (User * ...) -> ...
```

Inline error union:

```canon
read = (File * Path) -> Result<Bytes, IoError + NotFound + PermissionDenied> {
    ...
}
```

Dispatch arms in variant order:

```canon
ord.(
    * (Equal) -> R { ... }
    * (Greater) -> R { ... }
    * (Less) -> R { ... }
)
```

## Auto-Fixing

The canonical order is mechanical, so you never sort by hand:

```sh
canon fmt path/to/file.can
```

`canon fmt` sorts `use` imports, type definitions, function
declarations, and dispatch arms into canonical order (the entry point —
`main` or the HTTP handler — keeps its position; it is a distinguished
role, not a regular free function). The checker's ordering errors are
the backstop for code that bypassed the formatter.

## Checking Without Compiling

```sh
canon check path/to/file.can
```

This runs only the sort-order check, with no codegen. Useful as a quick
lint while editing.

## Rationale

Ordering is a constant source of bikeshedding and diff noise. By forcing
one canonical order, code reads the same way no matter who wrote it, and
reordering is never a meaningful change.
