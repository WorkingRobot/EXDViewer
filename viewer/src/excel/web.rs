use super::{base::FileProvider, get_icon_path, get_xivapi_asset_url};
use async_trait::async_trait;
use ehttp::Request;
use either::Either;
use image::RgbaImage;
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
        {
            let mut path_segments = url.path_segments_mut().map_err(|_| {
                ironworks::Error::Invalid(
                    ironworks::ErrorValue::Other("URL".to_string()),
                    "path parsing error".to_string(),
                )
            })?;
            path_segments.push("latest");
            path_segments.extend(path.split('/'));
        }

        let resp = ehttp::fetch_async(Request::get(url))
            .await
            .map_err(|e| ironworks::Error::NotFound(ironworks::ErrorValue::Other(e)))?;
        if !resp.ok {
            return Err(ironworks::Error::NotFound(ironworks::ErrorValue::Other(
                String::from_utf8_lossy(&resp.bytes).to_string(),
            )));
        }
        T::read(Cursor::new(resp.bytes))
    }

    async fn get_icon(
        &self,
        icon_id: u32,
        hires: bool,
    ) -> Result<Either<Url, RgbaImage>, anyhow::Error> {
        let path = get_icon_path(icon_id, hires);
        let url = get_xivapi_asset_url(&path, Some("png"));
        Ok(Either::Left(url))
    }
}
