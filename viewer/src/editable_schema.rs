use crate::{
    schema::{Schema, boxed::BoxedSchemaProvider, provider::SchemaProvider},
    settings::{
        CODE_SYNTAX_THEME, SCHEMA_EDITOR_ERRORS_SHOWN, SCHEMA_EDITOR_VISIBLE,
        SCHEMA_EDITOR_WORD_WRAP,
    },
    shortcuts::{SCHEMA_CLEAR, SCHEMA_REVERT, SCHEMA_SAVE, SCHEMA_SAVE_AS},
    utils::{TrackedPromise, highlight, shortcut},
};
use egui::{
    CentralPanel, CornerRadius, Frame, Id, Layout, Margin, MenuBar, Response, RichText, TextBuffer,
    TopBottomPanel, collapsing_header::CollapsingState, epaint::text::cursor::LayoutCursor,
};
use itertools::Itertools;
use jsonschema::output::{ErrorDescription, OutputUnit};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};

pub struct EditableSchema {
    sheet_name: String,
    original: Rc<RefCell<String>>,
    text: String,
    is_modified: Rc<Cell<bool>>,
    schema: anyhow::Result<Result<Schema, VecDeque<OutputUnit<ErrorDescription>>>>,
    save_promise: Cell<Option<TrackedPromise<()>>>,
    save_as_promise: Cell<Option<TrackedPromise<()>>>,
}

impl EditableSchema {
    pub fn new(sheet_name: impl Into<String>, schema_text: String) -> Self {
        let schema = Schema::from_str(&schema_text);
        Self {
            sheet_name: sheet_name.into(),
            original: Rc::new(RefCell::new(schema_text.clone())),
            text: schema_text,
            is_modified: Rc::new(Cell::new(false)),
            schema,
            save_promise: Cell::new(None),
            save_as_promise: Cell::new(None),
        }
    }

    fn new_unchecked(schema: Schema) -> anyhow::Result<Self> {
        let text = serde_yml::to_string(&schema)?;
        Ok(Self {
            sheet_name: schema.name.clone(),
            original: Rc::new(RefCell::new(text.clone())),
            text,
            is_modified: Rc::new(Cell::new(false)),
            schema: Ok(Ok(schema)),
            save_promise: Cell::new(None),
            save_as_promise: Cell::new(None),
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
        self.is_modified.get()
    }

    pub fn get_schema(&self) -> Option<&Schema> {
        self.schema.as_ref().ok().and_then(|r| r.as_ref().ok())
    }

    pub fn draw(&mut self, ui: &mut egui::Ui, provider: &BoxedSchemaProvider) -> Response {
        let resp = self.draw_internal(ui, provider);
        if resp.changed() {
            self.schema = Schema::from_str(self.get_text());
            self.is_modified.set(self.text != *self.original.borrow());
        }
        resp
    }

    fn draw_internal(&mut self, ui: &mut egui::Ui, provider: &BoxedSchemaProvider) -> Response {
        let mut response = ui.response();

        let is_shown = SCHEMA_EDITOR_VISIBLE.get(ui.ctx());
        let mut is_shown_toggle = is_shown;

        let window_margin = ui.style().spacing.window_margin;
        egui::Window::new("Schema Editor")
            .open(&mut is_shown_toggle)
            .frame(Frame::window(ui.style()).inner_margin(Margin {
                top: window_margin.top,
                ..Default::default()
            }))
            .show(ui.ctx(), |ui| {
                let schema_editor_id = Id::new("schema-editor");
                let schema_editor_cursor_position_id = schema_editor_id.with("position");

                if shortcut::consume_ui(ui, SCHEMA_REVERT) && self.is_modified() {
                    self.command_revert();
                    response.mark_changed();
                }
                if shortcut::consume_ui(ui, SCHEMA_CLEAR) {
                    self.command_clear();
                    response.mark_changed();
                }
                if shortcut::consume_ui(ui, SCHEMA_SAVE) && provider.can_save_schemas() {
                    self.command_save(provider);
                }
                if shortcut::consume_ui(ui, SCHEMA_SAVE_AS) {
                    self.command_save_as(provider);
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

                        MenuBar::new().ui(ui, |ui| {
                            ui.menu_button("File", |ui| {
                                ui.add_enabled_ui(self.is_modified(), |ui| {
                                    if shortcut::button(ui, "Revert", SCHEMA_REVERT).clicked() {
                                        self.command_revert();
                                        response.mark_changed();
                                        ui.close();
                                    }
                                });
                                if shortcut::button(ui, "Clear", SCHEMA_CLEAR).clicked() {
                                    self.command_clear();
                                    response.mark_changed();
                                    ui.close();
                                }
                                ui.add_enabled_ui(
                                    self.is_modified() && provider.can_save_schemas(),
                                    |ui| {
                                        if shortcut::button(ui, "Save", SCHEMA_SAVE).clicked() {
                                            self.command_save(provider);
                                            ui.close();
                                        }
                                    },
                                );
                                if shortcut::button(ui, "Save As", SCHEMA_SAVE_AS).clicked() {
                                    self.command_save_as(provider);
                                    ui.close();
                                }
                            });

                            ui.menu_button("View", |ui| {
                                let mut word_wrap = SCHEMA_EDITOR_WORD_WRAP.get(ui.ctx());
                                if ui.toggle_value(&mut word_wrap, "Word Wrap").changed() {
                                    SCHEMA_EDITOR_WORD_WRAP.set(ui.ctx(), word_wrap);
                                    ui.close();
                                }
                            });

                            ui.with_layout(
                                Layout::right_to_left(ui.layout().vertical_align()),
                                |ui| {
                                    let mut errors_visible =
                                        SCHEMA_EDITOR_ERRORS_SHOWN.get(ui.ctx());
                                    let resp = ui.toggle_value(&mut errors_visible, "Show Errors");
                                    if resp.changed() {
                                        SCHEMA_EDITOR_ERRORS_SHOWN.set(ui.ctx(), errors_visible);
                                    }
                                },
                            );
                        });

                        error_panel_state.set_open(
                            !matches!(self.schema, Ok(Ok(_)))
                                && SCHEMA_EDITOR_ERRORS_SHOWN.get(ui.ctx()),
                        );
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
                                                RichText::new(format!("At {location}")).strong(),
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
                    MenuBar::new().ui(ui, |ui| {
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
                            let cursor = ui.data(|d| {
                                d.get_temp::<LayoutCursor>(schema_editor_cursor_position_id)
                            });

                            let mut add_separator = false;
                            if let Some(cursor) = cursor {
                                ui.label(format!(
                                    "Ln {}, Col {}",
                                    cursor.row + 1,
                                    cursor.column + 1
                                ));
                                add_separator = true;
                            }

                            if self.is_modified() {
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
                            let theme = CODE_SYNTAX_THEME.get(ui.ctx());

                            let mut layouter =
                                |ui: &egui::Ui, buf: &dyn TextBuffer, wrap_width: f32| {
                                    let mut layout_job = highlight(
                                        ui.ctx(),
                                        ui.style(),
                                        &theme,
                                        buf.as_str(),
                                        "yaml",
                                    );
                                    if SCHEMA_EDITOR_WORD_WRAP.get(ui.ctx()) {
                                        layout_job.wrap.max_width = wrap_width;
                                    }
                                    ui.fonts_mut(|f| f.layout_job(layout_job))
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
                                    d.insert_temp::<LayoutCursor>(
                                        schema_editor_cursor_position_id,
                                        ret.galley.layout_from_cursor(range.primary),
                                    );
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
            SCHEMA_EDITOR_VISIBLE.set(ui.ctx(), is_shown_toggle);
        }

        response
    }

    fn command_revert(&mut self) {
        self.text.replace_with(&self.original.borrow());
    }

    fn command_clear(&mut self) {
        TextBuffer::clear(&mut self.text);
    }

    pub fn command_save(&self, provider: &BoxedSchemaProvider) {
        let sheet_name = self.sheet_name.clone();
        let sheet_data = self.text.clone();
        let provider = provider.clone();

        let original = self.original.clone();
        let is_modified = self.is_modified.clone();

        self.save_promise
            .set(Some(TrackedPromise::spawn_local(async move {
                if let Err(e) = provider.save_schema(&sheet_name, &sheet_data).await {
                    log::error!("Failed to save schema: {e}");
                } else {
                    log::info!("Schema '{sheet_name}' saved successfully");
                    original.replace(sheet_data);
                    is_modified.set(false);
                }
            })));
    }

    pub fn command_save_as(&self, provider: &BoxedSchemaProvider) {
        let start_dir = provider
            .can_save_schemas()
            .then(|| provider.save_schema_start_dir())
            .flatten();

        let sheet_name = self.sheet_name.clone();
        let sheet_data = self.text.clone();

        self.save_as_promise
            .set(Some(TrackedPromise::spawn_local(async move {
                let mut dialog = rfd::AsyncFileDialog::new()
                    .set_title("Save Schema As")
                    .set_file_name(format!("{sheet_name}.yml"));
                if let Some(start_dir) = start_dir {
                    dialog = dialog.set_directory(start_dir);
                }
                if let Some(file) = dialog.save_file().await {
                    if let Err(e) = file.write(sheet_data.as_bytes()).await {
                        log::error!("Failed to save schema: {e}");
                    } else {
                        log::info!("Schema '{sheet_name}' saved successfully");
                    }
                }
            })));
    }
}
