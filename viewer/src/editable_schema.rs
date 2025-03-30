use crate::{
    schema::{Schema, boxed::BoxedSchemaProvider, provider::SchemaProvider},
    syntax_highlighting,
    utils::TrackedPromise,
};
use egui::{
    CentralPanel, CornerRadius, Frame, Id, Key, KeyboardShortcut, Layout, Margin, Modifiers,
    Response, RichText, TextBuffer, TopBottomPanel, Widget, collapsing_header::CollapsingState,
    text::CursorRange,
};
use itertools::Itertools;
use jsonschema::output::{ErrorDescription, OutputUnit};
use std::collections::VecDeque;

pub struct EditableSchema {
    sheet_name: String,
    original: String,
    text: String,
    is_modified: bool,
    schema: anyhow::Result<Result<Schema, VecDeque<OutputUnit<ErrorDescription>>>>,
    save_promise: Option<TrackedPromise<()>>,
}

impl EditableSchema {
    pub fn new(sheet_name: impl Into<String>, schema_text: String) -> Self {
        let schema = Schema::from_str(&schema_text);
        Self {
            sheet_name: sheet_name.into(),
            original: schema_text.clone(),
            text: schema_text,
            is_modified: false,
            schema,
            save_promise: None,
        }
    }

    fn new_unchecked(schema: Schema) -> anyhow::Result<Self> {
        let text = serde_yml::to_string(&schema)?;
        Ok(Self {
            sheet_name: schema.name.clone(),
            original: text.clone(),
            text,
            is_modified: false,
            schema: Ok(Ok(schema)),
            save_promise: None,
        })
    }

    pub fn from_blank(sheet_name: impl Into<String>, column_count: usize) -> anyhow::Result<Self> {
        Self::new_unchecked(Schema::from_blank(sheet_name, column_count))
    }

    pub fn from_miscellaneous(sheet_name: impl Into<String>) -> anyhow::Result<Self> {
        Self::new_unchecked(Schema::misc_sheet(sheet_name))
    }

    pub fn get_text(&self) -> &String {
        &self.text
    }

    pub fn is_modified(&self) -> bool {
        self.is_modified
    }

    pub fn get_schema(&self) -> Option<&Schema> {
        self.schema.as_ref().ok().and_then(|r| r.as_ref().ok())
    }

    pub fn set_visible(&mut self, ui: &mut egui::Ui, visible: bool) {
        let id = Id::new("schema-editor-visible");
        ui.data_mut(|d| {
            d.insert_persisted(id, visible);
        });
    }

    pub fn visible(&mut self, ui: &mut egui::Ui) -> bool {
        let id = Id::new("schema-editor-visible");
        ui.data_mut(|d| d.get_persisted(id).unwrap_or_default())
    }

    fn set_errors_visible(&mut self, ui: &mut egui::Ui, visible: bool) {
        let id = Id::new("schema-editor-errors-shown");
        ui.data_mut(|d| {
            d.insert_persisted(id, visible);
        });
    }

    fn errors_visible(&mut self, ui: &mut egui::Ui) -> bool {
        let id = Id::new("schema-editor-errors-shown");
        ui.data_mut(|d| d.get_persisted(id).unwrap_or_default())
    }

    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        word_wrap_enabled: &mut bool,
        provider: &BoxedSchemaProvider,
    ) -> Response {
        let resp = self.draw_internal(ui, word_wrap_enabled, provider);
        if resp.changed() {
            self.schema = Schema::from_str(self.get_text());
            self.is_modified = self.text != self.original;
        }
        resp
    }

    fn draw_internal(
        &mut self,
        ui: &mut egui::Ui,
        word_wrap_enabled: &mut bool,
        provider: &BoxedSchemaProvider,
    ) -> Response {
        let mut response = ui.response();

        let is_shown = self.visible(ui);
        let mut is_shown_toggle = is_shown;

        let window_margin = ui.style().spacing.window_margin;
        egui::Window::new("Schema Editor")
            .open(&mut is_shown_toggle)
            .frame(Frame::window(ui.style()).inner_margin(Margin {
                top: window_margin.top,
                ..Default::default()
            }))
            .show(ui.ctx(), |ui| {
                let shortcut_revert = KeyboardShortcut::new(Modifiers::CTRL, Key::R);
                let shortcut_clear = KeyboardShortcut::new(Modifiers::CTRL, Key::N);
                let shortcut_save = KeyboardShortcut::new(Modifiers::CTRL, Key::S);
                let shortcut_save_as =
                    KeyboardShortcut::new(Modifiers::CTRL | Modifiers::SHIFT, Key::S);
                let schema_editor_id = Id::new("schema-editor");
                let schema_editor_cursor_position_id = schema_editor_id.with("position");

                if self.is_modified() {
                    if consume_shortcut(ui, &shortcut_revert) {
                        self.command_revert();
                        response.mark_changed();
                    }
                }
                if consume_shortcut(ui, &shortcut_clear) {
                    self.command_clear();
                    response.mark_changed();
                }
                if provider.can_save_schemas() {
                    if consume_shortcut(ui, &shortcut_save) {
                        if let Err(e) = provider.save_schema(&self.sheet_name, self.get_text()) {
                            log::error!("Failed to save schema: {}", e);
                        }
                    }
                }
                if consume_shortcut(ui, &shortcut_save_as) {
                    self.command_save_as(ui.ctx(), provider);
                }

                TopBottomPanel::top("editor-top-bar")
                    .frame(Frame::side_top_panel(ui.style()).inner_margin(Margin {
                        top: 2,
                        bottom: window_margin.bottom,
                        left: 8,
                        right: 8,
                    }))
                    .show_inside(ui, |ui| {
                        let mut error_panel_state = CollapsingState::load_with_default_open(
                            ui.ctx(),
                            Id::new("schema-editor-errors-shown"),
                            false,
                        );

                        egui::menu::bar(ui, |ui| {
                            ui.menu_button("File", |ui| {
                                ui.add_enabled_ui(self.is_modified(), |ui| {
                                    if shortcut_button(ui, "Revert", &shortcut_revert).clicked() {
                                        self.command_revert();
                                        response.mark_changed();
                                        ui.close_menu();
                                    }
                                });
                                if shortcut_button(ui, "Clear", &shortcut_clear).clicked() {
                                    self.command_clear();
                                    response.mark_changed();
                                    ui.close_menu();
                                }
                                ui.add_enabled_ui(
                                    self.is_modified() && provider.can_save_schemas(),
                                    |ui| {
                                        if shortcut_button(ui, "Save", &shortcut_save).clicked() {
                                            if let Err(e) = provider
                                                .save_schema(&self.sheet_name, self.get_text())
                                            {
                                                log::error!("Failed to save schema: {}", e);
                                            }
                                            ui.close_menu();
                                        }
                                    },
                                );
                                if shortcut_button(ui, "Save As", &shortcut_save_as).clicked() {
                                    self.command_save_as(ui.ctx(), provider);
                                    ui.close_menu();
                                }
                            });

                            ui.menu_button("View", |ui| {
                                if ui.toggle_value(word_wrap_enabled, "Word Wrap").changed() {
                                    ui.close_menu();
                                }
                            });

                            ui.with_layout(
                                Layout::right_to_left(ui.layout().vertical_align()),
                                |ui| {
                                    let mut errors_visible = self.errors_visible(ui);
                                    let resp = ui.toggle_value(&mut errors_visible, "Show Errors");
                                    if resp.changed() {
                                        self.set_errors_visible(ui, errors_visible);
                                    }
                                },
                            );
                        });

                        error_panel_state
                            .set_open(!matches!(self.schema, Ok(Ok(_))) && self.errors_visible(ui));
                        error_panel_state.show_body_unindented(ui, |ui| {
                            ui.separator();
                            egui::ScrollArea::vertical()
                                .auto_shrink(false)
                                .max_height(100.0)
                                .show(ui, |ui| match &self.schema {
                                    Ok(Err(errors)) => {
                                        for (location, errors) in
                                            &errors.iter().chunk_by(|e| e.instance_location())
                                        {
                                            let location = match location.as_str() {
                                                loc if !loc.is_empty() => loc,
                                                _ => "/",
                                            };
                                            ui.label(
                                                RichText::new(format!("At {}", location)).strong(),
                                            );
                                            ui.indent(location, |ui| {
                                                for error in errors {
                                                    ui.label(error.error_description().to_string());
                                                }
                                            });
                                        }
                                    }
                                    Err(err) => {
                                        ui.label(err.to_string());
                                    }
                                    _ => {}
                                });
                        });
                    });

                TopBottomPanel::bottom("status-panel").show_inside(ui, |ui| {
                    egui::menu::bar(ui, |ui| {
                        let validation_text: String = match &self.schema {
                            Ok(Ok(_)) => "Valid Schema".into(),
                            Ok(Err(e)) => format!(
                                "Invalid Schema ({} error{})",
                                e.len(),
                                if e.len() != 1 { "s" } else { "" }
                            ),
                            Err(_) => "Invalid Schema (Error when validating)".into(),
                        };
                        ui.label(validation_text);
                        ui.with_layout(Layout::right_to_left(ui.layout().vertical_align()), |ui| {
                            let cursor = ui
                                .data(|d| {
                                    d.get_temp::<CursorRange>(schema_editor_cursor_position_id)
                                })
                                .map(|range| range.primary.rcursor);

                            let mut add_separator = false;
                            if let Some(cursor) = cursor {
                                ui.label(format!(
                                    "Ln {}, Col {}",
                                    cursor.row + 1,
                                    cursor.column + 1
                                ));
                                add_separator = true;
                            }

                            if self.is_modified {
                                if add_separator {
                                    ui.separator();
                                }
                                ui.label("Modified");
                            }
                        });
                    });
                });

                let corner_radius = ui.style().visuals.window_corner_radius;
                CentralPanel::default()
                    .frame(
                        Frame::central_panel(ui.style())
                            .inner_margin(0)
                            .corner_radius(CornerRadius {
                                sw: corner_radius.sw,
                                se: corner_radius.se,
                                ..Default::default()
                            }),
                    )
                    .show_inside(ui, |ui| {
                        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                            let theme =
                                syntax_highlighting::CodeTheme::from_memory(ui.ctx(), ui.style());

                            let mut layouter = |ui: &egui::Ui, string: &str, wrap_width: f32| {
                                let mut layout_job = syntax_highlighting::highlight(
                                    ui.ctx(),
                                    ui.style(),
                                    &theme,
                                    string,
                                    "yaml",
                                );
                                if *word_wrap_enabled {
                                    layout_job.wrap.max_width = wrap_width;
                                }
                                ui.fonts(|f| f.layout_job(layout_job))
                            };

                            let ret = {
                                let layout = (*ui.layout()).with_main_justify(true);
                                ui.allocate_ui_with_layout(ui.available_size(), layout, |ui| {
                                    ui.style_mut().visuals.selection.stroke.width = 0.0;
                                    ui.style_mut().visuals.widgets.hovered.bg_stroke.width = 0.0;
                                    egui::TextEdit::multiline(&mut self.text)
                                        .id(schema_editor_id)
                                        .code_editor()
                                        .desired_width(f32::INFINITY)
                                        .layouter(&mut layouter)
                                        .show(ui)
                                })
                                .inner
                            };

                            if let Some(range) = ret.cursor_range {
                                ui.data_mut(|d| {
                                    d.insert_temp::<CursorRange>(
                                        schema_editor_cursor_position_id,
                                        range,
                                    )
                                });
                            }

                            if ret.response.changed() {
                                response.mark_changed();

                                let mut range = ret.state.cursor.char_range();
                                let mut modified = false;
                                // Replace tabs with spaces
                                while let Some((tab_idx, tab_char)) =
                                    self.text.char_indices().find(|&(_, c)| c == '\t')
                                {
                                    let replace_with = " ".repeat(4);
                                    self.text.replace_range(
                                        tab_idx..tab_idx + tab_char.len_utf8(),
                                        replace_with.as_str(),
                                    );
                                    // Adjust range if needed
                                    if let Some(range) = &mut range {
                                        let char_delta = replace_with.chars().count() - 1;
                                        if range.primary.index > tab_idx {
                                            range.primary.index += char_delta;
                                            modified = true;
                                        }
                                        if range.secondary.index > tab_idx {
                                            range.secondary.index += char_delta;
                                            modified = true;
                                        }
                                    }
                                }
                                if modified {
                                    let mut state = ret.state.clone();
                                    state.cursor.set_char_range(range);
                                    state.store(ui.ctx(), schema_editor_id);
                                    ui.ctx().request_discard(
                                        "Tab characters in schema editor was replaced with spaces",
                                    );
                                }
                            }
                            ret.response
                        })
                    })
            });

        if is_shown != is_shown_toggle {
            self.set_visible(ui, is_shown_toggle);
        }

        response
    }

    fn command_revert(&mut self) {
        self.text.replace_with(&self.original);
    }

    fn command_clear(&mut self) {
        TextBuffer::clear(&mut self.text);
    }

    fn command_save_as(&mut self, ctx: &egui::Context, provider: &BoxedSchemaProvider) {
        let start_dir = if provider.can_save_schemas() {
            Some(provider.save_schema_start_dir())
        } else {
            None
        };

        let sheet_name = self.sheet_name.clone();
        let sheet_data = self.text.clone();

        self.save_promise = Some(TrackedPromise::spawn_local(ctx.clone(), async move {
            let mut dialog = rfd::AsyncFileDialog::new()
                .set_title("Save Schema As")
                .set_file_name(format!("{}.yml", sheet_name));
            if let Some(start_dir) = start_dir {
                dialog = dialog.set_directory(start_dir);
            }
            if let Some(file) = dialog.save_file().await {
                if let Err(e) = file.write(sheet_data.as_bytes()).await {
                    log::error!("Failed to save schema: {}", e);
                }
            }
        }));
    }
}

fn shortcut_button(
    ui: &mut egui::Ui,
    text: impl Into<egui::WidgetText>,
    shortcut: &KeyboardShortcut,
) -> Response {
    egui::Button::new(text)
        .shortcut_text(ui.ctx().format_shortcut(shortcut))
        .ui(ui)
}

fn consume_shortcut(ui: &mut egui::Ui, shortcut: &KeyboardShortcut) -> bool {
    ui.input_mut(|i| i.consume_shortcut(shortcut))
}
