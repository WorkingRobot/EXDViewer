use egui::{Frame, Layout, Modal, Sense, UiBuilder, Vec2, WidgetText};

use crate::{
    DEFAULT_API_URL, DEFAULT_SCHEMA_URL,
    backend::Backend,
    settings::{BACKEND_CONFIG, BackendConfig, InstallLocation, SchemaLocation},
    utils::{PromiseKind, UnsendPromise},
};

#[cfg(target_arch = "wasm32")]
use crate::utils::TrackedPromise;
#[cfg(target_arch = "wasm32")]
use anyhow::anyhow;

pub struct SetupWindow {
    location: InstallLocation,
    schema: SchemaLocation,
    is_startup: bool,
    #[cfg(target_arch = "wasm32")]
    location_promises: SetupPromises,
    #[cfg(target_arch = "wasm32")]
    schema_promises: SetupPromises,
    setup_promise: Option<UnsendPromise<anyhow::Result<(Backend, BackendConfig)>>>,
    display_error: Option<anyhow::Error>,
}

impl SetupWindow {
    pub fn from_blank(is_startup: bool) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let location = ironworks::sqpack::Install::search()
            .and_then(|p| Some(InstallLocation::Sqpack(p.path().to_str()?.to_owned())))
            .unwrap_or(InstallLocation::Web(super::DEFAULT_API_URL.to_string()));

        #[cfg(target_arch = "wasm32")]
        let location = InstallLocation::Web(super::DEFAULT_API_URL.to_string());

        Self {
            location,
            schema: SchemaLocation::Web(super::DEFAULT_SCHEMA_URL.to_string()),
            is_startup,
            #[cfg(target_arch = "wasm32")]
            location_promises: Default::default(),
            #[cfg(target_arch = "wasm32")]
            schema_promises: Default::default(),
            setup_promise: None,
            display_error: None,
        }
    }

    pub fn from_config(ctx: &egui::Context, is_startup: bool) -> Self {
        if let Some(Some(config)) = BACKEND_CONFIG.try_get(ctx) {
            Self {
                location: config.location,
                schema: config.schema,
                is_startup,
                #[cfg(target_arch = "wasm32")]
                location_promises: Default::default(),
                #[cfg(target_arch = "wasm32")]
                schema_promises: Default::default(),
                setup_promise: None,
                display_error: None,
            }
        } else {
            Self::from_blank(is_startup)
        }
    }

    pub fn draw(&mut self, ctx: &egui::Context) -> Option<(Backend, BackendConfig)> {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(path) = self.location_promises.take_folder() {
                self.location = InstallLocation::Worker(path);
            }

            if let Some(path) = self.schema_promises.take_folder() {
                self.schema = SchemaLocation::Worker(path);
            }
        }

        let show_inner = |ui: &mut egui::Ui| {
            ui.vertical_centered(|ui| {
                ui.heading("Setup");
            });
            ui.separator();

            let enabled: bool;
            match self.setup_promise.take().map(|p| p.try_take()) {
                None => {
                    enabled = true;
                }
                Some(Err(promise)) => {
                    self.setup_promise = Some(promise);
                    enabled = false;
                    ui.label("Loading...");
                }
                Some(Ok(Ok(backend))) => {
                    return Some(backend);
                }
                Some(Ok(Err(err))) => {
                    log::error!("Setup Error: {err}");
                    self.display_error = Some(err);
                    enabled = true;
                }
            }

            if let Some(err) = &self.display_error {
                ui.label(err.to_string());
            } else {
                ui.label("Please select the location of the game files and schema.");
            }

            let is_go_clicked = ui
                .add_enabled_ui(enabled, |ui| {
                    Frame::group(ui.style()).show(ui, |ui| {
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
                                        if ui.button("Browse").clicked() {
                                            if let Some(picked_path) = rfd::FileDialog::new()
                                                .pick_folder()
                                                .and_then(|d| d.to_str().map(|s| s.to_owned()))
                                            {
                                                *path = picked_path;
                                            }
                                        }
                                        ui.add(
                                            egui::TextEdit::singleline(path)
                                                .desired_width(ui.available_width()),
                                        );
                                    });
                                });
                            }

                            #[cfg(target_arch = "wasm32")]
                            InstallLocation::Worker(name) => {
                                ui.horizontal(|ui| {
                                    ui.label("Name:");
                                    ui.with_layout(Layout::right_to_left(egui::Align::Min), |ui| {
                                        if ui.button("Browse").clicked() {
                                            use crate::excel::worker::WorkerFileProvider;

                                            self.location_promises.open_folder_picker(
                                                web_sys::FileSystemPermissionMode::Read,
                                                WorkerFileProvider::add_folder,
                                            );
                                        }
                                        egui::ComboBox::from_id_salt("install_folder")
                                            .selected_text(name.as_str())
                                            .width(ui.available_width())
                                            .show_ui(ui, |ui| {
                                                match self.location_promises.get_folder_list(
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

                            InstallLocation::Web(url) => {
                                ui.horizontal(|ui| {
                                    ui.label("URL:");
                                    ui.text_edit_singleline(url);
                                });
                            }
                        }
                    });

                    Frame::group(ui.style()).show(ui, |ui| {
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
                                        if ui.button("Browse").clicked() {
                                            if let Some(picked_path) = rfd::FileDialog::new()
                                                .pick_folder()
                                                .and_then(|d| d.to_str().map(|s| s.to_owned()))
                                            {
                                                *path = picked_path;
                                            }
                                        }
                                        ui.add(
                                            egui::TextEdit::singleline(path)
                                                .desired_width(ui.available_width()),
                                        );
                                    });
                                });
                            }

                            #[cfg(target_arch = "wasm32")]
                            SchemaLocation::Worker(name) => {
                                ui.horizontal(|ui| {
                                    ui.label("Name:");
                                    ui.with_layout(Layout::right_to_left(egui::Align::Min), |ui| {
                                        if ui.button("Browse").clicked() {
                                            self.schema_promises.open_folder_picker(
                                                web_sys::FileSystemPermissionMode::Readwrite,
                                                crate::schema::worker::WorkerProvider::add_folder,
                                            );
                                        }
                                        egui::ComboBox::from_id_salt("schema_folder")
                                            .selected_text(name.as_str())
                                            .width(ui.available_width())
                                            .show_ui(ui, |ui| {
                                                match self.schema_promises.get_folder_list(
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
                                                        } else {
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
                .inner;

            if is_go_clicked || self.is_startup {
                self.is_startup = false;
                if self.setup_promise.is_none() {
                    let location = self.location.clone();
                    let schema = self.schema.clone();
                    self.setup_promise = Some(UnsendPromise::new(async move {
                        let config = BackendConfig { location, schema };
                        Backend::new(config.clone())
                            .await
                            .map(|backend| (backend, config))
                    }));
                }
            }
            None
        };

        Modal::default_area("setup-modal".into())
            .show(ctx, |ui| {
                ui.scope_builder(UiBuilder::new().sense(Sense::CLICK | Sense::DRAG), |ui| {
                    egui::containers::Frame::window(ui.style())
                        .show(ui, show_inner)
                        .inner
                })
                .inner
            })
            .inner
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
        if let Some(result) = self.selected.take_if(|p| p.ready()) {
            let result = result.block_and_take();

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
        mode: web_sys::FileSystemPermissionMode,
        store_folder: impl Fn(web_sys::FileSystemDirectoryHandle) -> F + 'static,
    ) {
        use eframe::wasm_bindgen::JsCast;
        use wasm_bindgen_futures::JsFuture;
        use web_sys::{DirectoryPickerOptions, FileSystemDirectoryHandle};

        let ret = TrackedPromise::spawn_local(async move {
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
        future: impl FnOnce() -> F,
    ) -> Option<&anyhow::Result<Vec<String>>> {
        if self.list.is_none() {
            self.list = Some(TrackedPromise::spawn_local(future()));
        }
        self.list.as_ref().unwrap().try_get()
    }
}
