pub mod async_analysis;
mod wasm;

pub use wasm::component::{vendored_extern_byte_stream_return, vendored_extern_uses_async_value};
pub use wasm::generate;
pub use wasm::generate_wit;
