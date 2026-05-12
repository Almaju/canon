fn classify(n: i32) -> &'static str {
    match n {
        2 => "two",
        1 => "one",
        _ => "other",
    }
}

fn main() {
    let _ = classify(1);
}
