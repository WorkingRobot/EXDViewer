use std::{
    cell::RefCell,
    sync::atomic::{AtomicU32, Ordering},
};

use history::History;
use matchit::{InsertError, Params};
use path::Path;

pub mod history;
pub mod path;
mod route;

static ID: AtomicU32 = AtomicU32::new(0);

fn next_id() -> u32 {
    ID.fetch_add(1, Ordering::SeqCst)
}

pub struct Router<T, H: History = history::DefaultHistory> {
    history: RefCell<H>,
    matcher: matchit::Router<route::Route<T>>,
    unmatched: route::Route<T>,
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
            last_path: RefCell::new(None),
        }
    }

    pub fn add_route(
        &mut self,
        path: &str,
        on_start: impl Fn(&mut T, &mut egui::Ui, &Path, &Params<'_, '_>) -> Result<(), Path> + 'static,
        on_render: impl Fn(&mut T, &mut egui::Ui, &Path, &Params<'_, '_>) + 'static,
    ) -> Result<(), InsertError> {
        let route = route::Route::new(on_start, on_render);
        self.matcher.insert(path, route)
    }

    pub fn navigate(&self, path: impl Into<path::Path>) -> anyhow::Result<()> {
        self.history.borrow_mut().push(path.into(), next_id())
    }

    pub fn replace(&self, path: impl Into<path::Path>) -> anyhow::Result<()> {
        self.history.borrow_mut().replace(path.into(), next_id())
    }

    pub fn back(&self) -> anyhow::Result<()> {
        self.history.borrow_mut().back()
    }

    pub fn forward(&self) -> anyhow::Result<()> {
        self.history.borrow_mut().forward()
    }

    pub fn current_path(&self) -> Path {
        self.history.borrow().active_route().0
    }

    pub fn ui(&self, state: &mut T, ui: &mut egui::Ui) {
        let path = self.current_path();
        let is_new_path = self.last_path.borrow().as_ref() != Some(&path);
        if is_new_path {
            self.last_path.replace(Some(path.clone()));
        }
        match self.matcher.at(path.path()) {
            Ok(val) => {
                if is_new_path {
                    if let Err(path) = val.value.start(state, ui, &path, &val.params) {
                        if let Err(e) = self.replace(path) {
                            log::error!("Failed to navigate: {}", e);
                        } else {
                            self.ui(state, ui);
                        }
                        return;
                    }
                }
                val.value.render(state, ui, &path, &val.params);
            }
            Err(_) => self.unmatched.render(state, ui, &path, &Params::new()),
        }
        if self.current_path() != path {
            ui.ctx().request_discard("Navigation requested");
        }
    }
}
