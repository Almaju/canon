#[derive(Debug, Clone)]
pub struct User {
    pub name: String,
}

fn main() {
    let _ = User { name: String::new() };
}
