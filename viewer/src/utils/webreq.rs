use ehttp::{Method, Request};

pub struct HttpResponse {
    pub status: u16,
    pub ok: bool,
    pub bytes: Vec<u8>,
}

impl HttpResponse {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

pub async fn request(
    method: &str,
    url: impl ToString,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> anyhow::Result<HttpResponse> {
    let mut req = Request::get(url);
    req.method = Method::parse(method).map_err(|e| anyhow::anyhow!("invalid HTTP method: {e}"))?;
    if let Some(body) = body {
        req.body = body;
    }
    for (key, value) in headers {
        req.headers.insert(*key, *value);
    }

    let resp = ehttp::fetch_async(req)
        .await
        .map_err(|msg| anyhow::anyhow!(msg))?;

    Ok(HttpResponse {
        status: resp.status,
        ok: resp.ok,
        bytes: resp.bytes,
    })
}

pub async fn fetch_url(url: impl ToString) -> anyhow::Result<Vec<u8>> {
    let resp = ehttp::fetch_async(Request::get(url))
        .await
        .map_err(|msg| anyhow::anyhow!(msg))?;

    if !resp.ok {
        anyhow::bail!(
            "Response not OK ({}{}{}): {}",
            resp.status,
            if resp.status_text.is_empty() { "" } else { " " },
            resp.status_text,
            String::from_utf8_lossy(&resp.bytes)
        );
    }

    Ok(resp.bytes)
}

pub async fn fetch_url_str(url: impl ToString) -> anyhow::Result<String> {
    let bytes = fetch_url(url).await?;
    Ok(String::from_utf8(bytes)?)
}
