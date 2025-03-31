use super::{cache::CachedProvider, provider::SchemaProvider};

pub type BoxedSchemaProvider = CachedProvider<Box<dyn SchemaProvider>>;

impl BoxedSchemaProvider {
    pub fn new_local(value: super::local::LocalProvider) -> Self {
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
