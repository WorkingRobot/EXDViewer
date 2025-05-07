use std::rc::Rc;

use ironworks::excel::Language;

use crate::{backend::Backend, utils::IconManager};

#[derive(Clone)]
pub struct GlobalContext(Rc<GlobalContextImpl>);

pub struct GlobalContextImpl {
    ctx: egui::Context,
    backend: Backend,
    language: Language,
    icon_manager: IconManager,
}

impl GlobalContext {
    pub fn new(
        ctx: egui::Context,
        backend: Backend,
        language: Language,
        icon_manager: IconManager,
    ) -> Self {
        Self(Rc::new(GlobalContextImpl {
            ctx,
            backend,
            language,
            icon_manager,
        }))
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.0.ctx
    }

    pub fn backend(&self) -> &Backend {
        &self.0.backend
    }

    pub fn language(&self) -> Language {
        self.0.language
    }

    pub fn icon_manager(&self) -> &IconManager {
        &self.0.icon_manager
    }
}
