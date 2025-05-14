use matchit::Params;

use super::path::Path;

pub struct Route<T> {
    on_start: Box<dyn Fn(&mut T, &mut egui::Ui, &Path, &Params<'_, '_>) -> Result<(), Path>>,
    on_render: Box<dyn Fn(&mut T, &mut egui::Ui, &Path, &Params<'_, '_>)>,
}

impl<T> Route<T> {
    pub fn new(
        on_start: impl Fn(&mut T, &mut egui::Ui, &Path, &Params<'_, '_>) -> Result<(), Path> + 'static,
        on_render: impl Fn(&mut T, &mut egui::Ui, &Path, &Params<'_, '_>) + 'static,
    ) -> Self {
        Self {
            on_start: Box::new(on_start),
            on_render: Box::new(on_render),
        }
    }

    pub fn unmatched() -> Self {
        Self::new(
            |_, _, _, _| Ok(()),
            |_, ui, _, _| {
                ui.vertical_centered_justified(|ui| {
                    ui.heading("Not Found");
                    ui.label("The requested page was not found.");
                    ui.label("Please check the URL and try again.");
                });
            },
        )
    }

    pub fn start(
        &self,
        state: &mut T,
        ui: &mut egui::Ui,
        path: &Path,
        params: &Params<'_, '_>,
    ) -> Result<(), Path> {
        (self.on_start)(state, ui, path, params)
    }

    pub fn render(&self, state: &mut T, ui: &mut egui::Ui, path: &Path, params: &Params<'_, '_>) {
        (self.on_render)(state, ui, path, params)
    }
}
