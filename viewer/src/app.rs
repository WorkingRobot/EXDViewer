use std::{cell::OnceCell, num::NonZero, rc::Rc};

use egui::{
    Button, CentralPanel, FontData, FontFamily, Label, Layout, RichText, ScrollArea, TextEdit,
    Vec2, Widget,
    epaint::text::{FontInsert, FontPriority, InsertFontFamily},
};
use egui_extras::install_image_loaders;
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use ironworks::excel::Language;
use itertools::Itertools;
use lru::LruCache;
use matchit::Params;

use crate::{
    backend::Backend,
    editable_schema::EditableSchema,
    excel::{
        base::BaseSheet,
        provider::{ExcelHeader, ExcelProvider},
    },
    router::{Router, path::Path},
    schema::provider::SchemaProvider,
    settings::{
        ALWAYS_HIRES, BACKEND_CONFIG, LANGUAGE, LOGGER_SHOWN, MISC_SHEETS_SHOWN, SELECTED_SHEET,
        SHEETS_FILTER, SORTED_BY_OFFSET, TEMP_HIGHLIGHTED_ROW_NR, TEMP_SCROLL_TO_COL,
        TEMP_SCROLL_TO_ROW,
    },
    setup::{self, SetupWindow},
    sheet::{GlobalContext, SheetTable, TableContext},
    utils::{CodeTheme, ConvertiblePromise, IconManager, TrackedPromise, tick_promises},
};

pub struct App {
    router: Rc<OnceCell<Router<Self>>>,
    icon_manager: IconManager,
    setup_window: Option<setup::SetupWindow>,
    backend: Option<Backend>,
    sheet_data: LruCache<
        (Language, String),
        ConvertiblePromise<
            TrackedPromise<anyhow::Result<(BaseSheet, Option<anyhow::Result<String>>)>>,
            anyhow::Result<(SheetTable, EditableSchema)>,
        >,
    >,
    sheet_matcher: SkimMatcherV2,
}

impl Default for App {
    fn default() -> Self {
        Self {
            router: Rc::new(OnceCell::new()),
            icon_manager: IconManager::new(),
            setup_window: None,
            backend: None,
            sheet_data: LruCache::new(NonZero::new(32).unwrap()),
            sheet_matcher: SkimMatcherV2::default(),
        }
    }
}

fn create_router(ctx: egui::Context) -> anyhow::Result<Router<App>> {
    let mut builder = Router::<App>::new(ctx);
    builder.add_route("/", App::on_setup, App::draw_setup)?;
    builder.add_route("/sheet", App::on_unnamed_sheet, App::draw_unnamed_sheet)?;
    builder.add_route("/sheet/{name}", App::on_named_sheet, App::draw_named_sheet)?;
    Ok(builder)
}

impl App {
    fn draw(&mut self, ctx: &egui::Context) {
        self.router
            .get_or_init(|| create_router(ctx.clone()).unwrap());

        if cfg!(debug_assertions) && !super::IS_WEB {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                "EXDViewer - {}",
                self.router.get().as_ref().unwrap().current_path(),
            )));
        }

        self.draw_menubar(ctx);
        self.draw_logger(ctx);

        CentralPanel::default().show(ctx, |ui| {
            self.draw_router(ui);
        });
    }

    fn draw_router(&mut self, ui: &mut egui::Ui) {
        self.router.clone().get().unwrap().ui(self, ui);
    }

    fn navigate(&mut self, path: impl Into<Path>) {
        self.router.get().unwrap().navigate(path).unwrap()
    }

    fn navigate_replace(&mut self, path: impl Into<Path>) {
        self.router.get().unwrap().replace(path).unwrap()
    }

    fn draw_menubar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("App", |ui| {
                    if ui.button("Configure").clicked() {
                        self.navigate("/");
                        ui.close_menu();
                    }
                    if !super::IS_WEB {
                        if ui.button("Quit").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            ui.close_menu();
                        }
                    }
                });

                ui.menu_button("Language", |ui| {
                    let mut saved_lang = LANGUAGE.get(ctx);
                    for lang in Language::iter() {
                        if lang != Language::None {
                            if ui
                                .selectable_value(&mut saved_lang, lang, lang.to_string())
                                .changed()
                            {
                                LANGUAGE.set(ctx, lang);
                                ui.close_menu();
                            }
                        }
                    }
                });

                ui.menu_button("View", |ui| {
                    ui.menu_button("Sort Columns by", |ui| {
                        let mut sorted_by_offset = SORTED_BY_OFFSET.get(ctx);
                        if ui.toggle_value(&mut sorted_by_offset, "Offset").changed() {
                            SORTED_BY_OFFSET.set(ctx, sorted_by_offset);
                            ui.close_menu();
                        }

                        let mut sort_by_index = !sorted_by_offset;
                        if ui.toggle_value(&mut sort_by_index, "Index").changed() {
                            SORTED_BY_OFFSET.set(ctx, !sort_by_index);
                            ui.close_menu();
                        }
                    });

                    {
                        let mut always_hires = ALWAYS_HIRES.get(ctx);
                        if ui.checkbox(&mut always_hires, "Use HD Icons").changed() {
                            ALWAYS_HIRES.set(ctx, always_hires);
                            ui.close_menu();
                        }
                    }

                    {
                        let mut logger_shown = LOGGER_SHOWN.get(ctx);
                        if ui.checkbox(&mut logger_shown, "Show Log Window").changed() {
                            LOGGER_SHOWN.set(ctx, logger_shown);
                            ui.close_menu();
                        }
                    }
                });

                ui.with_layout(Layout::right_to_left(ui.layout().vertical_align()), |ui| {
                    egui::widgets::global_theme_preference_buttons(ui);

                    ui.menu_button("Code Theme", |ui| {
                        let mut theme = CodeTheme::from_memory(ui.ctx(), ui.style());

                        for (id, name) in CodeTheme::themes() {
                            if ui.selectable_label(theme.theme == id, name).clicked() {
                                theme.theme = id.to_owned();
                                ui.close_menu();
                            }
                        }
                        theme.store_in_memory(ui.ctx());
                    });
                });
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

    fn draw_sheet_list(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sheet_list").show(ctx, |ui| {
            egui::panel::TopBottomPanel::top("header").show_inside(ui, |ui| {
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    ui.heading("Sheets");
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

                    if ui
                        .add_sized(
                            Vec2::new(ui.available_width(), 0.0),
                            TextEdit::singleline(&mut sheets_filter).hint_text("Filter"),
                        )
                        .changed()
                    {
                        SHEETS_FILTER.set(ctx, sheets_filter);
                    }
                });
                ui.add_space(4.0);
            });

            egui::TopBottomPanel::bottom("egui_credit").show_inside(ui, powered_by_egui_and_eframe);

            let sheets_filter = SHEETS_FILTER.get(ctx);
            let misc_sheets_shown = MISC_SHEETS_SHOWN.get(ctx);
            let backend = self.backend.as_ref().cloned().unwrap();
            let sheets = backend
                .excel()
                .get_entries()
                .iter()
                .sorted_by_key(|(sheet, _)| *sheet)
                .filter(|(_, id)| {
                    if misc_sheets_shown {
                        return true;
                    }
                    misc_sheets_shown || **id >= 0
                })
                .filter_map(|(sheet, id)| {
                    if sheets_filter.is_empty() {
                        return Some((0, sheet, id));
                    }
                    self.sheet_matcher
                        .fuzzy_match(sheet.as_str(), &sheets_filter)
                        .map(|score| (score, sheet, id))
                })
                .sorted_unstable_by_key(|(score, _, _)| -score)
                .map(|(_, a, b)| (a, b))
                .collect_vec();

            egui::CentralPanel::default().show_inside(ui, |ui| {
                let row_height = ui.text_style_height(&egui::TextStyle::Button);
                ScrollArea::both().auto_shrink(false).show_rows(
                    ui,
                    row_height,
                    sheets.len(),
                    |ui, range| {
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                            let mut current_sheet = SELECTED_SHEET.get(ctx);
                            for &(sheet, &id) in sheets
                                .iter()
                                .skip(range.start)
                                .take(range.end - range.start)
                            {
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                                let resp = egui::SelectableLabel::new(
                                    current_sheet.as_ref() == Some(sheet),
                                    sheet.as_str(),
                                )
                                .ui(ui)
                                .on_hover_text(format!("{sheet}\nId: {id}"));
                                if resp.clicked() {
                                    current_sheet = Some(sheet.clone());
                                    SELECTED_SHEET.set(ctx, current_sheet.clone());
                                    self.navigate(format!("/sheet/{}", sheet.clone()));
                                }
                            }
                        });
                    },
                );
            });
        });
    }

    fn draw_sheet_data(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let backend = self.backend.as_ref().unwrap();
            let sheet_name = SELECTED_SHEET.get(ctx).unwrap();
            let language = LANGUAGE.get(ctx);

            let sheet_data =
                self.sheet_data
                    .get_or_insert_mut_ref(&(language, sheet_name.clone()), || {
                        let sheet_name = sheet_name.clone();
                        let is_sheet_miscellaneous = backend
                            .excel()
                            .get_entries()
                            .get(&sheet_name)
                            .cloned()
                            .unwrap_or_default()
                            < 0;
                        let excel = backend.excel().clone();
                        let schema = backend.schema().clone();

                        ConvertiblePromise::new_promise(TrackedPromise::spawn_local(async move {
                            Ok(futures_util::try_join!(
                                excel.get_sheet(&sheet_name, language),
                                async {
                                    if !is_sheet_miscellaneous {
                                        Ok(Some(schema.get_schema_text(&sheet_name).await))
                                    } else {
                                        Ok(None)
                                    }
                                },
                            )?)
                        }))
                    });

            let sheet_data = sheet_data.get(|result| {
                result.and_then(|(sheet, schema)| {
                    let sheet_name = sheet.name().to_owned();
                    let editor = match schema {
                        Some(Ok(schema)) => EditableSchema::new(sheet_name, schema),
                        Some(Err(error)) => {
                            // Soft-fail on schema retrieval/parsing errors
                            log::error!("Failed to get schema: {:?}", error);
                            EditableSchema::from_blank(sheet_name, sheet.columns().len())?
                        }
                        None => EditableSchema::from_miscellaneous(sheet_name)?,
                    };
                    let table = SheetTable::new(TableContext::new(
                        GlobalContext::new(
                            ui.ctx().clone(),
                            backend.clone(),
                            language,
                            self.icon_manager.clone(),
                        ),
                        sheet.clone(),
                        editor.get_schema().cloned(),
                    ));
                    Ok((table, editor))
                })
            });

            let (table, editor) = match sheet_data {
                Some(Ok(data)) => data,
                Some(Err(err)) => {
                    ui.label("Failed to load sheet");
                    ui.label(err.to_string());
                    return;
                }
                None => {
                    ui.label("Loading...");
                    return;
                }
            };

            ui.with_layout(Layout::right_to_left(egui::Align::Min), |ui| {
                let is_miscellaneous = backend
                    .excel()
                    .get_entries()
                    .get(&sheet_name)
                    .cloned()
                    .unwrap_or_default()
                    < 0;

                ui.add_enabled_ui(!is_miscellaneous, |ui| {
                    let mut visible = editor.visible(ui);
                    let resp = ui.horizontal(|ui| {
                        ui.set_min_height(ui.text_style_height(&egui::TextStyle::Heading));
                        ui.toggle_value(&mut visible, "Edit Schema")
                            .on_hover_text("Edit the schema for this sheet")
                    });
                    if resp.inner.changed() {
                        editor.set_visible(ui, visible);
                    }
                });

                ui.add_sized(
                    Vec2::new(ui.available_width(), 0.0),
                    Label::new(RichText::new(sheet_name).heading()),
                );
            });

            ui.separator();

            let resp = editor.draw(ui, backend.schema());
            if resp.changed() {
                if let Some(schema) = editor.get_schema() {
                    if let Err(e) = table.context().set_schema(Some(schema.clone())) {
                        log::error!("Failed to set schema: {:?}", e);
                    }
                }
            }

            let mut row_nr: Option<u64> = None;
            if let Some((row, subrow)) = TEMP_SCROLL_TO_ROW.take(ctx) {
                match table.get_row_nr(row, subrow) {
                    Ok(nr) => {
                        row_nr = Some(nr);
                    }
                    Err(e) => {
                        log::error!("Failed to scroll to row: {:?}", e);
                    }
                }
            }

            let mut col_nr: Option<u16> = None;
            if let Some(col) = TEMP_SCROLL_TO_COL.take(&ctx) {
                col_nr = Some(col);
            }

            table.draw(ui, move |mut table| {
                if let Some(row_nr) = row_nr {
                    TEMP_HIGHLIGHTED_ROW_NR.set(ctx, row_nr);
                    table = table.scroll_to_row(row_nr, Some(egui::Align::Center));
                }
                if let Some(col_nr) = col_nr {
                    table = table.scroll_to_column(col_nr as usize, Some(egui::Align::Center));
                }
                table
            });
        });
    }

    fn on_setup(
        &mut self,
        ui: &mut egui::Ui,
        path: &Path,
        _params: &Params<'_, '_>,
    ) -> Result<(), Path> {
        self.setup_window = Some(SetupWindow::from_config(
            ui.ctx(),
            path.query_pairs().contains_key("redirect"),
        ));
        Ok(())
    }

    fn draw_setup(&mut self, ui: &mut egui::Ui, path: &Path, _params: &Params<'_, '_>) {
        if let Some((backend, config)) = self.setup_window.as_mut().unwrap().draw(ui.ctx()) {
            self.backend = Some(backend);
            BACKEND_CONFIG.set(ui.ctx(), Some(config));
            if let Some(redirect_path) = path.query_pairs().get("redirect").map(|s| s.as_str()) {
                self.navigate_replace(redirect_path);
            } else {
                self.navigate("/sheet");
            }
        }
    }

    fn ensure_backend(&self, path: &Path) -> Result<(), Path> {
        if self.backend.is_none() {
            return Err(Path::with_params("/", &[("redirect", path.to_string())]));
        }
        Ok(())
    }

    fn on_unnamed_sheet(
        &mut self,
        ui: &mut egui::Ui,
        path: &Path,
        _params: &Params<'_, '_>,
    ) -> Result<(), Path> {
        self.ensure_backend(path)?;

        if let Some(sheet) = &SELECTED_SHEET.get(ui.ctx()) {
            return Err(format!("/sheet/{sheet}").into());
        }
        Ok(())
    }

    fn on_named_sheet(
        &mut self,
        ui: &mut egui::Ui,
        path: &Path,
        params: &Params<'_, '_>,
    ) -> Result<(), Path> {
        self.ensure_backend(path)?;

        if let Some(sheet) = params.get("name") {
            SELECTED_SHEET.set(ui.ctx(), Some(sheet.to_string()));
        } else {
            SELECTED_SHEET.set(ui.ctx(), None);
            return Err("/sheet".into());
        }

        if let Some(mut fragment) = path.fragment() {
            if let Some((rest, col_str)) = fragment.rsplit_once("C") {
                if let Ok(col) = col_str.parse::<u16>() {
                    TEMP_SCROLL_TO_COL.set(ui.ctx(), col);
                }
                fragment = rest;
            }

            if let Some((_rest, row_str)) = fragment.rsplit_once("R") {
                if let Some((row_str, subrow_str)) = row_str.split_once(".") {
                    let row = row_str.parse::<u32>().ok();
                    let subrow = subrow_str.parse::<u16>().ok();
                    if let Some(row) = row {
                        TEMP_SCROLL_TO_ROW.set(ui.ctx(), (row, subrow));
                    }
                } else {
                    if let Ok(row) = row_str.parse::<u32>() {
                        TEMP_SCROLL_TO_ROW.set(ui.ctx(), (row, None));
                    }
                }
            }
        }
        Ok(())
    }

    fn draw_unnamed_sheet(&mut self, ui: &mut egui::Ui, _path: &Path, _params: &Params<'_, '_>) {
        self.draw_sheet_list(ui.ctx());
    }

    fn draw_named_sheet(&mut self, ui: &mut egui::Ui, _path: &Path, _params: &Params<'_, '_>) {
        self.draw_sheet_list(ui.ctx());
        self.draw_sheet_data(ui.ctx());
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_image_loaders(&cc.egui_ctx);
        Self::setup_fonts(&cc.egui_ctx);

        Self::default()
    }

    fn setup_fonts(ctx: &egui::Context) {
        ctx.add_font(FontInsert::new(
            "NotoSans-JP",
            FontData::from_static(include_bytes!("../assets/NotoSansJP-Medium.ttf")),
            vec![InsertFontFamily {
                family: FontFamily::Proportional,
                priority: FontPriority::Lowest,
            }],
        ));
        ctx.add_font(FontInsert::new(
            "NotoSans-KR",
            FontData::from_static(include_bytes!("../assets/NotoSansKR-Medium.ttf")),
            vec![InsertFontFamily {
                family: FontFamily::Proportional,
                priority: FontPriority::Lowest,
            }],
        ));
        ctx.add_font(FontInsert::new(
            "FFXIV-PrivateUseIcons",
            FontData::from_static(include_bytes!("../assets/FFXIV_Lodestone_SSF.ttf")),
            vec![InsertFontFamily {
                family: FontFamily::Proportional,
                priority: FontPriority::Lowest,
            }],
        ));
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.draw(ctx);
        tick_promises(ctx);
    }
}

fn powered_by_egui_and_eframe(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label("Powered by ");
        ui.hyperlink_to("egui", "https://github.com/emilk/egui");
        ui.label(" and ");
        ui.hyperlink_to(
            "eframe",
            "https://github.com/emilk/egui/tree/master/crates/eframe",
        );
        ui.label(".");
    });
}
