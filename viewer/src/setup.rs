use std::env::current_dir;

use egui::{Frame, Vec2, WidgetText};

use crate::{
    DEFAULT_API_URL, DEFAULT_SCHEMA_URL,
    data::{AppConfig, InstallLocation, SchemaLocation},
};

pub struct SetupWindow {
    location: InstallLocation,
    schema: SchemaLocation,
}

impl Default for SetupWindow {
    fn default() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let location = ironworks::sqpack::Install::search()
            .and_then(|p| Some(InstallLocation::Sqpack(p.path().to_str()?.to_owned())))
            .unwrap_or(InstallLocation::Web(super::DEFAULT_API_URL.to_string()));

        #[cfg(target_arch = "wasm32")]
        let location = InstallLocation::Web(super::DEFAULT_API_URL.to_string());

        Self {
            location,
            schema: SchemaLocation::Web(super::DEFAULT_SCHEMA_URL.to_string()),
        }
    }
}

impl SetupWindow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_config(config: AppConfig) -> Self {
        Self {
            location: config.location,
            schema: config.schema,
        }
    }

    pub fn draw(
        &mut self,
        ctx: &egui::Context,
        loading_state: Option<Option<&anyhow::Error>>,
    ) -> Option<AppConfig> {
        let resp = egui::Modal::new("setup_modal".into())
            .frame(Frame::window(&ctx.style()))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading("Setup");
                });
                ui.separator();
                let enabled: bool;
                if let Some(Some(err)) = loading_state {
                    ui.label(err.to_string());
                    enabled = true;
                } else if let Some(None) = loading_state {
                    ui.label("Loading...");
                    enabled = false;
                } else {
                    enabled = true;
                }
                ui.add_enabled_ui(enabled, |ui| {
                    egui::containers::Frame::group(ui.style()).show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading("Location");
                        });
                        ui.horizontal(|ui| {
                            #[cfg(not(target_arch = "wasm32"))]
                            if radio(
                                ui,
                                matches!(self.location, InstallLocation::Sqpack(_)),
                                "Local",
                            ) {
                                self.location = InstallLocation::Sqpack(
                                    current_dir()
                                        .ok()
                                        .and_then(|p| Some(p.to_str()?.to_string()))
                                        .unwrap_or("/".to_owned()),
                                );
                            }
                            if radio(ui, matches!(self.location, InstallLocation::Web(_)), "Web") {
                                self.location = InstallLocation::Web(DEFAULT_API_URL.to_string());
                            }
                        });

                        match &mut self.location {
                            #[cfg(not(target_arch = "wasm32"))]
                            InstallLocation::Sqpack(path) => {
                                ui.horizontal(|ui| {
                                    ui.label("Path:");
                                    ui.text_edit_singleline(path);
                                    if ui.button("...").clicked() {
                                        if let Some(picked_path) = rfd::FileDialog::new()
                                            .pick_folder()
                                            .and_then(|d| d.to_str().map(|s| s.to_owned()))
                                        {
                                            *path = picked_path;
                                        }
                                    }
                                });
                            }
                            InstallLocation::Web(url) => {
                                ui.horizontal(|ui| {
                                    ui.label("URL:");
                                    ui.text_edit_singleline(url);
                                });
                            }
                        }
                    });

                    egui::containers::Frame::group(ui.style()).show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading("Schema");
                        });
                        ui.horizontal(|ui| {
                            #[cfg(not(target_arch = "wasm32"))]
                            if radio(ui, matches!(self.schema, SchemaLocation::Local(_)), "Local") {
                                self.schema = SchemaLocation::Local(
                                    current_dir()
                                        .ok()
                                        .and_then(|p| Some(p.to_str()?.to_string()))
                                        .unwrap_or("/".to_owned()),
                                );
                            }
                            if radio(ui, matches!(self.schema, SchemaLocation::Web(_)), "Web") {
                                self.schema = SchemaLocation::Web(DEFAULT_SCHEMA_URL.to_string());
                            }
                        });

                        match &mut self.schema {
                            #[cfg(not(target_arch = "wasm32"))]
                            SchemaLocation::Local(path) => {
                                ui.horizontal(|ui| {
                                    ui.label("Path:");
                                    ui.text_edit_singleline(path);
                                    if ui.button("...").clicked() {
                                        if let Some(picked_path) = rfd::FileDialog::new()
                                            .pick_folder()
                                            .and_then(|d| d.to_str().map(|s| s.to_owned()))
                                        {
                                            *path = picked_path;
                                        }
                                    }
                                });
                            }
                            SchemaLocation::Web(url) => {
                                ui.horizontal(|ui| {
                                    ui.label("URL:");
                                    ui.text_edit_singleline(url);
                                });
                            }
                        }
                    });

                    ui.add_sized(
                        Vec2::new(ui.available_size_before_wrap().x, 0.0),
                        egui::Button::new("Go"),
                    )
                    .clicked()
                })
                .inner
            });

        if resp.inner {
            Some(AppConfig {
                location: self.location.clone(),
                schema: self.schema.clone(),
            })
        } else {
            None
        }
    }
}

fn radio(ui: &mut egui::Ui, selected: bool, text: impl Into<WidgetText>) -> bool {
    let mut resp = ui.radio(selected, text);
    if resp.clicked() && !selected {
        resp.mark_changed();
        true
    } else {
        false
    }
}
