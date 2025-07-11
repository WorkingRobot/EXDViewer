use crate::utils::tex_loader;

use super::{base::FileProvider, get_icon_path};
use async_trait::async_trait;
use either::Either;
use image::RgbaImage;
use ironworks::{
    Ironworks,
    file::File,
    sqpack::{Install, SqPack},
};
use std::{path::PathBuf, str::FromStr};
use url::Url;

pub struct SqpackFileProvider(Ironworks<SqPack<Install>>);

impl SqpackFileProvider {
    pub fn new(install_location: &str) -> Self {
        let resource = Install::at_sqpack(PathBuf::from_str(install_location).unwrap());
        let resource = ironworks::sqpack::SqPack::new(resource);
        let ironworks = Ironworks::new().with_resource(resource);
        Self(ironworks)
    }
}

#[async_trait(?Send)]
impl FileProvider for SqpackFileProvider {
    async fn file<T: File>(&self, path: &str) -> anyhow::Result<T> {
        Ok(self.0.file(path)?)
    }

    async fn get_icon(&self, icon_id: u32, hires: bool) -> anyhow::Result<Either<Url, RgbaImage>> {
        let path = get_icon_path(icon_id, hires);
        let data = tex_loader::read(&self.0, &path)?;
        Ok(Either::Right(data.into_rgba8()))
    }
}
