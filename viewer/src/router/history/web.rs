use std::sync::mpsc::Receiver;

use eframe::wasm_bindgen::{JsCast, prelude::Closure};
use web_sys::{js_sys::Number, window};

use crate::{router::path::Path, utils::JsErr};

use super::{History, HistoryEvent};

pub struct WebHistory {
    base_href: String,
    rx: Receiver<HistoryEvent>,
    history: web_sys::History,
    cb: Closure<dyn FnMut(web_sys::PopStateEvent)>,
}

impl WebHistory {
    pub fn new(base_href: Option<String>, ctx: egui::Context) -> Self {
        let window = window().unwrap();

        let base_href = base_href
            .or_else(|| {
                window
                    .document()
                    .unwrap()
                    .get_elements_by_tag_name("base")
                    .item(0)
                    .map(|base| base.get_attribute("href").unwrap_or_default())
            })
            .unwrap_or_else(|| "/".to_string());
        let base_href = base_href
            .strip_prefix("/")
            .unwrap_or(&base_href)
            .to_string();

        let (tx, rx) = std::sync::mpsc::channel();

        let base_href_clone = base_href.clone();
        let cb = Closure::wrap(Box::new(move |event: web_sys::PopStateEvent| {
            let state = event.state().as_f64().map(|n| n as u32);
            let location = web_sys::window().unwrap().location();
            let full_path = format!(
                "{}{}{}",
                location.pathname().unwrap(),
                location.search().unwrap(),
                location.hash().unwrap()
            );
            let path = full_path
                .strip_prefix(&base_href_clone)
                .unwrap_or("/")
                .to_string();

            tx.send(HistoryEvent {
                location: path.into(),
                state,
            })
            .ok();

            ctx.request_repaint();
        }) as Box<dyn FnMut(_)>);

        window
            .add_event_listener_with_callback("popstate", cb.as_ref().unchecked_ref())
            .unwrap();

        Self {
            base_href,
            rx,
            history: window.history().unwrap(),
            cb,
        }
    }

    fn prefix_path(&self, url: &Path) -> String {
        let location = window().unwrap().location();
        format!("{}{}{}", location.origin().unwrap(), self.base_href, url)
    }
}

impl Drop for WebHistory {
    fn drop(&mut self) {
        window()
            .unwrap()
            .remove_event_listener_with_callback("popstate", self.cb.as_ref().unchecked_ref())
            .unwrap();
    }
}

impl History for WebHistory {
    fn new(ctx: egui::Context) -> Self {
        Self::new(None, ctx)
    }

    fn tick(&mut self) -> Vec<HistoryEvent> {
        self.rx.try_iter().collect()
    }

    fn active_route(&self) -> (Path, Option<u32>) {
        let location = window().unwrap().location();
        let full_path = format!(
            "{}{}{}",
            location.pathname().unwrap(),
            location.search().unwrap(),
            location.hash().unwrap(),
        );

        let path = full_path
            .strip_prefix(&self.base_href)
            .unwrap_or("/")
            .to_string();

        let state = self
            .history
            .state()
            .ok()
            .map(|s| s.as_f64())
            .flatten()
            .map(|n| n as u32);
        (path.into(), state)
    }

    fn push(&mut self, location: Path, state: u32) -> anyhow::Result<()> {
        self.history
            .push_state_with_url(&Number::from(state), "", Some(&self.prefix_path(&location)))
            .map_err(JsErr::from)?;
        Ok(())
    }

    fn replace(&mut self, location: Path, state: u32) -> anyhow::Result<()> {
        self.history
            .replace_state_with_url(&Number::from(state), "", Some(&self.prefix_path(&location)))
            .map_err(JsErr::from)?;
        Ok(())
    }

    fn back(&mut self) -> anyhow::Result<()> {
        self.history.back().map_err(JsErr::from)?;
        Ok(())
    }

    fn forward(&mut self) -> anyhow::Result<()> {
        self.history.back().map_err(JsErr::from)?;
        Ok(())
    }
}
