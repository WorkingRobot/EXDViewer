use std::cell::RefCell;

use history::History;
use matchit::{InsertError, Match, Params};
use path::Path;
use route::RouteResponse;

pub mod history;
pub mod path;
pub mod route;

pub struct Router<T, H: History = history::DefaultHistory> {
    history: RefCell<H>,
    matcher: matchit::Router<route::Route<T>>,
    unmatched: route::Route<T>,
    title_formatter: Box<dyn Fn(String) -> String>,
    last_path: RefCell<Option<Path>>,
}

impl<T, H: History> Router<T, H> {
    pub fn new(ctx: egui::Context) -> Self {
        Self::from_history(H::new(ctx))
    }

    pub fn from_history(history: H) -> Self {
        Self {
            history: RefCell::new(history),
            matcher: matchit::Router::new(),
            unmatched: route::Route::unmatched(),
            title_formatter: Box::new(|title| title),
            last_path: RefCell::new(None),
        }
    }

    pub fn add_route(
        &mut self,
        path: &str,
        on_start: impl Fn(&mut T, &mut egui::Ui, &Path, &Params<'_, '_>) -> RouteResponse + 'static,
        on_render: impl Fn(&mut T, &mut egui::Ui, &Path, &Params<'_, '_>) + 'static,
    ) -> Result<(), InsertError> {
        let route = route::Route::new(on_start, on_render);
        self.matcher.insert(path, route)
    }

    pub fn set_title_formatter(&mut self, formatter: impl Fn(String) -> String + 'static) {
        self.title_formatter = Box::new(formatter);
    }

    pub fn navigate(&self, path: impl Into<path::Path>) -> anyhow::Result<()> {
        self.history.borrow_mut().push(path.into())
    }

    pub fn replace(&self, path: impl Into<path::Path>) -> anyhow::Result<()> {
        self.history.borrow_mut().replace(path.into())
    }

    pub fn back(&self) -> anyhow::Result<()> {
        self.history.borrow_mut().back()
    }

    pub fn forward(&self) -> anyhow::Result<()> {
        self.history.borrow_mut().forward()
    }

    pub fn base_url(&self) -> String {
        self.history.borrow().base_url()
    }

    pub fn full_url(&self) -> String {
        format!("{}{}", self.base_url(), self.current_path())
    }

    pub fn current_path(&self) -> Path {
        self.history.borrow().active_route()
    }

    pub fn ui(&self, state: &mut T, ui: &mut egui::Ui) {
        let path = self.current_path();
        let is_new_path = self.last_path.borrow().as_ref() != Some(&path);
        if is_new_path {
            self.last_path.replace(Some(path.clone()));
        }

        let matched = match self.matcher.at(path.path()) {
            Ok(val) => val,
            Err(_) => Match {
                value: &self.unmatched,
                params: Params::new(),
            },
        };

        if is_new_path {
            log::info!("Navigating to {path}");
            match matched.value.start(state, ui, &path, &matched.params) {
                RouteResponse::Title(title) => {
                    self.history
                        .borrow_mut()
                        .set_title((self.title_formatter)(title));
                }
                RouteResponse::Redirect(path) => {
                    if let Err(e) = self.replace(path) {
                        log::error!("Failed to navigate: {}", e);
                    } else {
                        self.ui(state, ui);
                    }
                    return;
                }
            }
        }
        matched.value.render(state, ui, &path, &matched.params);

        if self.current_path() != path {
            ui.ctx().request_discard("Navigation requested");
        }
    }
}
