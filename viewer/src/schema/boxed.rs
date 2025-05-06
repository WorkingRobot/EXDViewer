use super::{cache::CachedProvider, provider::SchemaProvider};

pub type BoxedSchemaProvider = CachedProvider<Box<dyn SchemaProvider>>;

impl BoxedSchemaProvider {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new_local(value: super::local::LocalProvider) -> Self {
        CachedProvider::new(
            Box::new(value) as Box<dyn SchemaProvider>,
            std::num::NonZeroUsize::new(10).unwrap(),
        )
    }

    #[cfg(target_arch = "wasm32")]
    pub fn new_worker(value: super::worker::WorkerProvider) -> Self {
        CachedProvider::new(
            Box::new(value) as Box<dyn SchemaProvider>,
            std::num::NonZeroUsize::new(10).unwrap(),
        )
    }

    pub fn new_web(value: super::web::WebProvider) -> Self {
        CachedProvider::new(
            Box::new(value) as Box<dyn SchemaProvider>,
            std::num::NonZeroUsize::new(256).unwrap(),
        )
    }
}
