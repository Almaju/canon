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

```oneway
User = Birthday * Username
```

A union type:

```oneway
Ord = Equal + Greater + Less
```

Function declarations in a file:

```oneway
add    = (User * ...) -> ...
export = (User * ...) -> ...
remove = (User * ...) -> ...
```

Inline error union:

```oneway
read = (File * Path) -> Result<Bytes, IoError + NotFound + PermissionDenied> {
    ...
}
```

Dispatch arms in variant order:

```oneway
ord.(
    Equal   => ...,
    Greater => ...,
    Less    => ...,
)
```

## Checking Without Compiling

```sh
just check path/to/file.ow
```

This runs only the sort-order check, with no codegen. Useful in pre-commit
hooks or as a quick lint while editing.

## Rationale

Ordering is a constant source of bikeshedding and diff noise. By forcing
one canonical order, code reads the same way no matter who wrote it, and
reordering is never a meaningful change.
