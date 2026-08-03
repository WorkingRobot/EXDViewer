use std::{cell::OnceCell, collections::HashSet, io::Write, num::NonZero, rc::Rc, sync::Arc};

#[cfg(target_arch = "wasm32")]
use crate::utils::{PromiseKind, UnsendPromise};
use anyhow::Result;
use egui::{
    Button, CentralPanel, FontData, FontDefinitions, FontFamily, Layout, RichText, ScrollArea,
    TextEdit, Vec2, Widget,
    containers::{menu::MenuButton, panel::Panel},
    style::ScrollStyle,
};
use egui_extras::install_image_loaders;
use ironworks::excel::Language;
use itertools::{EitherOrBoth, Itertools};
use lru::LruCache;
use matchit::Params;
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::{
    about, assets,
    backend::Backend,
    editable_schema::EditableSchema,
    excel::{
        base::BaseSheet,
        provider::{ExcelHeader, ExcelProvider},
    },
    github::CALLBACK_PATH,
    goto::{self, ListNav},
    icons, music,
    pr_window::{self, PrAction, PrWindow},
    router::{
        Router,
        path::Path,
        route::{Redirect, static_title},
    },
    schema::{provider::SchemaProvider, web::WebProvider},
    settings::{
        ALWAYS_HIRES, BACKEND_CONFIG, BackendConfig, CODE_SYNTAX_THEME, COLOR_THEME,
        CURRENT_SHEET_LANGUAGES, DISPLAY_FIELD_SHOWN, EVALUATE_STRINGS, FILTER_GUIDE_VISIBLE,
        GithubSchemaBranch, LANGUAGE, LOGGER_SHOWN, MISC_SHEETS_SHOWN, PR_CHANGED_ONLY,
        SCHEMA_EDITOR_VISIBLE, SELECTED_SHEET, SHEET_FILTER_OPTIONS, SHEET_FILTERS, SHEETS_FILTER,
        SOLID_SCROLLBAR, SORTED_BY_OFFSET, SchemaLocation, TEMP_HIGHLIGHTED_ROW, TEMP_SCROLL_TO,
        TEXT_MAX_LINES, TEXT_USE_SCROLL, TEXT_WRAP_WIDTH,
    },
    setup::{self, SetupWindow},
    sheet::{
        CellResponse, FilterInputType, GlobalContext, MatchOptions, SheetTable, TableContext,
        draw_filter_guide, export_csv,
    },
    shortcuts::{GOTO_ROW, GOTO_SHEET, PALETTE},
    utils::{
        CodeTheme, CollapsibleSidePanel, ColorTheme, ConvertiblePromise, FuzzyMatcher, IconManager,
        Side, TrackedPromise, opt_slider, shortcut, tick_promises,
    },
};

const SHEETS_FILTER_ID: &str = "sheets_filter";

type CachedSheetEntry = (
    Language, // language
    String,   // sheet name
);

type CachedSheetPromise = TrackedPromise<Result<BaseSheet>>;
type ConvertibleSheetPromise = ConvertiblePromise<CachedSheetPromise, Result<SheetTable>>;

type CachedSchemaEntry = String; // sheet name

type CachedSchemaPromise = TrackedPromise<Option<Result<String>>>;
type ConvertibleSchemaPromise = ConvertiblePromise<CachedSchemaPromise, Result<EditableSchema>>;

type CachedLanguagesPromise = TrackedPromise<Result<Vec<Language>>>;
type ConvertibleLanguagesPromise =
    ConvertiblePromise<CachedLanguagesPromise, Result<Vec<Language>>>;

/// Fuzzy-matched sheet names (name + score) cached per (filter text, show-misc) key.
type SheetFilterData = LruCache<(String, bool), Rc<Vec<(String, i32)>>>;

/// Identifies which pull request a changed-schema set belongs to: (owner, repo, number).
type ChangedSchemasKey = (String, String, u32);

type CachedChangedSchemasPromise = TrackedPromise<Result<Vec<String>>>;
/// Converts to the set of PR-changed sheet names, or `None` if the fetch failed
/// (in which case the changed-only filter is treated as inactive).
type ConvertibleChangedSchemasPromise =
    ConvertiblePromise<CachedChangedSchemasPromise, Option<Rc<HashSet<String>>>>;

/// The state of the "changed schemas only" filter for the active schema source.
enum PrChangedState {
    /// The active schema source is not a pull request; the filter does not apply.
    NotPr,
    /// A pull request is active but its changed-file list is still loading.
    Pending,
    /// The changed-file list failed to load; the filter is inert (show everything).
    Failed,
    /// The set of sheet names the pull request changed.
    Ready(Rc<HashSet<String>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CjkFont {
    Japanese,
    Korean,
    ChineseSimplified,
    ChineseTraditional,
}

impl CjkFont {
    fn for_language(language: Language) -> Option<Self> {
        match language {
            Language::Korean => Some(Self::Korean),
            Language::ChineseSimplified => Some(Self::ChineseSimplified),
            Language::ChineseTraditional | Language::TaiwanChinese => {
                Some(Self::ChineseTraditional)
            }
            Language::Japanese
            | Language::None
            | Language::English
            | Language::German
            | Language::French => Some(Self::Japanese),
        }
    }

    fn family_name(self) -> &'static str {
        match self {
            Self::Japanese => "NotoSans-JP",
            Self::Korean => "NotoSans-KR",
            Self::ChineseSimplified => "NotoSans-SC",
            Self::ChineseTraditional => "NotoSans-TC",
        }
    }

    fn asset_file(self) -> &'static str {
        match self {
            Self::Japanese => "NotoSansJP-Regular.ttf",
            Self::Korean => "NotoSansKR-Regular.ttf",
            Self::ChineseSimplified => "NotoSansSC-Regular.ttf",
            Self::ChineseTraditional => "NotoSansTC-Regular.ttf",
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn embedded_bytes(self) -> &'static [u8] {
        match self {
            Self::Japanese => include_bytes!("../assets/NotoSansJP-Regular.ttf"),
            Self::Korean => include_bytes!("../assets/NotoSansKR-Regular.ttf"),
            Self::ChineseSimplified => include_bytes!("../assets/NotoSansSC-Regular.ttf"),
            Self::ChineseTraditional => include_bytes!("../assets/NotoSansTC-Regular.ttf"),
        }
    }
}

/// Which top-level tab the current route belongs to. Drives the switcher and scopes the shortcuts
/// and menus that only make sense over sheet data.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Sheets,
    Assets,
    Icons,
    Music,
}

impl Tab {
    fn of(path: &str) -> Self {
        if path.starts_with("/assets") {
            Tab::Assets
        } else if path.starts_with("/icons") {
            Tab::Icons
        } else if path.starts_with("/music") {
            Tab::Music
        } else {
            Tab::Sheets
        }
    }

    fn title(self) -> &'static str {
        match self {
            Tab::Sheets => "Sheets",
            Tab::Assets => "Assets",
            Tab::Icons => "Icons",
            Tab::Music => "Music",
        }
    }
}

pub struct App {
    router: Rc<OnceCell<Router<Self>>>,
    icon_manager: IconManager,
    setup_window: Option<setup::SetupWindow>,
    backend: Option<Backend>,
    sheet_data: LruCache<CachedSheetEntry, ConvertibleSheetPromise>,
    schema_data: LruCache<CachedSchemaEntry, ConvertibleSchemaPromise>,
    sheet_languages: LruCache<String, ConvertibleLanguagesPromise>,
    sheet_matcher: FuzzyMatcher,
    sheet_filter_data: SheetFilterData,
    changed_schemas: Option<(ChangedSchemasKey, ConvertibleChangedSchemasPromise)>,
    save_promise: Option<TrackedPromise<()>>,
    export_promise: Option<TrackedPromise<()>>,
    pr_window: PrWindow,
    goto_window: Option<goto::GoToWindow>,
    sheet_nav: ListNav,
    about_open: bool,
    music: music::MusicPlayer,
    assets: assets::AssetBrowser,
    icons: icons::IconBrowser,
    last_system_theme: Option<egui::Theme>,
    /// `None` = Latin only
    loaded_cjk: Option<CjkFont>,
    #[cfg(target_arch = "wasm32")]
    font_promise: Option<(CjkFont, UnsendPromise<anyhow::Result<Vec<u8>>>)>,
}

fn create_router(ctx: egui::Context) -> Result<Router<App>> {
    let mut builder = Router::<App>::new(ctx);
    builder.set_title_formatter(|title| format!("{title} - EXDViewer"));
    builder.add_route("/", App::on_setup, App::draw_setup, static_title("Setup"))?;
    builder.add_route(
        "/sheet",
        App::on_unnamed_sheet,
        App::draw_unnamed_sheet,
        static_title("Sheet List"),
    )?;
    builder.add_route(
        "/sheet/{*name}",
        App::on_named_sheet,
        App::draw_named_sheet,
        App::title_named_sheet,
    )?;
    builder.add_route(
        "/assets",
        App::on_assets,
        App::draw_assets,
        App::title_assets,
    )?;
    builder.add_route(
        "/assets/{*path}",
        App::on_asset_path,
        App::draw_assets,
        App::title_assets,
    )?;
    builder.add_route("/icons", App::on_icons, App::draw_icons, App::title_icons)?;
    builder.add_route(
        "/icons/{id}",
        App::on_icon,
        App::draw_icons,
        App::title_icons,
    )?;
    builder.add_route("/music", App::on_music, App::draw_music, App::title_music)?;
    builder.add_route(
        "/music/{id}",
        App::on_music_track,
        App::draw_music,
        App::title_music,
    )?;
    builder.add_route(
        CALLBACK_PATH,
        App::on_auth_callback,
        App::draw_auth_callback,
        static_title("Signing in…"),
    )?;
    Ok(builder)
}

impl App {
    fn title_named_sheet(&self, _path: &Path, params: &Params<'_, '_>) -> Option<String> {
        Some(params.get("name")?.to_string())
    }

    fn title_assets(&self, _path: &Path, params: &Params<'_, '_>) -> Option<String> {
        let Some(asset) = params.get("path") else {
            return Some("Assets".to_string());
        };
        Some(crate::utils::file_name(asset).to_string())
    }

    fn title_icons(&self, _path: &Path, params: &Params<'_, '_>) -> Option<String> {
        let id = params.get("id")?.parse::<u32>().ok()?;
        Some(format!("Icon {id:06}"))
    }

    fn title_music(&self, _path: &Path, params: &Params<'_, '_>) -> Option<String> {
        let id = params.get("id")?.parse::<u32>().ok()?;
        Some(self.music.name_of(id).unwrap_or("Music").to_string())
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.router
            .get_or_init(|| create_router(ctx.clone()).unwrap());

        let tab = Tab::of(self.router.get().unwrap().current_path().path());

        if tab == Tab::Sheets && shortcut::consume(&ctx, GOTO_ROW) {
            self.goto_window = Some(goto::GoToWindow::to_row());
        }
        if tab == Tab::Sheets && shortcut::consume(&ctx, GOTO_SHEET) {
            self.goto_window = Some(goto::GoToWindow::to_sheet());
        }
        if shortcut::consume(&ctx, PALETTE) {
            self.open_palette(tab);
        }

        self.update_fonts(&ctx);
        self.update_sheet_languages(&ctx);
        self.pr_window.poll(&ctx);
        about::draw(&ctx, &mut self.about_open);
        self.draw_menubar(ui, tab);
        self.draw_logger(ui.ctx());
        self.draw_pr_window(ui.ctx());

        CentralPanel::default().show(ui, |ui| {
            self.draw_router(ui);
        });
    }

    fn draw_router(&mut self, ui: &mut egui::Ui) {
        self.router.clone().get().unwrap().ui(self, ui);
    }

    fn update_sheet_languages(&mut self, ctx: &egui::Context) {
        let Some(backend) = self.backend.as_ref() else {
            return;
        };
        let Some(sheet_name) = SELECTED_SHEET.get(ctx) else {
            return;
        };

        let entry = self.sheet_languages.get_or_insert_mut_ref(&sheet_name, || {
            let sheet_name = sheet_name.clone();
            let excel = backend.excel().clone();
            ConvertiblePromise::new_promise(TrackedPromise::spawn_local(async move {
                excel.get_available_languages(&sheet_name).await
            }))
        });
        let just_resolved = !entry.converted() && entry.should_swap();
        if let Some(Ok(languages)) = entry.get(|r| r) {
            CURRENT_SHEET_LANGUAGES.set(ctx, (sheet_name.clone(), languages.clone()));
        }
        if just_resolved {
            ctx.request_repaint();
        }
    }

    fn navigate(&self, path: impl Into<Path>) {
        self.router.get().unwrap().navigate(path).unwrap();
    }

    fn navigate_replace(&self, path: impl Into<Path>) {
        self.router.get().unwrap().replace(path).unwrap();
    }

    fn draw_goto(&mut self, ctx: &egui::Context) {
        if let Some(window) = self.goto_window.take() {
            let misc_sheets_shown = MISC_SHEETS_SHOWN.get(ctx);
            match window.draw(
                ctx,
                &self.sheet_matcher,
                &self.backend.as_ref().map_or(vec![], |b| {
                    b.excel()
                        .get_entries()
                        .iter()
                        .filter(|(_, id)| misc_sheets_shown || **id >= 0)
                        .map(|(s, _)| s.as_str())
                        .collect()
                }),
            ) {
                Ok(Some(data)) => {
                    let sheet = match &data {
                        EitherOrBoth::Left(sheet_name) | EitherOrBoth::Both(sheet_name, _) => {
                            Some(sheet_name.clone())
                        }
                        EitherOrBoth::Right(_) => SELECTED_SHEET.get(ctx),
                    };
                    let location = match &data {
                        EitherOrBoth::Left(_) => None,
                        EitherOrBoth::Right(loc) | EitherOrBoth::Both(_, loc) => Some(loc),
                    };

                    if let Some(sheet_name) = sheet {
                        if let Some((row, subrow)) = location {
                            self.navigate(format!(
                                "/sheet/{sheet_name}#R{row}{}",
                                if let Some(subrow) = subrow {
                                    format!(".{subrow}")
                                } else {
                                    String::new()
                                }
                            ));
                        } else {
                            self.navigate(format!("/sheet/{sheet_name}"));
                        }
                    }
                }
                Ok(None) => {}
                Err(window) => {
                    self.goto_window = Some(window);
                }
            }
        }
    }

    fn open_palette(&mut self, tab: Tab) {
        match tab {
            Tab::Sheets => {}
            Tab::Assets => self.assets.open_palette(),
            Tab::Icons => self.icons.open_palette(),
            Tab::Music => self.music.open_palette(),
        }
    }

    fn draw_menubar(&mut self, ui: &mut egui::Ui, tab: Tab) {
        let ctx = &ui.ctx().clone();
        Panel::top("top_panel")
            .frame(
                egui::Frame::side_top_panel(&ctx.global_style())
                    .fill(ctx.global_style().visuals.code_bg_color),
            )
            .show(ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    let bar_left = ui.min_rect().left();
                    let bar_width = ui.available_width();

                    ui.menu_button("App", |ui| {
                        if ui.button("Configure").clicked() {
                            self.navigate("/");
                            ui.close();
                        }
                        if !super::IS_WEB && ui.button("Quit").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            ui.close();
                        }
                    });

                    ui.menu_button("Go", |ui| {
                        let palette = match tab {
                            Tab::Sheets => {
                                if shortcut::button(ui, "Go to Row…", GOTO_ROW).clicked() {
                                    self.goto_window = Some(goto::GoToWindow::to_row());
                                    ui.close();
                                }
                                if shortcut::button(ui, "Go to Sheet…", GOTO_SHEET).clicked() {
                                    self.goto_window = Some(goto::GoToWindow::to_sheet());
                                    ui.close();
                                }
                                return;
                            }
                            Tab::Assets => "Find Asset…",
                            Tab::Icons => "Find Icon…",
                            Tab::Music => "Find Track…",
                        };
                        if shortcut::button(ui, palette, PALETTE).clicked() {
                            self.open_palette(tab);
                            ui.close();
                        }
                    });

                    ui.menu_button("Language", |ui| {
                        let saved_lang = LANGUAGE.get(ctx);
                        let selected_sheet = SELECTED_SHEET.get(ctx);
                        let sheet_languages = CURRENT_SHEET_LANGUAGES
                            .try_get(ctx)
                            .filter(|(name, _)| Some(name.as_str()) == selected_sheet.as_deref())
                            .map(|(_, langs)| langs);
                        let restrict = sheet_languages
                            .as_ref()
                            .is_some_and(|langs| langs.iter().any(|&l| l != Language::None));
                        for lang in Language::iter() {
                            if lang == Language::None {
                                continue;
                            }
                            let available = !restrict
                                || sheet_languages
                                    .as_ref()
                                    .is_some_and(|langs| langs.contains(&lang));
                            let response = ui.add_enabled(
                                available,
                                egui::Button::selectable(saved_lang == lang, lang.to_string()),
                            );
                            if response.clicked() {
                                LANGUAGE.set(ctx, lang);
                                ui.close();
                            }
                        }
                    });

                    ui.menu_button("View", |ui| {
                        ui.menu_button("Color Theme", |ui| {
                            let mut color_theme = COLOR_THEME.get(ui.ctx());
                            for theme in ColorTheme::themes() {
                                if ui
                                    .selectable_value(&mut color_theme, *theme, theme.name())
                                    .changed()
                                {
                                    color_theme.apply(ui.ctx());
                                    let solid_scrollbar = SOLID_SCROLLBAR.get(ctx);
                                    ctx.all_styles_mut(|s| {
                                        s.spacing.scroll = if solid_scrollbar {
                                            ScrollStyle::solid()
                                        } else {
                                            ScrollStyle::default()
                                        };
                                    });

                                    COLOR_THEME.set(ui.ctx(), color_theme);
                                }
                            }
                        });

                        ui.menu_button("Code Theme", |ui| {
                            let mut theme = CODE_SYNTAX_THEME.get(ui.ctx());

                            for (id, name) in CodeTheme::themes() {
                                if ui
                                    .selectable_value(&mut theme.theme, id.to_string(), name)
                                    .changed()
                                {
                                    CODE_SYNTAX_THEME.set(ui.ctx(), theme.clone());
                                }
                            }
                        });

                        ui.menu_button("Sort Columns by", |ui| {
                            let mut sorted_by_offset = SORTED_BY_OFFSET.get(ctx);
                            let r = ui.selectable_value(&mut sorted_by_offset, true, "Offset");
                            let r =
                                r.union(ui.selectable_value(&mut sorted_by_offset, false, "Index"));
                            if r.changed() {
                                ui.close();
                                SORTED_BY_OFFSET.set(ctx, sorted_by_offset);
                            }
                        });

                        ui.menu_button("Text Wrapping", |ui| {
                            let r = opt_slider(
                                ui,
                                TEXT_WRAP_WIDTH.get(ctx).map(|e| e.into()),
                                50..=1000,
                                "Max Width",
                                "No Wrap",
                                "px",
                            );

                            let r2 = opt_slider(
                                ui,
                                TEXT_MAX_LINES.get(ctx).map(|e| e.into()),
                                1..=20,
                                "Max Lines",
                                "No Limit",
                                "",
                            );

                            if r.response.changed() || r2.response.changed() {
                                TEXT_WRAP_WIDTH.set(
                                    ctx,
                                    r.inner.map(|e| NonZero::new(e.get() as u16).unwrap()),
                                );

                                TEXT_MAX_LINES.set(
                                    ctx,
                                    r2.inner.map(|e| NonZero::new(e.get() as u8).unwrap()),
                                );

                                for sheet in &mut self.sheet_data {
                                    if let Ok(Ok(s)) = sheet.1.try_get_mut() {
                                        s.invalidate_sizes(ui);
                                    }
                                }
                            }

                            let mut use_scroll = TEXT_USE_SCROLL.get(ctx);
                            ui.with_layout(Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.style_mut().spacing.item_spacing.x /= 2.0;
                                ui.set_max_width(
                                    ui.spacing().slider_width + ui.spacing().interact_size.x,
                                );
                                ui.label("Show ");
                                if ui
                                    .selectable_label(
                                        use_scroll,
                                        if use_scroll { "Scrollbar" } else { "Tooltip" },
                                    )
                                    .clicked()
                                {
                                    use_scroll = !use_scroll;
                                    TEXT_USE_SCROLL.set(ctx, use_scroll);
                                }
                                ui.label(" on overflow");
                            })
                        });

                        {
                            let mut solid_scrollbar = SOLID_SCROLLBAR.get(ctx);
                            if ui
                                .checkbox(&mut solid_scrollbar, "Solid Scrollbar")
                                .changed()
                            {
                                SOLID_SCROLLBAR.set(ctx, solid_scrollbar);
                                ctx.all_styles_mut(|s| {
                                    s.spacing.scroll = if solid_scrollbar {
                                        ScrollStyle::solid()
                                    } else {
                                        ScrollStyle::default()
                                    };
                                });
                                ui.close();
                            }
                        }

                        {
                            let mut always_hires = ALWAYS_HIRES.get(ctx);
                            if ui.checkbox(&mut always_hires, "HD Icons").changed() {
                                ALWAYS_HIRES.set(ctx, always_hires);
                                ui.close();
                            }
                        }

                        {
                            let mut evaluate_strings = EVALUATE_STRINGS.get(ctx);
                            if ui
                                .checkbox(&mut evaluate_strings, "Evaluate SeStrings")
                                .changed()
                            {
                                EVALUATE_STRINGS.set(ctx, evaluate_strings);

                                for sheet in &mut self.sheet_data {
                                    if let Ok(Ok(s)) = sheet.1.try_get_mut() {
                                        s.invalidate_sizes(ui);
                                    }
                                }
                            }
                        }

                        {
                            let mut display_field_shown = DISPLAY_FIELD_SHOWN.get(ctx);
                            if ui
                                .checkbox(&mut display_field_shown, "Use Display Fields")
                                .changed()
                            {
                                DISPLAY_FIELD_SHOWN.set(ctx, display_field_shown);
                                ui.close();
                            }
                        }

                        {
                            let mut logger_shown = LOGGER_SHOWN.get(ctx);
                            if ui.checkbox(&mut logger_shown, "Show Log Window").changed() {
                                LOGGER_SHOWN.set(ctx, logger_shown);
                            }
                        }
                    });

                    let seg = egui::vec2(72.0, ui.spacing().interact_size.y);
                    let switcher_w = 4.0 * seg.x + 3.0 * ui.spacing().item_spacing.x;
                    let target_left = bar_left + bar_width / 2.0 - switcher_w / 2.0;
                    let space = target_left - ui.cursor().left();
                    if space > 0.0 {
                        ui.add_space(space);
                    }
                    for (target, route) in [
                        (Tab::Sheets, "/sheet"),
                        (Tab::Assets, "/assets"),
                        (Tab::Icons, "/icons"),
                        (Tab::Music, "/music"),
                    ] {
                        if ui
                            .add_sized(seg, Button::selectable(tab == target, target.title()))
                            .clicked()
                        {
                            self.navigate(route);
                        }
                    }

                    add_links(ui, &mut self.about_open);
                });
            });
    }

    fn draw_logger(&mut self, ctx: &egui::Context) {
        let logger_shown = LOGGER_SHOWN.get(ctx);
        let mut logger_shown_toggle = logger_shown;
        egui::Window::new("Log")
            .open(&mut logger_shown_toggle)
            .show(ctx, |ui| {
                egui_logger::logger_ui().show(ui);
            });
        if logger_shown_toggle != logger_shown {
            LOGGER_SHOWN.set(ctx, logger_shown_toggle);
        }
    }

    fn poll_changed_schemas(&mut self, ctx: &egui::Context) -> PrChangedState {
        let key = match BACKEND_CONFIG.get(ctx) {
            Some(BackendConfig {
                schema: SchemaLocation::Github(location),
                ..
            }) => match &location.branch {
                GithubSchemaBranch::PullRequest { number, .. } => {
                    (location.owner.clone(), location.repo.clone(), *number)
                }
                _ => {
                    self.changed_schemas = None;
                    return PrChangedState::NotPr;
                }
            },
            _ => {
                self.changed_schemas = None;
                return PrChangedState::NotPr;
            }
        };

        if self.changed_schemas.as_ref().map(|(k, _)| k) != Some(&key) {
            let (owner, repo, number) = key.clone();
            self.changed_schemas = Some((
                key,
                ConvertiblePromise::new_promise(TrackedPromise::spawn_local(async move {
                    WebProvider::fetch_github_pull_request_files(&owner, &repo, number).await
                })),
            ));
        }

        let (_, promise) = self.changed_schemas.as_mut().unwrap();
        match promise.get(|result| match result {
            Ok(names) => Some(Rc::new(names.into_iter().collect())),
            Err(e) => {
                log::error!("Error fetching PR-changed schemas: {e}");
                None
            }
        }) {
            None => PrChangedState::Pending,
            Some(None) => PrChangedState::Failed,
            Some(Some(set)) => PrChangedState::Ready(set.clone()),
        }
    }

    fn draw_sheet_list(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        let pr_changed = self.poll_changed_schemas(ctx);
        self.sheet_nav.claim(
            ctx,
            !CollapsibleSidePanel::is_collapsed(ctx, "sheet_list"),
            Some(egui::Id::new(SHEETS_FILTER_ID)),
        );
        let mut nav = std::mem::take(&mut self.sheet_nav);
        CollapsibleSidePanel::new("sheet_list", Side::Left).show(ui, |ui, is_open| {
            if !is_open {
                return;
            }

            Panel::top("sheet_list_header").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(egui::Align::Min), |ui| {
                        CollapsibleSidePanel::draw_arrow(ui, "sheet_list", Side::Left);
                        ui.vertical_centered_justified(|ui| ui.heading("Sheets"));
                    });
                });
                ui.add_space(4.0);
                ui.with_layout(Layout::right_to_left(egui::Align::Min), |ui| {
                    let mut sheets_filter = SHEETS_FILTER.get(ctx);
                    let resp = ui
                        .add_enabled(!sheets_filter.is_empty(), Button::new("↩"))
                        .on_hover_text("Clear");
                    if resp.clicked() {
                        sheets_filter.clear();
                        SHEETS_FILTER.set(ctx, sheets_filter.clone());
                    }

                    let mut misc_sheets_shown = MISC_SHEETS_SHOWN.get(ctx);
                    if ui
                        .toggle_value(&mut misc_sheets_shown, "🗄")
                        .on_hover_text("Show Miscellaneous Sheets")
                        .changed()
                    {
                        MISC_SHEETS_SHOWN.set(ctx, misc_sheets_shown);
                    }

                    if !matches!(pr_changed, PrChangedState::NotPr) {
                        let mut changed_only = PR_CHANGED_ONLY.get(ctx);
                        let hover = match &pr_changed {
                            PrChangedState::Ready(_) => "Filter unchanged sheets",
                            PrChangedState::Pending => "Filter unchanged sheets (loading…)",
                            PrChangedState::Failed => "Filter unchanged sheets (failed to load)",
                            PrChangedState::NotPr => unreachable!(),
                        };
                        if ui
                            .toggle_value(&mut changed_only, "±")
                            .on_hover_text(hover)
                            .changed()
                        {
                            PR_CHANGED_ONLY.set(ctx, changed_only);
                        }
                    }

                    if ui
                        .add_sized(
                            Vec2::new(ui.available_width(), 0.0),
                            TextEdit::singleline(&mut sheets_filter)
                                .id(egui::Id::new(SHEETS_FILTER_ID))
                                .hint_text("Filter"),
                        )
                        .changed()
                    {
                        SHEETS_FILTER.set(ctx, sheets_filter);
                    }
                });
                ui.add_space(4.0);
            });

            let modified_schemas = self.get_modified_schemas();
            if !modified_schemas.is_empty() {
                let count = modified_schemas.len();
                let modified_tooltip = modified_schemas.iter().map(|(name, _)| name).join("\n");
                drop(modified_schemas);
                let save_label = if count > 1 { "Save All" } else { "Save" };

                Panel::bottom("sheet_list_status").show(ui, |ui| {
                    let can_pr = pr_window::github_source(ctx).is_some();
                    ui.vertical_centered(|ui| {
                        ui.label(format!(
                            "{count} modified schema{}",
                            if count > 1 { "s" } else { "" }
                        ))
                        .on_hover_text(modified_tooltip);
                    });

                    let mut save = false;
                    let mut open_pr = false;
                    if can_pr {
                        ui.columns_const(|[c1, c2]| {
                            c1.vertical_centered_justified(|ui| {
                                if ui.button("Create PR").clicked() {
                                    open_pr = true;
                                }
                            });
                            c2.vertical_centered_justified(|ui| {
                                if ui.button(save_label).clicked() {
                                    save = true;
                                }
                            });
                        });
                    } else {
                        ui.vertical_centered_justified(|ui| {
                            if ui.button(save_label).clicked() {
                                save = true;
                            }
                        });
                    }
                    if save {
                        self.command_save_all_schemas();
                    }
                    if open_pr {
                        self.command_open_pr();
                    }
                });
            }

            let sheets_filter = SHEETS_FILTER.get(ctx);
            let misc_sheets_shown = MISC_SHEETS_SHOWN.get(ctx);
            let backend = self.backend.clone().unwrap();
            let sheets = self
                .sheet_filter_data
                .get_or_insert((sheets_filter.clone(), misc_sheets_shown), || {
                    let sheets = backend
                        .excel()
                        .get_entries()
                        .iter()
                        .filter(|(_, id)| misc_sheets_shown || **id >= 0)
                        .sorted_by_key(|(sheet, _)| *sheet)
                        .map(|(s, &id)| (s.clone(), id));
                    let sheets = self.sheet_matcher.match_list_indirect(
                        (!sheets_filter.is_empty()).then_some(&sheets_filter),
                        sheets,
                        |s| &s.0,
                    );
                    Rc::new(sheets)
                })
                .clone();

            let sheets = match &pr_changed {
                PrChangedState::Ready(changed) if PR_CHANGED_ONLY.get(ctx) => Rc::new(
                    sheets
                        .iter()
                        .filter(|(name, _)| changed.contains(name))
                        .cloned()
                        .collect::<Vec<_>>(),
                ),
                _ => sheets,
            };

            egui::CentralPanel::default().show(ui, |ui| {
                let row_height = ui.text_style_height(&egui::TextStyle::Button);
                let mut opened = nav.apply(sheets.len()).map(|at| sheets[at].0.clone());
                let mut area = ScrollArea::both().auto_shrink(false);
                if let Some(offset) = nav.scroll(ui, row_height, sheets.len()) {
                    area = area.vertical_scroll_offset(offset);
                }
                let output = area.show_rows(ui, row_height, sheets.len(), |ui, range| {
                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                        let current_sheet = SELECTED_SHEET.get(ctx);
                        for (at, (sheet, id)) in sheets[range.clone()].iter().enumerate() {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                            let resp = Button::selectable(
                                current_sheet.as_ref() == Some(sheet),
                                sheet.as_str(),
                            )
                            .ui(ui)
                            .on_hover_text(format!("{sheet}\nId: {id}"));
                            nav.mark(ui, range.start + at, resp.rect);
                            if resp.clicked() {
                                opened = Some(sheet.clone());
                            }
                        }
                    });
                });
                nav.seen(&output);
                if let Some(sheet) = opened {
                    SELECTED_SHEET.set(ctx, Some(sheet.clone()));
                    self.navigate(format!("/sheet/{sheet}"));
                }
            });
        });
        self.sheet_nav = nav;
    }

    fn draw_sheet_data(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        self.export_promise.take_if(|p| p.try_get().is_some());
        let mut export_request = None;

        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(&ctx.global_style()).inner_margin(egui::Margin {
                    left: 8,
                    right: 8,
                    top: 2,
                    bottom: 8,
                }),
            )
            .show(ui, |ui| {
                let backend = self.backend.as_ref().unwrap();
                let sheet_name = SELECTED_SHEET.get(ctx).unwrap();
                let language = LANGUAGE.get(ctx);

                let sheet_data =
                    self.sheet_data
                        .get_or_insert_mut_ref(&(language, sheet_name.clone()), || {
                            let sheet_name = sheet_name.clone();
                            let excel = backend.excel().clone();

                            ConvertiblePromise::new_promise(TrackedPromise::spawn_local(
                                async move { excel.get_sheet(&sheet_name, language).await },
                            ))
                        });

                let schema_data = self.schema_data.get_or_insert_mut_ref(&sheet_name, || {
                    let sheet_name = sheet_name.clone();
                    let is_sheet_miscellaneous = backend
                        .excel()
                        .get_entries()
                        .get(&sheet_name)
                        .copied()
                        .unwrap_or_default()
                        < 0;
                    let schema = backend.schema().clone();

                    ConvertiblePromise::new_promise(TrackedPromise::spawn_local(async move {
                        if !is_sheet_miscellaneous {
                            Some(schema.get_schema_text(&sheet_name).await)
                        } else {
                            None
                        }
                    }))
                });

                let schema_loading = !schema_data.should_swap();
                let sheet_loading = !sheet_data.should_swap();

                let combined_result = sheet_data.get_mut_with(schema_data, |sheet, schema| {
                    let editor = schema.either(
                        |schema| match schema {
                            Some(Ok(schema)) => Ok(EditableSchema::new(&sheet_name, schema)),
                            Some(Err(error)) => {
                                // Soft-fail on schema retrieval/parsing errors
                                log::error!("Failed to get schema: {error:?}");
                                let column_count = sheet.as_ref().either(
                                    |sheet| sheet.as_ref().map(|sheet| sheet.columns().len()),
                                    |sheet| {
                                        sheet
                                            .as_ref()
                                            .map(|sheet| sheet.context().sheet().columns().len())
                                    },
                                );
                                if let Ok(column_count) = column_count {
                                    EditableSchema::from_blank(&sheet_name, column_count)
                                } else {
                                    Err(anyhow::anyhow!(
                                        "Failed to load sheet to create blank schema"
                                    ))
                                }
                            }
                            None => EditableSchema::from_miscellaneous(&sheet_name),
                        },
                        |schema| schema,
                    );

                    let table = sheet.either(
                        |sheet| {
                            sheet.and_then(|sheet| {
                                let schema = editor.as_ref().map(|e| e.get_schema());
                                if let Ok(schema) = schema {
                                    Ok(SheetTable::new(
                                        TableContext::new(
                                            GlobalContext::new(
                                                ui.ctx().clone(),
                                                backend.clone(),
                                                language,
                                                self.icon_manager.clone(),
                                            ),
                                            sheet,
                                            schema,
                                        ),
                                        ui,
                                    ))
                                } else {
                                    Err(anyhow::anyhow!("Failed to load schema to create table"))
                                }
                            })
                        },
                        |table| table,
                    );

                    (table, editor)
                });

                let (table, editor) = match combined_result {
                    None if schema_loading && sheet_loading => {
                        ui.label("Loading sheet and schema...");
                        return;
                    }
                    None if schema_loading => {
                        ui.label("Loading schema...");
                        return;
                    }
                    None if sheet_loading => {
                        ui.label("Loading sheet...");
                        return;
                    }
                    None => {
                        ui.label("Preparing sheet and schema...");
                        return;
                    }
                    Some((Err(err), Err(err2))) => {
                        ui.label("Failed to load sheet and schema");
                        ui.label(err.to_string());
                        ui.label(err2.to_string());
                        return;
                    }
                    Some((Err(err), _)) => {
                        ui.label("Failed to load sheet");
                        ui.label(err.to_string());
                        return;
                    }
                    Some((_, Err(err))) => {
                        ui.label("Failed to load schema");
                        ui.label(err.to_string());
                        return;
                    }
                    Some((Ok(table), Ok(editor))) => (table, editor),
                };

                Panel::top("sheet_data_header").show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if CollapsibleSidePanel::is_collapsed(ui.ctx(), "sheet_list") {
                            ui.with_layout(Layout::left_to_right(egui::Align::Min), |ui| {
                                CollapsibleSidePanel::draw_arrow(ui, "sheet_list", Side::Left);
                            });
                        }

                        ui.vertical_centered_justified(|ui| ui.heading(sheet_name.clone()));
                    });
                    ui.add_space(4.0);
                    ui.with_layout(Layout::left_to_right(egui::Align::Min), |ui| {
                        let (mut filter_type, mut filter_text) = SHEET_FILTERS
                            .use_with(ui.ctx(), |map| {
                                map.entry(sheet_name.clone()).or_default().clone()
                            });

                        ui.spacing_mut().item_spacing.x /= 2.0;

                        let (button_resp, menu_resp) = MenuButton::from_button(
                            Button::new(filter_type.emoji())
                                .min_size(Vec2::splat(ui.spacing().interact_size.y)),
                        )
                        .ui(ui, |ui| {
                            let mut changed = false;
                            for value in &[
                                FilterInputType::Equals,
                                FilterInputType::Contains,
                                FilterInputType::Complex,
                            ] {
                                let resp =
                                    ui.selectable_value(&mut filter_type, *value, value.emoji());
                                if resp.changed() {
                                    changed = true;
                                }
                                resp.on_hover_text(value.to_string());
                            }
                            changed
                        });

                        button_resp.on_hover_text(format!("Filter Type:\n{filter_type}"));

                        let mut filter_dirty = menu_resp.is_some_and(|m| m.inner);

                        {
                            let MatchOptions {
                                mut case_insensitive,
                                mut use_display_field,
                            } = SHEET_FILTER_OPTIONS.get(ctx);

                            let mut is_dirty = ui
                                .toggle_value(&mut case_insensitive, "🔡")
                                .on_hover_text("Case Insensitive")
                                .changed();
                            is_dirty |= ui
                                .toggle_value(&mut use_display_field, "📝")
                                .on_hover_text("Use Display Field")
                                .changed();

                            if is_dirty {
                                SHEET_FILTER_OPTIONS.set(
                                    ctx,
                                    MatchOptions {
                                        case_insensitive,
                                        use_display_field,
                                    },
                                );
                                filter_dirty = true;
                            }
                        }

                        if filter_type == FilterInputType::Complex {
                            let mut guide_visible = FILTER_GUIDE_VISIBLE.get(ctx);
                            if ui
                                .toggle_value(&mut guide_visible, "\u{ff1f}")
                                .on_hover_text("Filter Guide")
                                .changed()
                            {
                                FILTER_GUIDE_VISIBLE.set(ctx, guide_visible);
                            }
                            draw_filter_guide(ctx);
                        }

                        ui.with_layout(Layout::right_to_left(egui::Align::Min), |ui| {
                            let is_miscellaneous = backend
                                .excel()
                                .get_entries()
                                .get(&sheet_name)
                                .copied()
                                .unwrap_or_default()
                                < 0;

                            ui.add_enabled_ui(!is_miscellaneous, |ui| {
                                let mut visible = SCHEMA_EDITOR_VISIBLE.get(ui.ctx());
                                let resp = ui
                                    .toggle_value(&mut visible, "Edit Schema")
                                    .on_hover_text("Edit the schema for this sheet");
                                if resp.changed() {
                                    SCHEMA_EDITOR_VISIBLE.set(ui.ctx(), visible);
                                }
                            });

                            let exporting = self.export_promise.is_some();
                            if exporting {
                                ui.spinner();
                            }
                            ui.add_enabled_ui(!exporting, |ui| {
                                ui.menu_button("Export", |ui| {
                                    if ui
                                        .button("As CSV")
                                        .on_hover_text("Links export as display values")
                                        .clicked()
                                    {
                                        export_request = Some((table.context().clone(), true));
                                        ui.close();
                                    }
                                    if ui
                                        .button("As CSV (Raw)")
                                        .on_hover_text("Links export as raw values")
                                        .clicked()
                                    {
                                        export_request = Some((table.context().clone(), false));
                                        ui.close();
                                    }
                                });
                            });

                            let filter_error = table.get_filter_error();

                            let filter_resp = ui.add_sized(
                                Vec2::new(ui.available_width(), 0.0),
                                TextEdit::singleline(&mut filter_text)
                                    .hint_text("Filter")
                                    .background_color(if filter_error.is_some() {
                                        ui.visuals()
                                            .text_edit_bg_color()
                                            .blend(ui.visuals().error_fg_color.gamma_multiply(0.2))
                                    } else {
                                        ui.visuals().text_edit_bg_color()
                                    }),
                            );

                            filter_dirty |= filter_resp.changed();

                            if let Some(text) = filter_error {
                                filter_resp.on_hover_text(RichText::new(text).monospace());
                            }
                        });

                        if filter_dirty {
                            SHEET_FILTERS.use_with(ui.ctx(), |map| {
                                map.entry(sheet_name.clone())
                                    .insert_entry((filter_type, filter_text.clone()));
                            });

                            table.update_filter(ui.ctx());
                        }
                    });
                    ui.add_space(4.0);
                });

                let resp = editor.draw(ui, backend.schema());
                if resp.changed()
                    && let Some(schema) = editor.get_schema()
                {
                    match table.context().set_schema(Some(schema)) {
                        // The filter is compiled against the columns, so it has to be redone
                        Ok(()) => table.update_filter(ui.ctx()),
                        Err(e) => log::error!("Failed to set schema: {e:?}"),
                    }
                }

                let scroll_to = TEMP_SCROLL_TO.take(ctx);
                if let Some((row_pos, _)) = &scroll_to {
                    TEMP_HIGHLIGHTED_ROW.set(ctx, *row_pos);
                }

                let resp = table.draw(ui, scroll_to);
                match resp {
                    CellResponse::None => {}
                    CellResponse::Icon(_) => {}
                    CellResponse::Link((sheet_name, (row_id, subrow_id))) => {
                        self.navigate(format!(
                            "/sheet/{sheet_name}#R{row_id}{}",
                            if let Some(subrow_id) = subrow_id {
                                format!(".{subrow_id}")
                            } else {
                                String::new()
                            }
                        ));
                    }
                    CellResponse::Row((sheet_name, (row_id, subrow_id))) => {
                        self.navigate_replace(format!(
                            "/sheet/{sheet_name}#R{row_id}{}",
                            if let Some(subrow_id) = subrow_id {
                                format!(".{subrow_id}")
                            } else {
                                String::new()
                            }
                        ));
                        ui.ctx().copy_text(self.router.get().unwrap().full_url());
                    }
                }
            });

        if let Some((context, resolve_display_field)) = export_request {
            self.command_export_csv(context, resolve_display_field);
        }
    }

    fn on_setup(&mut self, ui: &mut egui::Ui, path: &Path, _params: &Params<'_, '_>) -> Redirect {
        self.setup_window = Some(SetupWindow::from_config(
            ui.ctx(),
            path.query_pairs().contains_key("redirect"),
        ));
        None
    }

    fn draw_setup(&mut self, ui: &mut egui::Ui, path: &Path, _params: &Params<'_, '_>) {
        if let Some((backend, config)) = self.setup_window.as_mut().unwrap().draw(ui.ctx()) {
            self.backend = Some(backend);
            self.sheet_data.clear();
            self.schema_data.clear();
            self.sheet_languages.clear();
            // The sheet list is keyed on the filter alone, and icons on their id, so neither
            // notices that they now belong to a different install.
            self.sheet_filter_data.clear();
            self.icon_manager.clear();
            self.assets.reset();
            self.icons.reset();
            self.music.reset();
            CURRENT_SHEET_LANGUAGES.remove(ui.ctx());

            BACKEND_CONFIG.set(ui.ctx(), Some(config));
            if let Some(redirect_path) = path.query_pairs().get("redirect").map(|s| s.as_str()) {
                self.navigate_replace(redirect_path);
            } else {
                self.navigate("/sheet");
            }
        }
    }

    fn on_auth_callback(
        &mut self,
        _ui: &mut egui::Ui,
        _path: &Path,
        _params: &Params<'_, '_>,
    ) -> Redirect {
        None
    }

    fn draw_auth_callback(&mut self, ui: &mut egui::Ui, _path: &Path, _params: &Params<'_, '_>) {
        pr_window::draw_auth_callback(ui);
    }

    fn ensure_backend(&self, path: &Path) -> Redirect {
        if self.backend.is_none() {
            return Some(Path::with_params("/", &[("redirect", path.to_string())]));
        }
        None
    }

    fn on_unnamed_sheet(
        &mut self,
        ui: &mut egui::Ui,
        path: &Path,
        _params: &Params<'_, '_>,
    ) -> Redirect {
        if let Some(redirect) = self.ensure_backend(path) {
            return Some(redirect);
        }

        if let Some(sheet) = &SELECTED_SHEET.get(ui.ctx()) {
            return Some(format!("/sheet/{sheet}").into());
        }
        None
    }

    fn on_named_sheet(
        &mut self,
        ui: &mut egui::Ui,
        path: &Path,
        params: &Params<'_, '_>,
    ) -> Redirect {
        if let Some(redirect) = self.ensure_backend(path) {
            return Some(redirect);
        }
        TEMP_HIGHLIGHTED_ROW.take(ui.ctx());

        if let Some(sheet) = params.get("name") {
            SELECTED_SHEET.set(ui.ctx(), Some(sheet.to_string()));
        } else {
            SELECTED_SHEET.set(ui.ctx(), None);
            return Some("/sheet".into());
        }

        if let Some(mut fragment) = path.fragment() {
            let mut col_nr: Option<u16> = None;
            if let Some((rest, col_str)) = fragment.rsplit_once('C') {
                col_nr = col_str.parse::<u16>().ok();
                fragment = rest;
            }

            let mut row_pos: Option<(u32, Option<u16>)> = None;
            if let Some((_rest, row_str)) = fragment.rsplit_once('R') {
                if let Some((row_str, subrow_str)) = row_str.split_once('.') {
                    let row = row_str.parse::<u32>().ok();
                    let subrow = subrow_str.parse::<u16>().ok();
                    if let Some(row) = row {
                        row_pos = Some((row, subrow));
                    }
                } else if let Ok(row) = row_str.parse::<u32>() {
                    row_pos = Some((row, None));
                }
            }

            if let Some((row, subrow)) = row_pos {
                TEMP_SCROLL_TO.set(ui.ctx(), ((row, subrow), col_nr.unwrap_or_default()));
            }
        }
        None
    }

    fn draw_unnamed_sheet(&mut self, ui: &mut egui::Ui, _path: &Path, _params: &Params<'_, '_>) {
        self.draw_goto(ui.ctx());

        self.draw_sheet_list(ui);
    }

    fn draw_named_sheet(&mut self, ui: &mut egui::Ui, _path: &Path, _params: &Params<'_, '_>) {
        self.draw_goto(ui.ctx());

        self.draw_sheet_list(ui);
        self.draw_sheet_data(ui);
    }

    fn on_assets(&mut self, _ui: &mut egui::Ui, path: &Path, _params: &Params<'_, '_>) -> Redirect {
        if let Some(redirect) = self.ensure_backend(path) {
            return Some(redirect);
        }
        self.assets
            .selected()
            .map(|asset| format!("/assets/{asset}").into())
    }

    fn on_asset_path(
        &mut self,
        _ui: &mut egui::Ui,
        path: &Path,
        params: &Params<'_, '_>,
    ) -> Redirect {
        if let Some(redirect) = self.ensure_backend(path) {
            return Some(redirect);
        }
        let Some(asset) = params.get("path") else {
            return Some("/assets".into());
        };
        self.assets.request(asset.to_string());
        None
    }

    fn draw_assets(&mut self, ui: &mut egui::Ui, _path: &Path, _params: &Params<'_, '_>) {
        if let Some(backend) = self.backend.clone()
            && let Some(action) = self.assets.ui(ui, &backend)
        {
            match action {
                assets::Action::Select(asset) => self.navigate(format!("/assets/{asset}")),
                assets::Action::Navigate(route) => self.navigate(route),
                assets::Action::Redirect(asset) => {
                    self.navigate_replace(format!("/assets/{asset}"))
                }
            }
        }
    }

    fn on_icons(&mut self, _ui: &mut egui::Ui, path: &Path, _params: &Params<'_, '_>) -> Redirect {
        if let Some(redirect) = self.ensure_backend(path) {
            return Some(redirect);
        }
        self.icons
            .selected()
            .map(|icon_id| format!("/icons/{icon_id}").into())
    }

    fn on_icon(&mut self, _ui: &mut egui::Ui, path: &Path, params: &Params<'_, '_>) -> Redirect {
        if let Some(redirect) = self.ensure_backend(path) {
            return Some(redirect);
        }
        match params.get("id").and_then(|id| id.parse::<u32>().ok()) {
            Some(icon_id) => {
                self.icons.request(icon_id);
                None
            }
            None => Some("/icons".into()),
        }
    }

    fn draw_icons(&mut self, ui: &mut egui::Ui, _path: &Path, _params: &Params<'_, '_>) {
        if let Some(backend) = self.backend.clone()
            && let Some(action) = self.icons.ui(ui, &backend, &self.icon_manager)
        {
            match action {
                icons::Action::Select(icon_id) => self.navigate(format!("/icons/{icon_id}")),
                icons::Action::Navigate(route) => self.navigate(route),
            }
        }
    }

    fn on_music(&mut self, _ui: &mut egui::Ui, path: &Path, _params: &Params<'_, '_>) -> Redirect {
        if let Some(redirect) = self.ensure_backend(path) {
            return Some(redirect);
        }
        self.music
            .now_playing_row()
            .map(|id| format!("/music/{id}").into())
    }

    fn on_music_track(
        &mut self,
        _ui: &mut egui::Ui,
        path: &Path,
        params: &Params<'_, '_>,
    ) -> Redirect {
        if let Some(redirect) = self.ensure_backend(path) {
            return Some(redirect);
        }
        if let Some(id) = params.get("id").and_then(|id| id.parse::<u32>().ok()) {
            self.music.request(id);
        }
        None
    }

    fn draw_music(&mut self, ui: &mut egui::Ui, _path: &Path, _params: &Params<'_, '_>) {
        if let Some(backend) = self.backend.clone()
            && let Some(row_id) = self.music.ui(ui, &backend)
        {
            self.navigate(format!("/music/{row_id}"));
        }
    }

    fn command_open_pr(&mut self) {
        let names: Vec<String> = self
            .get_modified_schemas()
            .iter()
            .map(|(name, _)| (*name).clone())
            .collect();
        self.pr_window.open(&names);
    }

    fn draw_pr_window(&mut self, ctx: &egui::Context) {
        let location = pr_window::github_source(ctx);
        let modified: Vec<(String, Option<String>)> = self
            .get_modified_schemas()
            .iter()
            .map(|(name, schema)| ((*name).clone(), schema.invalid_reason()))
            .collect();
        if let Some(PrAction::Submit { title, body }) =
            self.pr_window.draw(ctx, location.as_ref(), &modified)
            && let Some(location) = &location
        {
            let files: Vec<(String, String)> = self
                .get_modified_schemas()
                .into_iter()
                .map(|(name, schema)| (format!("{name}.yml"), schema.get_text().clone()))
                .collect();
            self.pr_window.submit(location, title, body, files);
        }
    }

    fn get_modified_schemas(&self) -> Vec<(&String, &EditableSchema)> {
        self.schema_data
            .iter()
            .filter_map(|(name, schema)| schema.try_get().ok().map(|s| (name, s)))
            .filter_map(|(name, schema)| schema.as_ref().ok().map(|s| (name, s)))
            .filter(|(_, schema)| schema.is_modified())
            .collect()
    }

    fn command_export_csv(&mut self, context: TableContext, resolve_display_field: bool) {
        let file_name = format!("{}.csv", context.sheet().name().replace('/', "_"));

        self.export_promise = Some(TrackedPromise::spawn_local(async move {
            let data = match export_csv(context, resolve_display_field).await {
                Ok(data) => data,
                Err(e) => {
                    log::error!("Failed to export CSV: {e:?}");
                    return;
                }
            };

            if let Some(file) = rfd::AsyncFileDialog::new()
                .set_title("Export CSV")
                .set_file_name(file_name)
                .save_file()
                .await
            {
                if let Err(e) = file.write(&data).await {
                    log::error!("Failed to write CSV: {e}");
                } else {
                    log::info!("Exported CSV successfully");
                }
            }
        }));
    }

    fn command_save_all_schemas(&mut self) {
        let backend = self.backend.as_ref().unwrap();
        let modified_schemas = self.get_modified_schemas();

        if modified_schemas.is_empty() {
            log::info!("No modified schemas to save.");
            return;
        }

        let provider = backend.schema();
        let start_dir = provider
            .can_save_schemas()
            .then(|| provider.save_schema_start_dir())
            .flatten();

        if provider.can_save_schemas() {
            for (_, schema) in modified_schemas {
                schema.command_save(provider);
            }
        } else if let Ok((_, schema)) = modified_schemas.iter().exactly_one() {
            schema.command_save_as(provider);
        } else {
            let create_archive = || -> Result<Vec<u8>> {
                let mut archive = ZipWriter::new(std::io::Cursor::new(Vec::new()));
                for (sheet_name, schema) in modified_schemas {
                    archive
                        .start_file(format!("{sheet_name}.yml"), SimpleFileOptions::default())?;
                    archive.write_all(schema.get_text().as_bytes())?;
                }
                Ok(archive.finish()?.into_inner())
            };

            let archive = match create_archive() {
                Ok(archive) => archive,
                Err(e) => {
                    log::error!("Failed to create schema archive: {e}");
                    return;
                }
            };

            self.save_promise = Some(TrackedPromise::spawn_local(async move {
                let mut dialog = rfd::AsyncFileDialog::new()
                    .set_title("Save Schemas As")
                    .set_file_name("schemas.zip");
                if let Some(start_dir) = start_dir {
                    dialog = dialog.set_directory(start_dir);
                }
                if let Some(file) = dialog.save_file().await {
                    if let Err(e) = file.write(&archive).await {
                        log::error!("Failed to save schemas: {e}");
                    } else {
                        log::info!("Saved all saved successfully");
                    }
                }
            }));
        }
    }
}

impl App {
    #[must_use]
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_image_loaders(&cc.egui_ctx);
        Self::apply_fonts(&cc.egui_ctx, None);
        Self::setup_theme(&cc.egui_ctx);

        Self {
            router: Rc::new(OnceCell::new()),
            icon_manager: IconManager::new(),
            setup_window: None,
            backend: None,
            sheet_data: LruCache::new(NonZero::new(32).unwrap()),
            schema_data: LruCache::unbounded(),
            sheet_languages: LruCache::unbounded(),
            sheet_matcher: FuzzyMatcher::new(),
            sheet_filter_data: LruCache::new(NonZero::new(8).unwrap()),
            changed_schemas: None,
            save_promise: None,
            export_promise: None,
            pr_window: PrWindow::default(),
            goto_window: None,
            sheet_nav: ListNav::default(),
            about_open: false,
            music: music::MusicPlayer::default(),
            assets: assets::AssetBrowser::default(),
            icons: icons::IconBrowser::default(),
            last_system_theme: None,
            loaded_cjk: None,
            #[cfg(target_arch = "wasm32")]
            font_promise: None,
        }
    }

    fn apply_fonts(ctx: &egui::Context, cjk: Option<(String, Arc<FontData>)>) {
        let mut fonts = FontDefinitions::default();

        fonts.font_data.insert(
            "FFXIV-PrivateUseIcons".to_owned(),
            Arc::new(FontData::from_static(include_bytes!(
                "../assets/FFXIV_Lodestone_SSF.ttf"
            ))),
        );
        let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
        proportional.push("FFXIV-PrivateUseIcons".to_owned());

        if let Some((name, data)) = cjk {
            fonts.font_data.insert(name.clone(), data);
            proportional.push(name);
        }

        ctx.set_fonts(fonts);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn update_fonts(&mut self, ctx: &egui::Context) {
        let wanted = CjkFont::for_language(LANGUAGE.get(ctx));
        if wanted == self.loaded_cjk {
            return;
        }
        let cjk = wanted.map(|font| {
            (
                font.family_name().to_owned(),
                Arc::new(FontData::from_static(font.embedded_bytes())),
            )
        });
        Self::apply_fonts(ctx, cjk);
        self.loaded_cjk = wanted;
    }

    #[cfg(target_arch = "wasm32")]
    fn update_fonts(&mut self, ctx: &egui::Context) {
        let wanted = CjkFont::for_language(LANGUAGE.get(ctx));
        if wanted == self.loaded_cjk {
            return;
        }

        let Some(font) = wanted else {
            Self::apply_fonts(ctx, None);
            self.loaded_cjk = None;
            self.font_promise = None;
            return;
        };

        if self.font_promise.as_ref().is_some_and(|(f, _)| *f == font) {
            if !self.font_promise.as_ref().unwrap().1.ready() {
                return;
            }
            let (_, promise) = self.font_promise.take().unwrap();
            match promise.block_and_take() {
                Ok(bytes) => Self::apply_fonts(
                    ctx,
                    Some((
                        font.family_name().to_owned(),
                        Arc::new(FontData::from_owned(bytes)),
                    )),
                ),
                Err(error) => log::error!("Failed to fetch font {}: {error}", font.asset_file()),
            }
            self.loaded_cjk = Some(font);
            return;
        }

        let file = font.asset_file().to_owned();
        self.font_promise = Some((
            font,
            UnsendPromise::new(async move { crate::utils::fetch_url(file).await }),
        ));
    }

    fn setup_theme(ctx: &egui::Context) {
        COLOR_THEME.get(ctx).apply(ctx);
        let solid_scrollbar = SOLID_SCROLLBAR.get(ctx);
        ctx.all_styles_mut(|s| {
            s.spacing.scroll = if solid_scrollbar {
                ScrollStyle::solid()
            } else {
                ScrollStyle::default()
            };
            #[cfg(debug_assertions)]
            {
                s.debug.warn_if_rect_changes_id = false;
            }
        });
    }

    fn follow_system_theme(&mut self, ctx: &egui::Context) {
        if COLOR_THEME.get(ctx) != ColorTheme::System {
            return;
        }
        let system = ctx.system_theme();
        if system != self.last_system_theme {
            self.last_system_theme = system;
            ColorTheme::System.apply(ctx);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.follow_system_theme(ui.ctx());
        self.draw(ui);
        tick_promises(ui.ctx());
    }
}

fn add_links(ui: &mut egui::Ui, open_about: &mut bool) {
    ui.with_layout(Layout::right_to_left(ui.layout().vertical_align()), |ui| {
        if ui
            .link(format!("EXDViewer v{}", crate::build::PKG_VERSION))
            .clicked()
        {
            *open_about = true;
        }
        ui.label("/");
        ui.add(
            egui::Hyperlink::from_label_and_url(
                format!("Star me on {}", egui::special_emojis::GITHUB),
                crate::REPO_URL,
            )
            .open_in_new_tab(true),
        );
        egui::warn_if_debug_build(ui);
    });
}
