use std::{convert::Infallible, fmt, str::FromStr};

use eframe::wasm_bindgen::{JsCast, JsValue};
use web_sys::js_sys;

pub type JsResult<T> = Result<T, JsErr>;

pub enum JsErr {
    JsError {
        name: String,
        message: String,
        js_to_string: String,
    },
    NotJsError {
        js_to_string: String,
    },
    External {
        value: String,
    },
}

impl fmt::Display for JsErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                JsErr::JsError { js_to_string, .. } => js_to_string,
                JsErr::NotJsError { js_to_string } => js_to_string,
                JsErr::External { value } => value,
            }
        )
    }
}

impl fmt::Debug for JsErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsErr::JsError {
                name,
                message,
                js_to_string,
            } => f
                .debug_struct("JsError")
                .field("name", name)
                .field("message", message)
                .field("js_to_string", js_to_string)
                .finish(),
            JsErr::NotJsError { js_to_string } => f
                .debug_struct("NotJsError")
                .field("js_to_string", js_to_string)
                .finish(),
            JsErr::External { value } => f.debug_struct("External").field("value", value).finish(),
        }
    }
}

impl std::error::Error for JsErr {}

impl From<js_sys::Error> for JsErr {
    fn from(error: js_sys::Error) -> Self {
        JsErr::JsError {
            name: String::from(error.name()),
            message: String::from(error.message()),
            js_to_string: String::from(error.to_string()),
        }
    }
}

impl From<JsValue> for JsErr {
    fn from(value: JsValue) -> Self {
        match value.dyn_into::<js_sys::Error>() {
            Ok(error) => error.into(),
            Err(js_value) => JsErr::NotJsError {
                js_to_string: String::from(js_sys::JsString::from(js_value.clone())),
            },
        }
    }
}

impl FromStr for JsErr {
    type Err = Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(JsErr::External {
            value: String::from(value),
        })
    }
}

impl JsErr {
    pub fn msg(value: impl Into<String>) -> Self {
        JsErr::External {
            value: value.into(),
        }
    }
}
