mod wasm;

pub use wasm::component::{vendored_extern_returns_byte_stream, vendored_extern_uses_async_value};
pub use wasm::generate;
pub use wasm::generate_wit;
