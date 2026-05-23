#[allow(dead_code)]
async fn oneway_http_get_text(url: Url) -> Result<String, reqwest::Error> {
    let resp = reqwest::get::<&str>(url.as_ref()).await?;
    resp.text().await
}
