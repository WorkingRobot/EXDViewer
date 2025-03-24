use super::base::FileProvider;
// use crate::web_stream::WebStream;
use async_trait::async_trait;
use ehttp::Request;
use ironworks::file::File;
use std::{io::Cursor, str::FromStr};
use url::Url;

pub struct WebFileProvider(Url);

impl FromStr for WebFileProvider {
    type Err = url::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Url::parse(s)?))
    }
}

#[async_trait(?Send)]
impl FileProvider for WebFileProvider {
    async fn file<T: File>(&self, path: &str) -> Result<T, ironworks::Error> {
        let mut url = self.0.clone();
        url.query_pairs_mut().append_pair("path", path);

        let resp = ehttp::fetch_async(Request::get(url))
            .await
            .map_err(|e| ironworks::Error::NotFound(ironworks::ErrorValue::Other(e)))?;
        if !resp.ok {
            return Err(ironworks::Error::NotFound(ironworks::ErrorValue::Other(
                String::from_utf8_lossy(&resp.bytes).to_string(),
            )));
        }
        T::read(Cursor::new(resp.bytes))

        //let stream = WebStream::new(Request::get(url), true);
        //T::read(stream)
    }
}
