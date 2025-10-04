use ehttp::Request;

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
