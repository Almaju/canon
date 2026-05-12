pub struct User;

impl User {
    pub fn name(&self) -> &str {
        ""
    }
    pub fn new() -> Self {
        Self
    }
}

fn main() {
    let _ = User::new().name();
}
