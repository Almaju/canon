#[allow(dead_code)]
fn oneway_json_parse<T: serde::de::DeserializeOwned>(s: String) -> Result<T, serde_json::Error> {
    serde_json::from_str(&s)
}
