use super::base::FileProvider;
use async_trait::async_trait;
use ironworks::{Ironworks, file::File, sqpack::Install};
use std::{path::PathBuf, str::FromStr};

pub struct SqpackFileProvider(Ironworks);

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
    async fn file<T: File>(&self, path: &str) -> Result<T, ironworks::Error> {
        self.0.file(path)
    }
}
