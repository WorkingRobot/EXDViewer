use egui::{
    Button, FontData, FontFamily, Label, Layout, RichText, ScrollArea, TextEdit, Vec2,
    epaint::text::{FontInsert, FontPriority, InsertFontFamily},
};
use ironworks::excel::Language;
use itertools::Itertools;

use crate::{
    backend::Backend,
    data::{AppConfig, AppState},
    editable_schema::EditableSchema,
    excel::provider::{ExcelHeader, ExcelProvider},
    future::{BackgroundInitializer, TrackedPromise, tick_promises},
    schema::provider::SchemaProvider,
    setup::{self, SetupWindow},
    sheet_table::SheetTable,
    syntax_highlighting::CodeTheme,
    value_cache::KeyedCache,
};

#[derive(Default)]
pub struct App {
    state: AppState,
    setup_window: setup::SetupWindow,
    backend: Option<BackgroundInitializer<Backend>>,
    sheet_data: KeyedCache<
        (Language, String),
        TrackedPromise<anyhow::Result<(SheetTable, EditableSchema)>>,
    >,
}

impl App {
    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.

        Self::setup_fonts(&cc.egui_ctx);

        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        if let Some(storage) = cc.storage {
            if let Some(state) = eframe::get_value(storage, eframe::APP_KEY) {
                return Self::from_state(state);
            }
        }

        Self::default()
    }

    fn from_state(state: AppState) -> Self {
        let mut ret = Self {
            state,
            ..Default::default()
        };
        if let Some(config) = &ret.state.config {
            ret.set_config(None, config.clone());
        }
        ret
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

    fn clear_config(&mut self) {
        self.state.config = None;
        self.backend = None;
    }

    fn set_config(&mut self, ctx: Option<&egui::Context>, config: AppConfig) {
        self.backend = Some(BackgroundInitializer::new(
            ctx,
            Backend::new(config.clone()),
        ));
        self.state.config = Some(config);
    }

    fn draw_with_backend(&mut self, ctx: &egui::Context, backend: &Backend) {
        egui::SidePanel::left("sheet_list").show(ctx, |ui| {
            egui::panel::TopBottomPanel::top("header").show_inside(ui, |ui| {
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    ui.heading("Sheets");
                });
                ui.add_space(4.0);
                ui.with_layout(Layout::right_to_left(egui::Align::Min), |ui| {
                    let resp = ui
                        .add_enabled(!self.state.current_filter.is_empty(), Button::new("↩"))
                        .on_hover_text("Clear");
                    if resp.clicked() {
                        self.state.current_filter.clear();
                    }
                    ui.toggle_value(&mut self.state.are_misc_sheets_shown, "🗄")
                        .on_hover_text("Show Miscellaneous Sheets");
                    ui.add_sized(
                        Vec2::new(ui.available_width(), 0.0),
                        TextEdit::singleline(&mut self.state.current_filter).hint_text("Filter"),
                    );
                });
                ui.add_space(4.0);
            });

            egui::TopBottomPanel::bottom("egui_credit").show_inside(ui, powered_by_egui_and_eframe);

            let sheets = backend
                .excel()
                .get_names()
                .iter()
                .sorted()
                .filter(|sheet| {
                    if self.state.are_misc_sheets_shown {
                        return true;
                    }
                    self.state.are_misc_sheets_shown || sheet.find('/').is_none()
                })
                .filter(|sheet| {
                    if self.state.current_filter.is_empty() {
                        return true;
                    }
                    sheet
                        .to_ascii_lowercase()
                        .contains(&self.state.current_filter.to_ascii_lowercase())
                })
                .collect_vec();

            egui::CentralPanel::default().show_inside(ui, |ui| {
                let row_height = ui.text_style_height(&egui::TextStyle::Button);
                ScrollArea::both().auto_shrink(false).show_rows(
                    ui,
                    row_height,
                    sheets.len(),
                    |ui, range| {
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                            for &sheet in sheets
                                .iter()
                                .skip(range.start)
                                .take(range.end - range.start)
                            {
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                                // let galley_ui = &mut ui.new_child(UiBuilder::new().sizing_pass());
                                // let (_, galley, _) = Label::new(sheet).extend().layout_in_ui(galley_ui);
                                let resp = ui.add(egui::SelectableLabel::new(
                                    self.state.current_sheet.as_ref() == Some(sheet),
                                    sheet.as_str(),
                                ));
                                if resp.clicked() {
                                    self.state.current_sheet = Some(sheet.clone());
                                }
                            }
                        });
                    },
                );
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let sheet_name = match self.state.current_sheet.as_ref() {
                Some(sheet) => sheet,
                None => return,
            };

            let sheet_data = self.sheet_data.get_or_set_ref(
                &(self.state.language, sheet_name.clone()),
                || -> TrackedPromise<anyhow::Result<(SheetTable, EditableSchema)>> {
                    let language = self.state.language;
                    let sheet_name = sheet_name.clone();
                    let excel = backend.excel().clone();
                    let schema = backend.schema().clone();

                    let ctx = ui.ctx().clone();
                    TrackedPromise::spawn_local(ctx.clone(), async move {
                        let (sheet, schema) = futures_util::try_join!(
                            excel.get_sheet(&sheet_name, language),
                            async { Ok(schema.get_schema_text(&sheet_name).await) },
                        )?;
                        let editor = match schema {
                            Ok(schema) => EditableSchema::new(sheet_name, schema),
                            Err(error) => {
                                // Soft-fail on schema retrieval/parsing errors
                                log::error!("Failed to get schema: {:?}", error);
                                EditableSchema::from_blank(sheet_name, sheet.columns().len())?
                            }
                        };

                        Ok((SheetTable::new(sheet, editor.get_schema().cloned()), editor))
                    })
                },
            );

            let (table, editor) = match sheet_data.ready_mut() {
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
                let mut visible = editor.visible();
                let resp = ui.horizontal(|ui| {
                    ui.set_min_height(ui.text_style_height(&egui::TextStyle::Heading));
                    ui.toggle_value(&mut visible, "Edit Schema")
                        .on_hover_text("Edit the schema for this sheet")
                });
                if resp.inner.changed() {
                    editor.set_visible(visible);
                }

                ui.add_sized(
                    Vec2::new(ui.available_width(), 0.0),
                    Label::new(RichText::new(sheet_name).heading()),
                );
            });

            ui.separator();

            let resp = editor.draw(
                ui,
                &mut self.state.schema_editor_word_wrap,
                backend.schema(),
            );
            if resp.changed() {
                if let Some(schema) = editor.get_schema() {
                    table.set_schema(schema.clone());
                }
            }

            table.draw(ui);
        });
    }
}

impl eframe::App for App {
    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.state);
    }

    /// Called each time the UI needs repainting, which may be many times per second.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        tick_promises(ctx);

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("App", |ui| {
                    if ui.button("Reset").clicked() {
                        if let Some(config) = self.state.config.clone() {
                            self.setup_window = SetupWindow::from_config(config);
                        } else {
                            self.setup_window = SetupWindow::new();
                        }
                        self.clear_config();
                    }
                    if !super::IS_WEB {
                        if ui.button("Quit").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                });

                ui.menu_button("Language", |ui| {
                    for lang in Language::iter() {
                        if lang != Language::None {
                            if ui
                                .selectable_value(&mut self.state.language, lang, lang.to_string())
                                .changed()
                            {
                                ui.close_menu();
                            }
                        }
                    }
                });

                ui.with_layout(Layout::right_to_left(ui.layout().vertical_align()), |ui| {
                    egui::widgets::global_theme_preference_buttons(ui);
                    ui.add_space(16.0);
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

        let config = match self.backend.as_ref().map(|b| b.result()) {
            None => self.setup_window.draw(ctx, None),
            Some(None) => self.setup_window.draw(ctx, Some(None)),
            Some(Some(Err(err))) => self.setup_window.draw(ctx, Some(Some(err))),
            Some(Some(Ok(backend))) => {
                self.draw_with_backend(ctx, &backend);
                None
            }
        };
        if let Some(config) = config {
            self.set_config(Some(ctx), config);
        }
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
