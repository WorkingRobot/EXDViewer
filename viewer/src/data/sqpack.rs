use crate::utils::tex_loader;

use super::{
    FileProvider, build_local_presence, get_icon_path, index_hash, list_url, stream, with_list_id,
};
use crate::utils::fetch_url;
use async_trait::async_trait;
use either::Either;
use image::RgbaImage;
use ironworks::{
    Ironworks,
    sqpack::{Install, SqPack},
};
use std::{path::PathBuf, rc::Rc, str::FromStr};
use url::Url;

pub struct SqpackFileProvider {
    ironworks: Ironworks<Rc<SqPack<Install>>>,
    /// The same resource the `Ironworks` holds. Hash lookups are a SqPack concept, so they need the
    /// concrete type; sharing it keeps one index cache rather than two.
    sqpack: Rc<SqPack<Install>>,
}

impl SqpackFileProvider {
    pub fn new(install_location: &str) -> Self {
        let resource = Install::at_sqpack(PathBuf::from_str(install_location).unwrap());
        let sqpack = Rc::new(SqPack::new(resource));
        Self {
            ironworks: Ironworks::new().with_resource(sqpack.clone()),
            sqpack,
        }
    }
}

#[async_trait(?Send)]
impl FileProvider for SqpackFileProvider {
    async fn read_stream(&self, path: &str) -> anyhow::Result<(Option<String>, Vec<u8>)> {
        let (kind, bytes) = stream(self.sqpack.file(path)?)?;
        Ok((Some(kind), bytes))
    }

    async fn read_stream_by_hash(
        &self,
        repository: u8,
        category: u8,
        hash: u64,
        split: bool,
    ) -> anyhow::Result<(Option<String>, Vec<u8>)> {
        let (kind, bytes) = stream(self.sqpack.file_by_hash(
            repository,
            category,
            index_hash(hash, split),
        )?)?;
        Ok((Some(kind), bytes))
    }

    async fn path_index(&self, api_base: &str) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        let paths = with_list_id(api_base, |id| fetch_url(list_url(api_base, id))).await?;
        let presence = build_local_presence(&self.sqpack, &paths)?;
        Ok((paths, presence))
    }

    async fn get_icon(&self, icon_id: u32, hires: bool) -> anyhow::Result<Either<Url, RgbaImage>> {
        let path = get_icon_path(icon_id, hires);
        let data = tex_loader::read(&self.ironworks, &path)?;
        Ok(Either::Right(data.into_rgba8()))
    }

    async fn exists_many(&self, paths: &[String]) -> anyhow::Result<Vec<bool>> {
        let mut result = Vec::with_capacity(paths.len());
        for path in paths {
            result.push(self.ironworks.exists(path)?);
        }
        Ok(result)
    }
}
