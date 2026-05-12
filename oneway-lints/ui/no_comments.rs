// SAFETY: well-justified comment passes
pub const A: u8 = 1;

// TODO: ship later
pub const B: u8 = 2;

// see https://example.com/spec
pub const C: u8 = 3;

// fixes #1234
pub const D: u8 = 4;

// SAFETY: first line carries the label
// rest of the group passes by association
pub const E: u8 = 5;

// increment by 1
pub const F: u8 = 6;

// safety: lowercase is not allowed
pub const G: u8 = 7;

/* WHY: block comment with label */
pub const H: u8 = 8;

/* this block has no label */
pub const I: u8 = 9;

fn main() {}
