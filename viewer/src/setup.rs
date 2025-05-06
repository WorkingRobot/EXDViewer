use egui::{ComboBox, Frame, Layout, TextEdit, Vec2, WidgetText};

use crate::{
    DEFAULT_API_URL, DEFAULT_SCHEMA_URL,
    data::{AppConfig, InstallLocation, SchemaLocation},
    utils::TrackedPromise,
};

pub struct SetupWindow {
    location: InstallLocation,
    schema: SchemaLocation,
    #[cfg(target_arch = "wasm32")]
    location_promises: SetupPromises,
    #[cfg(target_arch = "wasm32")]
    schema_promises: SetupPromises,
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
            #[cfg(target_arch = "wasm32")]
            location_promises: Default::default(),
            #[cfg(target_arch = "wasm32")]
            schema_promises: Default::default(),
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
            #[cfg(target_arch = "wasm32")]
            location_promises: Default::default(),
            #[cfg(target_arch = "wasm32")]
            schema_promises: Default::default(),
        }
    }

    pub fn draw(
        &mut self,
        ctx: &egui::Context,
        loading_state: Option<Option<&anyhow::Error>>,
    ) -> Option<AppConfig> {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(path) = self.location_promises.take_folder() {
                self.location = InstallLocation::Worker(path);
            }

            if let Some(path) = self.schema_promises.take_folder() {
                self.schema = SchemaLocation::Worker(path);
            }
        }

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
                                    std::env::current_dir()
                                        .ok()
                                        .and_then(|p| Some(p.to_str()?.to_string()))
                                        .unwrap_or("/".to_owned()),
                                );
                            }
                            #[cfg(target_arch = "wasm32")]
                            if radio(
                                ui,
                                matches!(self.location, InstallLocation::Worker(_)),
                                "Local",
                            ) {
                                self.location =
                                    InstallLocation::Worker("Select folder".to_string());
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
                                    ui.with_layout(Layout::right_to_left(egui::Align::Min), |ui| {
                                        if ui
                                            .button("Browse")
                                            .clicked()
                                        {
                                            if let Some(picked_path) = rfd::FileDialog::new()
                                                .pick_folder()
                                                .and_then(|d| d.to_str().map(|s| s.to_owned()))
                                            {
                                                *path = picked_path;
                                            }
                                        }
                                        ui.add(TextEdit::singleline(path).desired_width(ui.available_width()));
                                    });
                                });
                            }

                            #[cfg(target_arch = "wasm32")]
                            InstallLocation::Worker(name) => {
                                ui.horizontal(|ui| {
                                    ui.label("Name:");
                                    ui.with_layout(Layout::right_to_left(egui::Align::Min), |ui| {
                                        if ui.button("Browse").clicked() {
                                            self.location_promises.open_folder_picker(
                                                ui.ctx().clone(),
                                                web_sys::FileSystemPermissionMode::Read,
                                                crate::excel::worker::WorkerFileProvider::add_folder,
                                            );
                                        }
                                        ComboBox::from_id_salt("install_folder")
                                            .selected_text(name.as_str())
                                            .width(ui.available_width())
                                            .show_ui(ui, |ui| {
                                                match self.location_promises.get_folder_list(
                                                ui.ctx().clone(),
                                                crate::excel::worker::WorkerFileProvider::folders,
                                            ) {
                                                None => {
                                                    ui.label("Retrieving...");
                                                }
                                                Some(Err(e)) => {
                                                    ui.label(format!("An error occured: {e}"));
                                                }
                                                Some(Ok(entries)) => {
                                                    if entries.is_empty() {
                                                        ui.label("None");
                                                    }
                                                    else{
                                                    for entry in entries {
                                                        ui.selectable_value(
                                                            name,
                                                            entry.to_string(),
                                                            entry,
                                                        );
                                                    }
                                                }
                                                }
                                            }
                                        });
                                    });
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
                                    std::env::current_dir()
                                        .ok()
                                        .and_then(|p| Some(p.to_str()?.to_string()))
                                        .unwrap_or("/".to_owned()),
                                );
                            }
                            #[cfg(target_arch = "wasm32")]
                            if radio(
                                ui,
                                matches!(self.schema, SchemaLocation::Worker(_)),
                                "Local",
                            ) {
                                self.schema = SchemaLocation::Worker("Select folder".to_string());
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
                                    ui.with_layout(Layout::right_to_left(egui::Align::Min), |ui| {
                                    if ui
                                        .button("Browse")
                                        .clicked()
                                    {
                                        if let Some(picked_path) = rfd::FileDialog::new()
                                            .pick_folder()
                                            .and_then(|d| d.to_str().map(|s| s.to_owned()))
                                        {
                                            *path = picked_path;
                                        }
                                    }
                                    ui.add(TextEdit::singleline(path).desired_width(ui.available_width()));
                                });
                                });
                            }

                            #[cfg(target_arch = "wasm32")]
                            SchemaLocation::Worker(name) => {
                                ui.horizontal(|ui| {
                                    ui.label("Name:");
                                    ui.with_layout(Layout::right_to_left(egui::Align::Min), |ui| {
                                        if ui
                                        .button("Browse")
                                        .clicked() {
                                            self.schema_promises.open_folder_picker(
                                                ui.ctx().clone(),
                                                web_sys::FileSystemPermissionMode::Readwrite,
                                                crate::schema::worker::WorkerProvider::add_folder,
                                            );
                                        }
                                        ComboBox::from_id_salt("schema_folder")
                                        .selected_text(name.as_str())
                                        .width(ui.available_width())
                                        .show_ui(ui, |ui| {
                                            match self.schema_promises.get_folder_list(
                                                ui.ctx().clone(),
                                                crate::schema::worker::WorkerProvider::folders,
                                            ) {
                                                None => {
                                                    ui.label("Retrieving...");
                                                }
                                                Some(Err(e)) => {
                                                    ui.label(format!("An error occured: {e}"));
                                                }
                                                Some(Ok(entries)) => {
                                                    if entries.is_empty() {
                                                        ui.label("None");
                                                    }
                                                    else {
                                                        for entry in entries {
                                                            ui.selectable_value(
                                                                name,
                                                                entry.to_string(),
                                                                entry,
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                        });
                                    });
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

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct SetupPromises {
    selected: Option<TrackedPromise<anyhow::Result<String>>>,
    list: Option<TrackedPromise<anyhow::Result<Vec<String>>>>,
}

#[cfg(target_arch = "wasm32")]
impl SetupPromises {
    fn take_folder(&mut self) -> Option<String> {
        if let Some(result) = self.selected.take_if(|p| p.poll().is_ready()) {
            let result = poll_promise::Promise::from(result).block_and_take();

            self.list.take();
            match result {
                Ok(path) => Some(path),
                Err(e) => {
                    log::error!("Error picking folder: {e}");
                    None
                }
            }
        } else {
            None
        }
    }

    fn open_folder_picker<F: Future<Output = anyhow::Result<String>>>(
        &mut self,
        ctx: egui::Context,
        mode: web_sys::FileSystemPermissionMode,
        store_folder: impl Fn(web_sys::FileSystemDirectoryHandle) -> F + 'static,
    ) {
        use anyhow::anyhow;
        use eframe::wasm_bindgen::JsCast;
        use wasm_bindgen_futures::JsFuture;
        use web_sys::{DirectoryPickerOptions, FileSystemDirectoryHandle};

        let ret = crate::utils::TrackedPromise::spawn_local(ctx, async move {
            let opts = DirectoryPickerOptions::new();
            opts.set_mode(mode);
            let promise = web_sys::window()
                .expect("no window")
                .show_directory_picker_with_options(&opts);
            let promise = promise.map_err(|e| anyhow!("Error picking folder: {e:?}"))?;
            let result = JsFuture::from(promise).await;
            match result {
                Ok(handle) => {
                    let handle = handle
                        .dyn_into::<FileSystemDirectoryHandle>()
                        .map_err(|_| anyhow!("Error casting to FileSystemDirectoryHandle"))?;
                    store_folder(handle).await
                }
                Err(e) => Err(anyhow!("Error picking folder: {e:?}")),
            }
        });
        self.selected = Some(ret);
    }

    fn get_folder_list<F: Future<Output = anyhow::Result<Vec<String>>> + 'static>(
        &mut self,
        ctx: egui::Context,
        future: impl FnOnce() -> F,
    ) -> Option<&anyhow::Result<Vec<String>>> {
        if self.list.is_none() {
            self.list = Some(TrackedPromise::spawn_local(ctx, future()));
        }
        let promise = self.list.as_deref().unwrap();

        promise.ready()
    }
}
