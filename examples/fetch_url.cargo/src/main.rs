#[allow(dead_code)]
async fn oneway_http_get_text(url: Url) -> Result<String, reqwest::Error> {
    let resp = reqwest::get::<&str>(url.as_ref()).await?;
    resp.text().await
}

impl AsRef<str> for Url {
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

#[allow(dead_code)]
fn oneway_url_parse(s: String) -> Result<Url, url::ParseError> {
    url::Url::parse(&s).map(|_| Url(s))
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct Url(String);

pub type InvalidUrl = url::ParseError;

pub type HttpError = reqwest::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("{}", oneway_http_get_text(oneway_url_parse("https://example.com".to_string())?).await?);
    Ok(())
}

