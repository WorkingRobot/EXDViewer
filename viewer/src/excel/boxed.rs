use super::base::{CachedProvider, ExcelFileProvider};

pub type BoxedExcelProvider = CachedProvider<Box<dyn ExcelFileProvider>>;

impl BoxedExcelProvider {
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new_sqpack(
        value: super::sqpack::SqpackFileProvider,
    ) -> Result<Self, ironworks::Error> {
        CachedProvider::new(
            Box::new(value) as Box<dyn ExcelFileProvider>,
            std::num::NonZeroUsize::new(64).unwrap(),
        )
        .await
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn new_web_sqpack(
        value: super::web_sqpack::WebSqpackFileProvider,
    ) -> Result<Self, ironworks::Error> {
        CachedProvider::new(
            Box::new(value) as Box<dyn ExcelFileProvider>,
            std::num::NonZeroUsize::new(64).unwrap(),
        )
        .await
    }

    pub async fn new_web(value: super::web::WebFileProvider) -> Result<Self, ironworks::Error> {
        CachedProvider::new(
            Box::new(value) as Box<dyn ExcelFileProvider>,
            std::num::NonZeroUsize::new(256).unwrap(),
        )
        .await
    }
}
