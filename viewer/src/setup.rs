use egui::{Frame, Layout, Modal, Sense, TextEdit, UiBuilder, Vec2, WidgetText};

use crate::{
    DEFAULT_API_URL,
    backend::Backend,
    excel::web::{VersionInfo, WebFileProvider},
    schema::web::WebProvider,
    settings::{BACKEND_CONFIG, BackendConfig, InstallLocation, SchemaLocation},
    utils::{ConvertiblePromise, GameVersion, PromiseKind, TrackedPromise, UnsendPromise},
};

#[cfg(target_arch = "wasm32")]
use crate::worker::WorkerDirectory;

type VersionPromise<T> = ConvertiblePromise<TrackedPromise<anyhow::Result<T>>, Option<T>>;
type VersionPromiseHolder<K, T> = Option<(K, VersionPromise<T>)>;

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

    web_version_promise: VersionPromiseHolder<String, VersionInfo>,
    github_branch_promise: VersionPromiseHolder<(String, String), Vec<GameVersion>>,
}

impl SetupWindow {
    pub fn from_blank(is_startup: bool) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let location = ironworks::sqpack::Install::search()
            .and_then(|p| Some(InstallLocation::Sqpack(p.path().to_str()?.to_owned())))
            .unwrap_or(InstallLocation::Web(
                super::DEFAULT_API_URL.to_string(),
                None,
            ));

        #[cfg(target_arch = "wasm32")]
        let location = InstallLocation::Web(super::DEFAULT_API_URL.to_string(), None);

        Self {
            location,
            schema: SchemaLocation::Github(
                (
                    super::DEFAULT_GITHUB_REPO.0.to_string(),
                    super::DEFAULT_GITHUB_REPO.1.to_string(),
                ),
                None,
            ),
            is_startup,
            #[cfg(target_arch = "wasm32")]
            location_promises: Default::default(),
            #[cfg(target_arch = "wasm32")]
            schema_promises: Default::default(),
            setup_promise: None,
            display_error: None,
            web_version_promise: None,
            github_branch_promise: None,
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
                web_version_promise: None,
                github_branch_promise: None,
            }
        } else {
            Self::from_blank(is_startup)
        }
    }

    pub fn draw(&mut self, ctx: &egui::Context) -> Option<(Backend, BackendConfig)> {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(handle) = self.location_promises.take_folder() {
                self.location = InstallLocation::Worker(handle.0.name());
            }

            if let Some(handle) = self.schema_promises.take_folder() {
                self.schema = SchemaLocation::Worker(handle.0.name());
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
                            ui.columns_const(|[col_0, col_1]| {
                                #[cfg(not(target_arch = "wasm32"))]
                                if radio(
                                    col_0,
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
                                    col_0,
                                    matches!(self.location, InstallLocation::Worker(_)),
                                    "Local",
                                ) {
                                    self.location =
                                        InstallLocation::Worker("Select folder".to_string());
                                }
                                if radio(
                                    col_1,
                                    matches!(self.location, InstallLocation::Web(_, _)),
                                    "Web",
                                ) {
                                    self.location =
                                        InstallLocation::Web(DEFAULT_API_URL.to_string(), None);
                                }
                            })
                        });

                        match &mut self.location {
                            #[cfg(not(target_arch = "wasm32"))]
                            InstallLocation::Sqpack(path) => {
                                ui.horizontal(|ui| {
                                    ui.label("Path:");
                                    ui.with_layout(Layout::right_to_left(egui::Align::Min), |ui| {
                                        if ui.button("Browse").clicked()
                                            && let Some(picked_path) = rfd::FileDialog::new()
                                                .pick_folder()
                                                .and_then(|d| d.to_str().map(|s| s.to_owned()))
                                        {
                                            *path = picked_path;
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
                                use crate::excel::worker::WorkerFileProvider;
                                use web_sys::FileSystemPermissionMode;

                                if !*IS_DIRECTORY_PICKER_SUPPORTED {
                                    draw_unsupported_directory_picker(ui);
                                } else {
                                    ui.horizontal(|ui| {
                                        ui.label("Name:");
                                        ui.with_layout(
                                            Layout::right_to_left(egui::Align::Min),
                                            |ui| {
                                                if ui.button("Browse").clicked() {
                                                    self.location_promises.open_folder_picker(
                                                        FileSystemPermissionMode::Read,
                                                        WorkerFileProvider::add_folder,
                                                    );
                                                }
                                                egui::ComboBox::from_id_salt("install_folder")
                                                    .selected_text(name.as_str())
                                                    .width(ui.available_width())
                                                    .show_ui(ui, |ui| {
                                                        match self
                                                            .location_promises
                                                            .get_folder_list(
                                                                WorkerFileProvider::folders,
                                                            ) {
                                                            None => {
                                                                ui.label("Retrieving...");
                                                            }
                                                            Some(Err(e)) => {
                                                                ui.label(format!(
                                                                    "An error occured: {e}"
                                                                ));
                                                            }
                                                            Some(Ok(entries)) => {
                                                                if entries.is_empty() {
                                                                    ui.label("None");
                                                                } else {
                                                                    for entry in entries {
                                                                        ui.selectable_value(
                                                                            name,
                                                                            entry.0.name(),
                                                                            entry.0.name(),
                                                                        );
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    });
                                            },
                                        );
                                    });
                                }
                            }

                            InstallLocation::Web(url, version) => {
                                ui.horizontal(|ui| {
                                    ui.label("URL:");
                                    ui.add(
                                        TextEdit::singleline(url)
                                            .desired_width(ui.available_width()),
                                    );
                                });

                                if !url.is_empty()
                                    && self
                                        .web_version_promise
                                        .as_ref()
                                        .is_none_or(|v| v.0 != *url)
                                {
                                    let url = url.clone();
                                    self.web_version_promise = Some((
                                        url.clone(),
                                        ConvertiblePromise::new_promise(
                                            TrackedPromise::spawn_local(async move {
                                                WebFileProvider::get_versions(&url).await
                                            }),
                                        ),
                                    ));
                                }

                                ui.horizontal(|ui| {
                                    ui.label("Version:");

                                    if let Some((_, promise)) = &mut self.web_version_promise {
                                        if let Some(versions) = promise.get_mut(|r| match r {
                                            Ok(vers) => {
                                                self.display_error = None;
                                                Some(vers)
                                            }
                                            Err(e) => {
                                                log::error!("Error fetching versions: {e}");
                                                self.display_error = Some(e);
                                                None
                                            }
                                        }) {
                                            if let Some(versions) = versions {
                                                egui::ComboBox::from_id_salt("setup_version")
                                                    .selected_text(version.as_ref().map_or_else(
                                                        || format!("Latest ({})", versions.latest),
                                                        |v| v.to_string(),
                                                    ))
                                                    .width(ui.available_width())
                                                    .show_ui(ui, |ui| {
                                                        ui.selectable_value(
                                                            version,
                                                            None,
                                                            format!("Latest ({})", versions.latest),
                                                        );
                                                        for entry in versions.versions.iter() {
                                                            ui.selectable_value(
                                                                version,
                                                                Some(entry.clone()),
                                                                entry.to_string(),
                                                            );
                                                        }
                                                    });
                                            } else {
                                                ui.label("Failed to load versions");
                                            }
                                        } else {
                                            ui.label("Loading versions...");
                                        }
                                    } else {
                                        ui.label("No versions available");
                                    }
                                });
                            }
                        }
                    });

                    Frame::group(ui.style()).show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading("Schema");
                        });
                        ui.horizontal(|ui| {
                            ui.columns_const(|[col_0, col_1, col_2]| {
                                #[cfg(not(target_arch = "wasm32"))]
                                if radio(
                                    col_0,
                                    matches!(self.schema, SchemaLocation::Local(_)),
                                    "Local",
                                ) {
                                    self.schema = SchemaLocation::Local(
                                        std::env::current_dir()
                                            .ok()
                                            .and_then(|p| Some(p.to_str()?.to_string()))
                                            .unwrap_or("/".to_owned()),
                                    );
                                }
                                #[cfg(target_arch = "wasm32")]
                                if radio(
                                    col_0,
                                    matches!(self.schema, SchemaLocation::Worker(_)),
                                    "Local",
                                ) {
                                    self.schema =
                                        SchemaLocation::Worker("Select folder".to_string());
                                }
                                if radio(
                                    col_1,
                                    matches!(self.schema, SchemaLocation::Github(_, _)),
                                    "GitHub",
                                ) {
                                    self.schema = SchemaLocation::Github(
                                        (
                                            super::DEFAULT_GITHUB_REPO.0.to_string(),
                                            super::DEFAULT_GITHUB_REPO.1.to_string(),
                                        ),
                                        None,
                                    );
                                }
                                if radio(
                                    col_2,
                                    matches!(self.schema, SchemaLocation::Web(_)),
                                    "Web",
                                ) {
                                    self.schema =
                                        SchemaLocation::Web(super::DEFAULT_SCHEMA_URL.to_string());
                                }
                            })
                        });

                        match &mut self.schema {
                            #[cfg(not(target_arch = "wasm32"))]
                            SchemaLocation::Local(path) => {
                                ui.horizontal(|ui| {
                                    ui.label("Path:");
                                    ui.with_layout(Layout::right_to_left(egui::Align::Min), |ui| {
                                        if ui.button("Browse").clicked()
                                            && let Some(picked_path) = rfd::FileDialog::new()
                                                .pick_folder()
                                                .and_then(|d| d.to_str().map(|s| s.to_owned()))
                                        {
                                            *path = picked_path;
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
                                use crate::schema::worker::WorkerProvider;
                                use web_sys::FileSystemPermissionMode;

                                if !*IS_DIRECTORY_PICKER_SUPPORTED {
                                    draw_unsupported_directory_picker(ui);
                                } else {
                                    ui.horizontal(|ui| {
                                        ui.label("Name:");
                                        ui.with_layout(
                                            Layout::right_to_left(egui::Align::Min),
                                            |ui| {
                                                if ui.button("Browse").clicked() {
                                                    self.schema_promises.open_folder_picker(
                                                        FileSystemPermissionMode::Readwrite,
                                                        WorkerProvider::add_folder,
                                                    );
                                                }
                                                egui::ComboBox::from_id_salt("schema_folder")
                                                    .selected_text(name.as_str())
                                                    .width(ui.available_width())
                                                    .show_ui(ui, |ui| {
                                                        match self.schema_promises.get_folder_list(
                                                            WorkerProvider::folders,
                                                        ) {
                                                            None => {
                                                                ui.label("Retrieving...");
                                                            }
                                                            Some(Err(e)) => {
                                                                ui.label(format!(
                                                                    "An error occured: {e}"
                                                                ));
                                                            }
                                                            Some(Ok(entries)) => {
                                                                if entries.is_empty() {
                                                                    ui.label("None");
                                                                } else {
                                                                    for entry in entries {
                                                                        ui.selectable_value(
                                                                            name,
                                                                            entry.0.name(),
                                                                            entry.0.name(),
                                                                        );
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    });
                                            },
                                        );
                                    });
                                }
                            }

                            SchemaLocation::Github((owner, repo), version) => {
                                ui.horizontal(|ui| {
                                    ui.columns_const(|[col_owner, col_repo]| {
                                        col_owner.horizontal(|ui| {
                                            ui.label("Owner:");
                                            ui.add(
                                                TextEdit::singleline(owner)
                                                    .desired_width(ui.available_width()),
                                            );
                                        });
                                        col_repo.horizontal(|ui| {
                                            ui.label("Repo:");
                                            ui.add(
                                                TextEdit::singleline(repo)
                                                    .desired_width(ui.available_width()),
                                            );
                                        });
                                    });
                                });

                                if !owner.is_empty()
                                    && !repo.is_empty()
                                    && !self
                                        .github_branch_promise
                                        .as_ref()
                                        .is_some_and(|v| &v.0.0 == owner && &v.0.1 == repo)
                                {
                                    let owner = owner.clone();
                                    let repo = repo.clone();
                                    self.github_branch_promise = Some((
                                        (owner.clone(), repo.clone()),
                                        ConvertiblePromise::new_promise(
                                            TrackedPromise::spawn_local(async move {
                                                WebProvider::fetch_github_repository(&owner, &repo)
                                                    .await
                                            }),
                                        ),
                                    ));
                                }

                                ui.horizontal(|ui| {
                                    ui.label("Version:");

                                    if let Some((_, promise)) = &mut self.github_branch_promise {
                                        if let Some(versions) = promise.get_mut(|r| match r {
                                            Ok(vers) => {
                                                self.display_error = None;
                                                Some(vers)
                                            }
                                            Err(e) => {
                                                log::error!("Error fetching versions: {e}");
                                                self.display_error = Some(e);
                                                None
                                            }
                                        }) {
                                            if let Some(versions) = versions {
                                                egui::ComboBox::from_id_salt(
                                                    "setup_github_version",
                                                )
                                                .selected_text(version.as_ref().map_or_else(
                                                    || "Latest".to_string(),
                                                    |v| v.to_string(),
                                                ))
                                                .width(ui.available_width())
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(
                                                        version,
                                                        None,
                                                        "Latest".to_string(),
                                                    );
                                                    for entry in versions.iter() {
                                                        ui.selectable_value(
                                                            version,
                                                            Some(entry.clone()),
                                                            entry.to_string(),
                                                        );
                                                    }
                                                });
                                            } else {
                                                ui.label("Failed to load versions");
                                            }
                                        } else {
                                            ui.label("Loading versions...");
                                        }
                                    } else {
                                        ui.label("No versions available");
                                    }
                                });
                            }

                            SchemaLocation::Web(url) => {
                                ui.horizontal(|ui| {
                                    ui.label("URL:");
                                    ui.add(
                                        TextEdit::singleline(url)
                                            .desired_width(ui.available_width()),
                                    );
                                });
                            }
                        }
                    });

                    ui.add_enabled_ui(self.can_go(), |ui| {
                        ui.add_sized(
                            Vec2::new(ui.available_size_before_wrap().x, 0.0),
                            egui::Button::new("Go"),
                        )
                        .clicked()
                    })
                    .inner
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

    fn can_go(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        if !*IS_DIRECTORY_PICKER_SUPPORTED
            && (matches!(self.location, InstallLocation::Worker(_))
                || matches!(self.schema, SchemaLocation::Worker(_)))
        {
            return false;
        }

        if matches!(self.location, InstallLocation::Web(_, _))
            && self
                .web_version_promise
                .as_ref()
                .is_none_or(|f| f.1.try_get().map_or(true, |v| v.is_none()))
        {
            return false;
        }
        if matches!(self.schema, SchemaLocation::Github(_, _))
            && self
                .github_branch_promise
                .as_ref()
                .is_none_or(|f| f.1.try_get().map_or(true, |v| v.is_none()))
        {
            return false;
        }

        true
    }
}

fn radio(ui: &mut egui::Ui, selected: bool, text: impl Into<WidgetText>) -> bool {
    let mut resp = ui
        .vertical_centered_justified(|ui| ui.radio(selected, text))
        .inner;
    if resp.clicked() && !selected {
        resp.mark_changed();
        true
    } else {
        false
    }
}

#[cfg(target_arch = "wasm32")]
type SelectedPickerPromise = UnsendPromise<anyhow::Result<WorkerDirectory>>;

#[cfg(target_arch = "wasm32")]
type FolderListPromise = UnsendPromise<anyhow::Result<Vec<WorkerDirectory>>>;
#[cfg(target_arch = "wasm32")]
type ConvertibleFolderListPromise =
    ConvertiblePromise<FolderListPromise, anyhow::Result<Vec<WorkerDirectory>>>;

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct SetupPromises {
    selected: Option<SelectedPickerPromise>,
    list: Option<ConvertibleFolderListPromise>,
}

#[cfg(target_arch = "wasm32")]
static IS_DIRECTORY_PICKER_SUPPORTED: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(SetupPromises::is_supported);

#[cfg(target_arch = "wasm32")]
fn draw_unsupported_directory_picker(ui: &mut egui::Ui) {
    static TITLE: &str = "Your browser does not support the File System Access API.";
    static LINK_DESC: &str = "At the moment, only Chromium-based browsers support it.";
    static LINK: &str = "https://developer.mozilla.org/en-US/docs/Web/API/File_System_Access_API#browser_compatibility";

    ui.vertical_centered(|ui| {
        ui.label(TITLE);
        ui.add(
            egui::Hyperlink::from_label_and_url(
                egui::RichText::new(LINK_DESC).small().weak(),
                LINK,
            )
            .open_in_new_tab(true),
        );
    });
}

#[cfg(target_arch = "wasm32")]
impl SetupPromises {
    fn take_folder(&mut self) -> Option<WorkerDirectory> {
        if let Some(result) = self.selected.take_if(|p| p.ready()) {
            let result = result.block_and_take();

            self.list.take();
            match result {
                Ok(handle) => Some(handle),
                Err(e) => {
                    log::error!("Error picking folder: {e}");
                    None
                }
            }
        } else {
            None
        }
    }

    fn open_folder_picker<F: Future<Output = anyhow::Result<()>>>(
        &mut self,
        mode: web_sys::FileSystemPermissionMode,
        store_folder: impl Fn(WorkerDirectory) -> F + 'static,
    ) {
        use eframe::wasm_bindgen::JsCast;
        use wasm_bindgen_futures::JsFuture;
        use web_sys::{DirectoryPickerOptions, FileSystemDirectoryHandle};

        let ret = UnsendPromise::new(async move {
            let opts = DirectoryPickerOptions::new();
            opts.set_mode(mode);
            let promise = web_sys::window()
                .expect("no window")
                .show_directory_picker_with_options(&opts);
            let promise = promise.map_err(|e| anyhow::anyhow!("{e:?}"))?;
            let result = JsFuture::from(promise).await;
            match result {
                Ok(handle) => {
                    let handle = handle
                        .dyn_into::<FileSystemDirectoryHandle>()
                        .map_err(|_| {
                            anyhow::anyhow!("Error casting to FileSystemDirectoryHandle")
                        })?;
                    let handle = WorkerDirectory(handle);
                    store_folder(handle.clone()).await.map(|_| handle)
                }
                Err(e) => Err(anyhow::anyhow!("Error picking folder: {e:?}")),
            }
        });
        self.selected = Some(ret);
    }

    fn get_folder_list<F: Future<Output = anyhow::Result<Vec<WorkerDirectory>>> + 'static>(
        &mut self,
        future: impl FnOnce() -> F,
    ) -> Option<&anyhow::Result<Vec<WorkerDirectory>>> {
        if self.list.is_none() {
            self.list = Some(ConvertiblePromise::new_promise(
                UnsendPromise::new(future()),
            ));
        }
        self.list.as_mut().unwrap().get(|r| r)
    }

    fn is_supported() -> bool {
        use web_sys::js_sys::Reflect;

        Reflect::has(
            &web_sys::window().expect("no window"),
            &"showDirectoryPicker".into(),
        )
        .expect("Reflect::has failed")
    }
}
