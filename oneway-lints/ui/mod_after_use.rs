use std::collections::HashMap;

mod inner {
    pub const X: u32 = 1;
}

fn main() {
    let _: HashMap<u8, u8> = HashMap::new();
    let _ = inner::X;
}
