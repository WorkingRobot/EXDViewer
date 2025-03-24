use std::{cell::RefCell, num::NonZeroUsize, sync::Arc};

use async_trait::async_trait;

use super::provider::SchemaProvider;

pub struct CachedProvider<T: SchemaProvider>(Arc<CachedProviderImpl<T>>);

impl<T: SchemaProvider> Clone for CachedProvider<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

pub struct CachedProviderImpl<T: SchemaProvider> {
    provider: T,
    cache: RefCell<lru::LruCache<String, String>>,
}

impl<T: SchemaProvider> CachedProvider<T> {
    pub fn new(provider: T, size: NonZeroUsize) -> Self {
        Self(Arc::new(CachedProviderImpl {
            provider,
            cache: RefCell::new(lru::LruCache::new(size)),
        }))
    }
}

#[async_trait(?Send)]
impl<T: SchemaProvider> SchemaProvider for CachedProvider<T> {
    async fn get_schema_text(&self, name: &str) -> anyhow::Result<String> {
        let mut cache = self.0.cache.borrow_mut();
        if let Some(text) = cache.get(name) {
            return Ok(text.clone());
        }
        let text = self.0.provider.get_schema_text(name).await?;
        cache.put(name.to_string(), text.clone());
        Ok(text)
    }

    fn can_save_schemas(&self) -> bool {
        self.0.provider.can_save_schemas()
    }

    fn save_schema_start_dir(&self) -> std::path::PathBuf {
        self.0.provider.save_schema_start_dir()
    }

    fn save_schema(&self, name: &str, text: &str) -> anyhow::Result<()> {
        self.0.provider.save_schema(name, text)?;
        self.0.cache.borrow_mut().pop(name);
        Ok(())
    }
}

#[async_trait(?Send)]
impl SchemaProvider for Box<dyn SchemaProvider> {
    async fn get_schema_text(&self, name: &str) -> anyhow::Result<String> {
        self.as_ref().get_schema_text(name).await
    }

    fn can_save_schemas(&self) -> bool {
        self.as_ref().can_save_schemas()
    }

    fn save_schema_start_dir(&self) -> std::path::PathBuf {
        self.as_ref().save_schema_start_dir()
    }

    fn save_schema(&self, name: &str, text: &str) -> anyhow::Result<()> {
        self.as_ref().save_schema(name, text)
    }
}
