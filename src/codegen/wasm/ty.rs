//! WASM-level type representation for compiled Canon expressions.
//!
//! `Ty` is what a compiled expression leaves on the WASM stack — the
//! codegen's runtime notion of a value's shape, distinct from the AST's
//! `TypeExpr` (the source-level notion). The `Named*` variants carry the
//! Canon type name so method dispatch can find the right user-defined
//! function.
//!
//! `IndirectReturnShape` describes the memory-based return convention the
//! canonical ABI uses when a result's flat representation exceeds
//! `MAX_FLAT_RESULTS = 1`.
use wasm_encoder::ValType;

/// What a compiled expression leaves on the WASM stack.
///
/// The `Named*` variants carry the Canon type name so method dispatch can
/// find the right user-defined function.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Ty {
    I64,              // Int and Int-aliases
    F64,              // Float and Float-aliases
    I32,              // Bool / raw tag
    Str,              // String (anonymous)
    NamedStr(String), // String alias (e.g. Greeting)  — 2 stack values (ptr, len)
    Ptr,              // anonymous heap ptr
    NamedPtr(String), // named heap ptr — union / product / Option / Result / List
    /// `NamedPtr` to a union whose Ok *and* Err arms carry a
    /// `String`-aliased payload. The three names are:
    ///
    ///   - `0`: the union type name (e.g. `"Result"`), used for variant
    ///     dispatch and method lookups on the wrapper itself.
    ///   - `1`: the Ok-payload type name (e.g. `"Url"` for
    ///     `Result<Url, InvalidUrl>`), used when `?` extracts the payload
    ///     so subsequent method calls dispatch against the right typed
    ///     alias.
    ///   - `2`: the Err-payload type name (e.g. `"InvalidUrl"`), used by
    ///     dispatch arms to type the bound variable in an `Err(e) =>` arm.
    ///
    /// In-memory layout matches `NamedPtr` plus a String payload:
    /// `[tag i32, ptr i32, len i32]` at offsets `0, 4, 8`. The two payload
    /// arms share the same `(ptr, len)` slots — the discriminant decides
    /// which type the bytes belong to.
    NamedPtrStr(String, String, String),
    List, // List<T>: 2 stack values (ptr: i32, len: i32)
    Unit, // no stack values
}

impl Ty {
    /// WASM value types occupied on the stack.
    pub(crate) fn val_types(&self) -> Vec<ValType> {
        match self {
            Ty::I64 => vec![ValType::I64],
            Ty::F64 => vec![ValType::F64],
            Ty::I32 | Ty::Ptr => vec![ValType::I32],
            Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => vec![ValType::I32],
            Ty::Str | Ty::NamedStr(_) | Ty::List => vec![ValType::I32, ValType::I32],
            Ty::Unit => vec![],
        }
    }

    /// The Canon type name, if known (used for method dispatch).
    pub(crate) fn canon_name(&self) -> Option<&str> {
        match self {
            Ty::NamedStr(n) | Ty::NamedPtr(n) | Ty::NamedPtrStr(n, _, _) => Some(n.as_str()),
            _ => None,
        }
    }

    pub(crate) fn is_str_like(&self) -> bool {
        matches!(self, Ty::Str | Ty::NamedStr(_))
    }
}

/// Shape of an indirect (memory-based) return value. The canonical ABI uses
/// indirect return whenever the result's flat representation exceeds
/// `MAX_FLAT_RESULTS = 1`. The caller allocates a return area and passes its
/// pointer as a trailing `i32` parameter; the host writes the result there
/// and the caller decodes it after the call.
#[derive(Clone, Debug)]
pub(crate) enum IndirectReturnShape {
    /// Bare `string` return. Return area: 8 bytes, `(i32 ptr, i32 len)` at
    /// offsets 0 and 4. After the call we push the pair as `Ty::Str`.
    String,
    /// `result<string-alias, string-alias>` return where both arms are
    /// `String` or any user alias of `String` (e.g. `File`, `IoError`,
    /// `Url`, `HttpError`). Return area: 12 bytes — byte 0 holds the WIT
    /// discriminant (0=ok, 1=err); bytes 4–7 the payload ptr; bytes 8–11
    /// the payload len. After the call the codegen flips the discriminant
    /// Canon's alphabetical convention (Err=0, Ok=1) and pushes the
    /// area pointer as `Ty::NamedPtrStr(union, ok_name, err_name)`. The
    /// three names preserve Canon-level types through `?` and dispatch
    /// so subsequent method calls find their externs (e.g. `.read()`
    /// after `Path(…).File()?`) and the Err arm of a `match` can type
    /// the bound payload (e.g. `Err(e) =>` where `e: IoError`).
    ResultStringString { ok_name: String, err_name: String },
    /// `option<string>` return. Return area: 12 bytes — byte 0 the
    /// discriminant (0=none, 1=some), bytes 4–7 the payload ptr, 8–11
    /// the payload len. Decoded into a fresh Canon `Option` struct
    /// (i32 tag at +0, payload at +4/+8) so ordinary
    /// `(None, Some<String>)` dispatch works.
    OptionString,
    /// `list<string>` return. Return area: 8 bytes — (i32 list ptr,
    /// i32 element count). The canonical-ABI element layout (8-byte
    /// stride, i32 ptr + i32 len per element) is byte-identical to
    /// Canon's `List<String>` representation, so the pair is pushed
    /// directly as `Ty::List`.
    ListString,
    /// A record whose fields are all scalar primitives (e.g.
    /// `wasi:clocks/system_clock#now`'s `instant`). The host writes
    /// the canonical record layout into the ret area; the decode
    /// copies each field into a fresh Canon product struct (the
    /// bindgen renders the record as `Product = ProductFieldA *
    /// ProductFieldB` with `Int`-newtype fields), widening narrow
    /// ints to i64 on the way.
    ScalarRecord {
        /// WIT type name in kebab (`"instant"`) — the component-level
        /// record type is exported under this name.
        wit_name: String,
        /// Canon product type name (`"Instant"`).
        product: String,
        /// Per-field decode info, in WIT declaration order.
        fields: Vec<RecordField>,
        /// Canonical size of the record (ret-area allocation).
        size: u32,
    },
}

/// One field of a `ScalarRecord` indirect return.
#[derive(Clone, Debug)]
pub(crate) struct RecordField {
    /// WIT field name in kebab (`"nanoseconds"`).
    pub(crate) wit_name: String,
    /// Canon product field name (`"InstantNanoseconds"`).
    pub(crate) canon_name: String,
    pub(crate) prim: wasm_encoder::PrimitiveValType,
    /// Byte offset within the canonical record layout.
    pub(crate) offset: u32,
}

impl IndirectReturnShape {
    /// Size of the return area in bytes (must be a multiple of 4).
    pub(crate) fn return_area_size(&self) -> u32 {
        match self {
            IndirectReturnShape::String => 8,
            IndirectReturnShape::ResultStringString { .. } => 12,
            IndirectReturnShape::OptionString => 12,
            IndirectReturnShape::ListString => 8,
            IndirectReturnShape::ScalarRecord { size, .. } => (*size).max(4),
        }
    }
}

/// Size in bytes of the ret-area an async-lowered call writes its result
/// into. The layout matches what the canonical ABI's async lower
/// expects: a single packed value at offset 0, aligned to its natural
/// boundary. Returns a multiple of 4 so the bump allocator's 4-byte
/// alignment is sufficient.
pub(crate) fn ret_area_size_for(ty: &Ty) -> u32 {
    match ty {
        Ty::Str | Ty::NamedStr(_) => 8, // (i32 ptr, i32 len)
        Ty::List => 8,                  // (i32 ptr, i32 len)
        Ty::I64 | Ty::F64 => 8,         // 8-byte scalar
        Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) => 4,
        // Tagged-string unions (Result<Ok-str, Err-str>): tag + ptr + len.
        Ty::NamedPtrStr(_, _, _) => 12,
        Ty::Unit => 0,
    }
}
