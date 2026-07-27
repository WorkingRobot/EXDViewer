use std::cell::RefCell;

use history::History;
use matchit::{InsertError, Match, Params};
use path::Path;
use route::Redirect;

use crate::{
    shortcuts::{NAV_BACK, NAV_FORWARD},
    utils::shortcut,
};

pub mod history;
pub mod path;
pub mod route;

pub struct Router<T, H: History = history::DefaultHistory> {
    history: RefCell<H>,
    matcher: matchit::Router<route::Route<T>>,
    unmatched: route::Route<T>,
    title_formatter: Box<dyn Fn(String) -> String>,
    current_title: RefCell<Option<String>>,
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
            current_title: RefCell::new(None),
            last_path: RefCell::new(None),
        }
    }

    pub fn add_route(
        &mut self,
        path: &str,
        on_start: impl Fn(&mut T, &mut egui::Ui, &Path, &Params<'_, '_>) -> Redirect + 'static,
        on_render: impl Fn(&mut T, &mut egui::Ui, &Path, &Params<'_, '_>) + 'static,
        on_title: impl Fn(&T, &Path, &Params<'_, '_>) -> Option<String> + 'static,
    ) -> Result<(), InsertError> {
        let route = route::Route::new(on_start, on_render, on_title);
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
        if shortcut::consume_ui(ui, NAV_BACK)
            && let Err(e) = self.back()
        {
            log::error!("Failed to navigate back: {e}");
        }
        if shortcut::consume_ui(ui, NAV_FORWARD)
            && let Err(e) = self.forward()
        {
            log::error!("Failed to navigate forward: {e}");
        }

        let path = self.current_path();
        let is_new_path = self.last_path.borrow().as_ref() != Some(&path);
        if is_new_path {
            self.last_path.replace(Some(path.clone()));
        }

        let matched = match self.matcher.at(path.path()) {
            Ok(val) => val,
            Err(_) => {
                if let Some(normalized) = self.trailing_slash_target(&path) {
                    if let Err(e) = self.replace(normalized) {
                        log::error!("Failed to normalize trailing slash: {e}");
                    } else {
                        self.ui(state, ui);
                        return;
                    }
                }
                Match {
                    value: &self.unmatched,
                    params: Params::new(),
                }
            }
        };

        if is_new_path {
            log::info!("Navigating to {path}");
            if let Some(redirect) = matched.value.start(state, ui, &path, &matched.params) {
                if let Err(e) = self.replace(redirect) {
                    log::error!("Failed to navigate: {e}");
                } else {
                    self.ui(state, ui);
                }
                return;
            }
        }
        matched.value.render(state, ui, &path, &matched.params);

        self.sync_title(matched.value, state, &path, &matched.params);

        if self.current_path() != path {
            ui.ctx().request_discard("Navigation requested");
        }
    }

    fn trailing_slash_target(&self, path: &Path) -> Option<Path> {
        let trimmed = path.path().trim_end_matches('/');
        if trimmed.is_empty() || trimmed.len() == path.path().len() {
            return None;
        }
        self.matcher.at(trimmed).ok()?;
        Some(path.with_path(trimmed))
    }

    fn sync_title(&self, route: &route::Route<T>, state: &T, path: &Path, params: &Params<'_, '_>) {
        let Some(title) = route.title(state, path, params) else {
            return;
        };
        let mut current = self.current_title.borrow_mut();
        if current.as_deref() == Some(title.as_str()) {
            return;
        }
        self.history
            .borrow_mut()
            .set_title((self.title_formatter)(title.clone()));
        *current = Some(title);
    }
}

#[cfg(test)]
mod tests {
    use super::{Router, history::memory::MemoryHistory, path::Path, route::static_title};

    fn router() -> Router<(), MemoryHistory> {
        let mut router = Router::<(), MemoryHistory>::new(egui::Context::default());
        for route in ["/sheet", "/assets", "/assets/{*path}", "/music/{id}"] {
            router
                .add_route(route, |_, _, _, _| None, |_, _, _, _| {}, static_title("x"))
                .unwrap();
        }
        router
    }

    #[test]
    fn trailing_slash_resolves_to_the_route_it_names() {
        let router = router();
        for (from, to) in [("/sheet/", "/sheet"), ("/assets/", "/assets")] {
            let target = router.trailing_slash_target(&Path::parse(from));
            assert_eq!(target.map(|p| p.path().to_string()).as_deref(), Some(to));
        }
    }

    #[test]
    fn trailing_slash_keeps_the_query_and_fragment() {
        let normalized = router()
            .trailing_slash_target(&Path::parse("/assets/?a=b#frag"))
            .unwrap();
        assert_eq!(normalized.path(), "/assets");
        assert_eq!(normalized.query(), Some("a=b"));
        assert_eq!(normalized.fragment(), Some("frag"));
    }

    #[test]
    fn leaves_alone_what_trimming_would_not_help() {
        let router = router();
        for path in ["/", "/sheet", "/nope/", "/music/"] {
            assert!(
                router.trailing_slash_target(&Path::parse(path)).is_none(),
                "{path}"
            );
        }
    }
}
