use egui::{Context, Id, Image, Layout, Modal, Sense, Spinner, TextStyle, UiBuilder};

use super::ManagedIcon;

/// Show `icon` over the whole app. Returns true once it has been dismissed.
pub fn icon_modal(ctx: &Context, icon_id: u32, icon: ManagedIcon) -> bool {
    Modal::new(Id::new("icon-modal"))
        .area(Modal::default_area(Id::new(format!(
            "icon-modal-{icon_id}"
        ))))
        .show(ctx, |ui| match icon {
            ManagedIcon::Loaded(icon) => {
                ui.add(Image::new(icon).fit_to_exact_size(ui.available_size()))
            }
            ManagedIcon::Failed(e) => ui.label("Failed to load icon").on_hover_text(e.to_string()),
            ManagedIcon::Loading => {
                let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(rect)
                        .layout(Layout::centered_and_justified(ui.layout().main_dir())),
                    |ui| {
                        ui.add(Spinner::new().size(ui.text_style_height(&TextStyle::Heading) * 3.0))
                    },
                )
                .inner
            }
            ManagedIcon::NotLoaded => ui.label("Icon not loaded"),
        })
        .should_close()
}
