use ironworks::excel::Language;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::utils::CodeTheme;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Key<K: Serialize + DeserializeOwned + Clone + Send + Sync + 'static> {
    id: &'static str,
    _marker: std::marker::PhantomData<K>,
}

impl<K: Serialize + DeserializeOwned + Clone + Send + Sync + 'static> Key<K> {
    const fn new(name: &'static str) -> Self {
        Self {
            id: name,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn try_get(&self, ctx: &egui::Context) -> Option<K> {
        ctx.data_mut(|d| d.get_persisted::<K>(self.id.into()))
    }

    pub fn get_or_insert(&self, ctx: &egui::Context, func: impl FnOnce() -> K) -> K {
        ctx.data_mut(|d| {
            d.get_persisted_mut_or_insert_with(self.id.into(), func)
                .clone()
        })
    }

    pub fn set(&self, ctx: &egui::Context, value: K) {
        ctx.data_mut(|d| d.insert_persisted(self.id.into(), value));
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DefaultedKey<K: Serialize + DeserializeOwned + Clone + Send + Sync + 'static> {
    imp: Key<K>,
    default: K,
}

impl<K: Serialize + DeserializeOwned + Clone + Send + Sync + 'static> DefaultedKey<K> {
    const fn new(name: &'static str, default: K) -> Self {
        Self {
            imp: Key::new(name),
            default,
        }
    }

    pub fn try_get(&self, ctx: &egui::Context) -> Option<K> {
        self.imp.try_get(ctx)
    }

    pub fn get(&self, ctx: &egui::Context) -> K {
        self.imp.get_or_insert(ctx, || self.default.clone())
    }

    pub fn set(&self, ctx: &egui::Context, value: K) {
        self.imp.set(ctx, value);
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

type DKey<K> = DefaultedKey<K>;
pub const LOGGER_SHOWN: DKey<bool> = DKey::new("logger-shown", false);
pub const SORTED_BY_OFFSET: DKey<bool> = DKey::new("sorted-by-offset", false);
pub const ALWAYS_HIRES: DKey<bool> = DKey::new("always-hires", false);
pub const DISPLAY_FIELD_SHOWN: DKey<bool> = DKey::new("display-field-shown", true);
pub const BACKEND_CONFIG: DKey<Option<BackendConfig>> = DKey::new("backend-config", None);
pub const LANGUAGE: DKey<Language> = DKey::new("language", Language::English);
pub const SHEETS_FILTER: DKey<String> = DKey::new("sheets-filter", String::new());
pub const SELECTED_SHEET: DKey<Option<String>> = DKey::new("selected-sheet", None);
pub const MISC_SHEETS_SHOWN: DKey<bool> = DKey::new("misc-sheets-shown", false);
pub const SCHEMA_EDITOR_VISIBLE: DKey<bool> = DKey::new("schema-editor-visible", false);
pub const SCHEMA_EDITOR_WORD_WRAP: DKey<bool> = DKey::new("schema-editor-word-wrap", false);
pub const SCHEMA_EDITOR_ERRORS_SHOWN: DKey<bool> = DKey::new("schema-editor-errors-shown", false);

pub const CODE_SYNTAX_THEME: Key<CodeTheme> = Key::new("syntax-theme");

pub const TEMP_SCROLL_TO: TempKey<((u32, Option<u16>), u16)> = TempKey::new("temp-scroll-to");
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
