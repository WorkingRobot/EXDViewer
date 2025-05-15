use ironworks::excel::Language;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Key<K: Serialize + DeserializeOwned + Clone + Send + Sync + 'static> {
    id: &'static str,
    default: K,
}

impl<K: Serialize + DeserializeOwned + Clone + Send + Sync + 'static> Key<K> {
    const fn new(name: &'static str, default: K) -> Self {
        Self { id: name, default }
    }

    pub fn try_get(&self, ctx: &egui::Context) -> Option<K> {
        ctx.data_mut(|d| d.get_persisted::<K>(self.id.into()))
    }

    pub fn get(&self, ctx: &egui::Context) -> K {
        ctx.data_mut(|d| {
            d.get_persisted::<K>(self.id.into())
                .unwrap_or_else(|| self.default.clone())
        })
    }

    pub fn set(&self, ctx: &egui::Context, value: K) {
        ctx.data_mut(|d| d.insert_persisted(self.id.into(), value));
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct TempKey<K: Send + Sync + Clone + 'static> {
    id: &'static str,
    _marker: std::marker::PhantomData<K>,
}

impl<K: Send + Sync + Clone + 'static> TempKey<K> {
    const fn new(name: &'static str) -> Self {
        Self {
            id: name,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn try_get(&self, ctx: &egui::Context) -> Option<K> {
        ctx.data_mut(|d| d.get_temp::<K>(self.id.into()))
    }

    pub fn set(&self, ctx: &egui::Context, value: K) {
        ctx.data_mut(|d| d.insert_temp(self.id.into(), value));
    }

    pub fn take(&self, ctx: &egui::Context) -> Option<K> {
        let ret = self.try_get(ctx);
        if ret.is_some() {
            ctx.data_mut(|d| d.remove::<K>(self.id.into()));
        }
        ret
    }
}

pub const LOGGER_SHOWN: Key<bool> = Key::new("logger-shown", false);
pub const SORTED_BY_OFFSET: Key<bool> = Key::new("sorted-by-offset", false);
pub const BACKEND_CONFIG: Key<Option<BackendConfig>> = Key::new("backend-config", None);
pub const LANGUAGE: Key<Language> = Key::new("language", Language::English);
pub const SHEETS_FILTER: Key<String> = Key::new("sheets-filter", String::new());
pub const SELECTED_SHEET: Key<Option<String>> = Key::new("selected-sheet", None);
pub const MISC_SHEETS_SHOWN: Key<bool> = Key::new("misc-sheets-shown", false);
pub const SCHEMA_EDITOR_WORD_WRAP: Key<bool> = Key::new("schema-editor-word-wrap", false);

pub const TEMP_SCROLL_TO_ROW: TempKey<(u32, Option<u16>)> = TempKey::new("temp-scroll-to-row");
pub const TEMP_SCROLL_TO_COL: TempKey<u16> = TempKey::new("temp-scroll-to-col");
pub const TEMP_HIGHLIGHTED_ROW_NR: TempKey<u64> = TempKey::new("temp-highlighted-row");

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub enum InstallLocation {
    #[cfg(not(target_arch = "wasm32"))]
    Sqpack(String),
    #[cfg(target_arch = "wasm32")]
    Worker(String),
    Web(String),
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub enum SchemaLocation {
    #[cfg(not(target_arch = "wasm32"))]
    Local(String),
    #[cfg(target_arch = "wasm32")]
    Worker(String),
    Web(String),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BackendConfig {
    pub location: InstallLocation,
    pub schema: SchemaLocation,
}
