use ironworks::excel::Language;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub enum InstallLocation {
    #[cfg(not(target_arch = "wasm32"))]
    Sqpack(String),
    Web(String),
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub enum SchemaLocation {
    #[cfg(not(target_arch = "wasm32"))]
    Local(String),
    Web(String),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub location: InstallLocation,
    pub schema: SchemaLocation,
}

#[derive(Serialize, Deserialize)]
pub struct AppState {
    pub config: Option<AppConfig>,
    pub language: Language,
    pub current_filter: String,
    pub current_sheet: Option<String>,
    pub are_misc_sheets_shown: bool,
    pub schema_editor_word_wrap: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            config: None,
            language: Language::English,
            current_filter: String::new(),
            current_sheet: None,
            are_misc_sheets_shown: false,
            schema_editor_word_wrap: false,
        }
    }
}
