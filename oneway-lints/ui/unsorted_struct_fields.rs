struct User {
    name: String,
    email: String,
    age: u32,
}

struct Sorted {
    age: u32,
    email: String,
    name: String,
}

fn main() {
    let _ = User { age: 0, email: String::new(), name: String::new() };
    let _ = Sorted { age: 0, email: String::new(), name: String::new() };
}
