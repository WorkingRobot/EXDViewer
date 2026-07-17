use egui::{Color32, Hyperlink, RichText};

pub fn draw(ctx: &egui::Context, open: &mut bool) {
    egui::Window::new("About")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(380.0)
        .show(ctx, |ui| {
            let body = egui::TextStyle::Body.resolve(ui.style()).size;
            let title_size = body * 1.7;
            let version_size = body * 1.25;
            let subheader_size = body * 1.25;

            ui.horizontal(|ui| {
                ui.add(
                    egui::Image::new(egui::include_image!("../assets/icon-small.png"))
                        .fit_to_exact_size(egui::vec2(108.0, 108.0)),
                );
                ui.vertical(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.add(
                            egui::Hyperlink::from_label_and_url(
                                RichText::new("EXDViewer").size(title_size).strong(),
                                crate::REPO_URL,
                            )
                            .open_in_new_tab(true),
                        );
                        ui.label(
                            RichText::new(format!(
                                "v{} {}",
                                crate::build::PKG_VERSION,
                                to_title_case(crate::build::BUILD_RUST_CHANNEL)
                            ))
                            .size(version_size),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{} · {}",
                                crate::build::BUILD_TIME,
                                crate::build::BUILD_TARGET_ARCH,
                            ))
                            .small()
                            .weak(),
                        );
                        centered_inline(ui, "By Asriel", |ui| {
                            ui.label("By ");
                            ui.add(
                                Hyperlink::from_label_and_url("Asriel", crate::AUTHOR_URL)
                                    .open_in_new_tab(true),
                            );
                        });
                        centered_inline(ui, "Support me on Ko-fi!", |ui| {
                            ui.label("Support me on ");
                            ui.add(
                                Hyperlink::from_label_and_url("Ko-fi", crate::KOFI_URL)
                                    .open_in_new_tab(true),
                            );
                            ui.label("!");
                        });
                    });
                });
            });

            ui.separator();
            ui.add_space(6.0);

            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Special Thanks").size(subheader_size));
            });
            ui.add_space(4.0);

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label("Thank you to all ");
                ui.add(
                    Hyperlink::from_label_and_url("EXDSchema", crate::SCHEMA_REPO_URL)
                        .open_in_new_tab(true),
                );
                ui.label(" contributors for your schema definitions.");
            });

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label("Thank you to ");
                ui.add(
                    Hyperlink::from_label_and_url("ackwell", crate::ACKWELL_URL)
                        .open_in_new_tab(true),
                );
                ui.label(" for ");
                ui.add(
                    Hyperlink::from_label_and_url("ironworks", crate::IRONWORKS_URL)
                        .open_in_new_tab(true),
                );
                ui.label(".");
            });

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label("Built with ");
                ui.add(
                    Hyperlink::from_label_and_url("egui", crate::EGUI_URL).open_in_new_tab(true),
                );
                ui.label(" and ");
                ui.add(
                    Hyperlink::from_label_and_url("eframe", crate::EFRAME_URL)
                        .open_in_new_tab(true),
                );
                ui.label(".");
            });
        });
}

pub fn centered_inline(ui: &mut egui::Ui, measure: &str, add: impl FnOnce(&mut egui::Ui)) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let width = ui
        .painter()
        .layout_no_wrap(measure.to_owned(), font, Color32::PLACEHOLDER)
        .size()
        .x;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(((ui.available_width() - width) * 0.5).max(0.0));
        add(ui);
    });
}

fn to_title_case(s: &str) -> String {
    s.chars().next().map_or_else(
        || String::new(),
        |first| first.to_uppercase().collect::<String>() + &s[first.len_utf8()..],
    )
}
