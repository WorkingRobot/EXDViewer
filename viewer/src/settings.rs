use std::{collections::HashMap, sync::Arc};

use egui::ThemePreference;
use ironworks::excel::Language;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::utils::{CodeTheme, ColorTheme, GameVersion};

pub trait Keyable: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {}

impl<K: Serialize + DeserializeOwned + Clone + Send + Sync + 'static> Keyable for K {}

#[derive(Debug, Clone, Copy)]
enum RetrievalMethod {
    Persisted,
    Temporary,
}

impl RetrievalMethod {
    pub fn try_get<K: Keyable>(self, ctx: &egui::Context, id: egui::Id) -> Option<K> {
        match self {
            RetrievalMethod::Persisted => ctx.data_mut(|d| d.get_persisted::<_>(id)),
            RetrievalMethod::Temporary => ctx.data(|d| d.get_temp::<_>(id)),
        }
    }

    pub fn get_or_insert<K: Keyable>(
        self,
        ctx: &egui::Context,
        id: egui::Id,
        func: impl FnOnce() -> K,
    ) -> K {
        match self {
            RetrievalMethod::Persisted => {
                ctx.data_mut(|d| d.get_persisted_mut_or_insert_with(id, func).clone())
            }
            RetrievalMethod::Temporary => {
                ctx.data_mut(|d| d.get_temp_mut_or_insert_with(id, func).clone())
            }
        }
    }

    pub fn remove<K: Keyable>(self, ctx: &egui::Context, id: egui::Id) {
        ctx.data_mut(|d| d.remove::<K>(id));
    }

    pub fn take<K: Keyable>(self, ctx: &egui::Context, id: egui::Id) -> Option<K> {
        self.try_get(ctx, id).inspect(|_| self.remove::<K>(ctx, id))
    }

    pub fn set<K: Keyable>(self, ctx: &egui::Context, id: egui::Id, value: K) {
        match self {
            RetrievalMethod::Persisted => ctx.data_mut(|d| d.insert_persisted(id, value)),
            RetrievalMethod::Temporary => ctx.data_mut(|d| d.insert_temp(id, value)),
        }
    }

    pub fn use_with<K: Keyable, T>(
        self,
        ctx: &egui::Context,
        id: egui::Id,
        insert_with: impl FnOnce() -> K,
        func: impl FnOnce(&mut K) -> T,
    ) -> T {
        match self {
            RetrievalMethod::Persisted => {
                ctx.data_mut(|d| func(d.get_persisted_mut_or_insert_with(id, insert_with)))
            }
            RetrievalMethod::Temporary => {
                ctx.data_mut(|d| func(d.get_temp_mut_or_insert_with(id, insert_with)))
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BaseKey<K: Keyable, const TEMP: bool> {
    id: &'static str,
    _marker: std::marker::PhantomData<K>,
}

impl<K: Keyable, const TEMP: bool> BaseKey<K, TEMP> {
    const fn new(name: &'static str) -> Self {
        Self {
            id: name,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn try_get(&self, ctx: &egui::Context) -> Option<K> {
        Self::method().try_get(ctx, self.id.into())
    }

    pub fn get_or_insert(&self, ctx: &egui::Context, func: impl FnOnce() -> K) -> K {
        Self::method().get_or_insert(ctx, self.id.into(), func)
    }

    pub fn set(&self, ctx: &egui::Context, value: K) {
        Self::method().set(ctx, self.id.into(), value);
    }

    pub fn use_with<T>(
        &self,
        ctx: &egui::Context,
        insert_with: impl FnOnce() -> K,
        func: impl FnOnce(&mut K) -> T,
    ) -> T {
        Self::method().use_with(ctx, self.id.into(), insert_with, func)
    }

    pub fn take(&self, ctx: &egui::Context) -> Option<K> {
        Self::method().take(ctx, self.id.into())
    }

    pub fn remove(&self, ctx: &egui::Context) {
        Self::method().remove::<K>(ctx, self.id.into());
    }

    fn method() -> RetrievalMethod {
        if TEMP {
            RetrievalMethod::Temporary
        } else {
            RetrievalMethod::Persisted
        }
    }
}

pub struct FuncKey<K: Keyable, const TEMP: bool, P> {
    imp: BaseKey<K, TEMP>,
    preflight: fn(&egui::Context) -> P,
    insert_with: fn(&egui::Context, P) -> K,
}

impl<K: Keyable, const TEMP: bool> FuncKey<K, TEMP, ()> {
    pub const fn new(name: &'static str, insert_with: fn(&egui::Context, ()) -> K) -> Self {
        Self {
            imp: BaseKey::new(name),
            preflight: |_| (),
            insert_with,
        }
    }
}

impl<K: Keyable, const TEMP: bool, P> FuncKey<K, TEMP, P> {
    // Required to help prevent deadlocking when calling ctx.data() and similar methods.
    const fn new_with_preflight(
        name: &'static str,
        preflight: fn(&egui::Context) -> P,
        insert_with: fn(&egui::Context, P) -> K,
    ) -> Self {
        Self {
            imp: BaseKey::new(name),
            preflight,
            insert_with,
        }
    }

    pub fn try_get(&self, ctx: &egui::Context) -> Option<K> {
        self.imp.try_get(ctx)
    }

    pub fn get(&self, ctx: &egui::Context) -> K {
        let r = (self.preflight)(ctx);
        self.imp.get_or_insert(ctx, || (self.insert_with)(ctx, r))
    }

    pub fn set(&self, ctx: &egui::Context, value: K) {
        self.imp.set(ctx, value);
    }

    pub fn use_with<T>(&self, ctx: &egui::Context, func: impl FnOnce(&mut K) -> T) -> T {
        let r = (self.preflight)(ctx);
        self.imp.use_with(ctx, || (self.insert_with)(ctx, r), func)
    }
}

pub struct DefaultedKey<K: Keyable, const TEMP: bool> {
    imp: BaseKey<K, TEMP>,
    default: K,
}

impl<K: Keyable, const TEMP: bool> DefaultedKey<K, TEMP> {
    const fn new(name: &'static str, default: K) -> Self {
        Self {
            imp: BaseKey::new(name),
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

    pub fn use_with<T>(&self, ctx: &egui::Context, func: impl FnOnce(&mut K) -> T) -> T {
        self.imp.use_with(ctx, || self.default.clone(), func)
    }
}

pub type Key<K> = BaseKey<K, false>;
pub type FKey<K, P = ()> = FuncKey<K, false, P>;
pub type DKey<K> = DefaultedKey<K, false>;

pub type TempKey<K> = BaseKey<K, true>;
pub type TempFKey<K, P = ()> = FuncKey<K, true, P>;
pub type TempDKey<K> = DefaultedKey<K, true>;

pub const LOGGER_SHOWN: DKey<bool> = DKey::new("logger-shown", false);
pub const SORTED_BY_OFFSET: DKey<bool> = DKey::new("sorted-by-offset", false);
pub const ALWAYS_HIRES: DKey<bool> = DKey::new("always-hires", false);
pub const DISPLAY_FIELD_SHOWN: DKey<bool> = DKey::new("display-field-shown", true);
pub const BACKEND_CONFIG: DKey<Option<BackendConfig>> = DKey::new("backend-config", None);
pub const LANGUAGE: DKey<Language> = DKey::new("language", Language::English);
pub const SHEETS_FILTER: DKey<String> = DKey::new("sheets-filter", String::new());
pub const SHEET_FILTERS: FKey<HashMap<String, String>> =
    FKey::new("sheet-filters", |_, _| HashMap::new());
pub const SELECTED_SHEET: DKey<Option<String>> = DKey::new("selected-sheet", None);
pub const MISC_SHEETS_SHOWN: DKey<bool> = DKey::new("misc-sheets-shown", false);
pub const SCHEMA_EDITOR_VISIBLE: DKey<bool> = DKey::new("schema-editor-visible", false);
pub const SCHEMA_EDITOR_WORD_WRAP: DKey<bool> = DKey::new("schema-editor-word-wrap", false);
pub const SCHEMA_EDITOR_ERRORS_SHOWN: DKey<bool> = DKey::new("schema-editor-errors-shown", false);

pub const COLOR_THEME: FKey<ColorTheme, ThemePreference> = FKey::new_with_preflight(
    "color-theme",
    |ctx| ctx.options(|opt| opt.theme_preference),
    |_, preference| preference.into(),
);
pub const CODE_SYNTAX_THEME: FKey<CodeTheme, Arc<egui::Style>> = FKey::new_with_preflight(
    "syntax-theme",
    |ctx| ctx.style(),
    |_, style| CodeTheme {
        theme: if style.visuals.dark_mode {
            "base16-mocha.dark"
        } else {
            "Solarized (light)"
        }
        .to_owned(),
        font_id: egui::FontId::monospace(egui::TextStyle::Monospace.resolve(&style).size),
    },
);

pub const TEMP_SCROLL_TO: TempKey<((u32, Option<u16>), u16)> = TempKey::new("temp-scroll-to");
pub const TEMP_HIGHLIGHTED_ROW: TempKey<(u32, Option<u16>)> = TempKey::new("temp-highlighted-row");

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub enum InstallLocation {
    #[cfg(not(target_arch = "wasm32"))]
    Sqpack(String),
    #[cfg(target_arch = "wasm32")]
    Worker(String),
    Web(String, Option<GameVersion>),
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
