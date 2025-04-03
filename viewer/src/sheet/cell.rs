use anyhow::bail;
use egui::{
    Color32, Direction, InnerResponse, Layout, Sense, Vec2, Widget, color_picker::show_color_at,
    ecolor::HexColor,
};
use either::Either::{Left, Right};
use ironworks::file::exh::ColumnKind;

use crate::excel::{
    get_icon_path,
    provider::{ExcelProvider, ExcelRow, ExcelSheet},
};

use super::{
    copyable_label, schema_column::SchemaColumnMeta, sheet_column::SheetColumnDefinition,
    table_context::TableContext,
};

pub struct Cell<'a> {
    row: ExcelRow<'a>,
    schema_column: SchemaColumnMeta,
    sheet_column: &'a SheetColumnDefinition,
    table_context: &'a TableContext,
}

pub enum CellResponse {
    None,
    Icon(u32),
}

impl<'a> Cell<'a> {
    pub fn new(
        row: ExcelRow<'a>,
        schema_column: SchemaColumnMeta,
        sheet_column: &'a SheetColumnDefinition,
        table_context: &'a TableContext,
    ) -> Self {
        Self {
            row,
            schema_column,
            sheet_column,
            table_context,
        }
    }

    fn draw(self, ui: &mut egui::Ui) -> anyhow::Result<CellResponse> {
        match self.schema_column {
            SchemaColumnMeta::Scalar => {
                let value = read_string(
                    self.row,
                    self.sheet_column.offset() as u32,
                    self.sheet_column.kind(),
                )?;
                self.draw_copyable_text(ui, value);
            }
            SchemaColumnMeta::Icon => {
                let icon_id: u32 = read_integer(
                    self.row,
                    self.sheet_column.offset() as u32,
                    self.sheet_column.kind(),
                )?;
                if self.draw_icon(ui, icon_id) {
                    return Ok(CellResponse::Icon(icon_id));
                }
            }
            SchemaColumnMeta::ModelId => {
                let model_id: u64 = read_integer(
                    self.row,
                    self.sheet_column.offset() as u32,
                    self.sheet_column.kind(),
                )?;
                self.draw_copyable_text(ui, model_id.to_string());
            }
            SchemaColumnMeta::Color => {
                let color: u32 = read_integer(
                    self.row,
                    self.sheet_column.offset() as u32,
                    self.sheet_column.kind(),
                )?;
                let [a, r, g, b] = color.to_le_bytes();
                let color = Color32::from_rgba_unmultiplied(r, g, b, a);
                self.draw_color(ui, color);
            }
            SchemaColumnMeta::Link(sheets) => {
                let row_id: isize = read_integer(
                    self.row,
                    self.sheet_column.offset() as u32,
                    self.sheet_column.kind(),
                )?;

                match row_id
                    .try_into()
                    .ok()
                    .and_then(|id| self.table_context.resolve_link(sheets, id))
                {
                    Some(Some((sheet_name, table))) => {
                        if let Some(cell) =
                            table.display_field_cell(table.sheet().get_row(row_id as u32).unwrap())
                        {
                            cell?.draw(ui)?;
                        } else {
                            copyable_label(ui, format!("{sheet_name}#{row_id}"));
                        }
                    }
                    Some(None) => {
                        copyable_label(ui, format!("...#{row_id}"));
                    }
                    None => {
                        copyable_label(ui, format!("???#{row_id}"));
                    }
                }
            }
            SchemaColumnMeta::ConditionalLink { column_idx, links } => {
                let (_, switch_column) = self.table_context.get_column(column_idx)?;
                let switch_data: i32 = read_integer(
                    self.row,
                    switch_column.offset() as u32,
                    switch_column.kind(),
                )?;
                if let Some(sheets) = links.get(&switch_data) {
                    Cell {
                        row: self.row,
                        schema_column: SchemaColumnMeta::Link(sheets.clone()),
                        sheet_column: self.sheet_column,
                        table_context: self.table_context,
                    }
                    .draw(ui)?;
                } else {
                    copyable_label(ui, format!("???#{switch_data}"));
                }
            }
        }
        Ok(CellResponse::None)
    }

    fn draw_copyable_text(&self, ui: &mut egui::Ui, text: String) {
        copyable_label(ui, text);
    }

    fn draw_icon(&self, ui: &mut egui::Ui, icon_id: u32) -> bool {
        let (excel, icon_mgr) = (
            self.table_context.global().backend().excel(),
            &self.table_context.global().icon_manager(),
        );
        let image_source = icon_mgr.get_icon(icon_id).or_else(|| {
            log::info!("Icon not found in cache: {icon_id}");
            match excel.get_icon(icon_id) {
                Ok(Left(url)) => Some(icon_mgr.insert_icon_url(icon_id, url)),
                Ok(Right(image)) => Some(icon_mgr.insert_icon_texture(icon_id, ui.ctx(), image)),
                Err(err) => {
                    log::error!("Failed to get icon: {:?}", err);
                    None
                }
            }
        });
        let resp = if let Some(source) = image_source {
            ui.with_layout(
                Layout::centered_and_justified(Direction::LeftToRight),
                |ui| {
                    egui::Image::new(source)
                        .sense(Sense::click())
                        .maintain_aspect_ratio(true)
                        .fit_to_exact_size(Vec2::new(f32::INFINITY, 32.0))
                        .ui(ui)
                },
            )
            .inner
        } else {
            ui.label("Icon not found")
        };
        let resp = resp.on_hover_text(format!(
            "Id: {icon_id}\nPath: {}",
            get_icon_path(icon_id, true)
        ));
        resp.context_menu(|ui| {
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(icon_id.to_string());
                ui.close_menu();
            }
            // ui.add_enabled_ui(image_source.is_some(), |ui| {
            //     if ui.button("Save").clicked() {
            //         image_source.unwrap().load(ctx, texture_options, size_hint)
            //     }
            // });
        });
        resp.clicked()
    }

    fn draw_color(&self, ui: &mut egui::Ui, color: Color32) {
        let resp = {
            let (rect, response) =
                ui.allocate_at_least(ui.available_size_before_wrap(), Sense::click());
            if ui.is_rect_visible(rect) {
                show_color_at(ui.painter(), color, rect);
            }
            response
        };
        let hex = if color.a() == u8::MAX {
            HexColor::Hex6(color)
        } else {
            HexColor::Hex8(color)
        };
        resp.on_hover_text(hex.to_string()).context_menu(|ui| {
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(hex.to_string());
                ui.close_menu();
            }
        });
    }

    fn size_text(&self, ui: &mut egui::Ui) -> f32 {
        ui.text_style_height(&egui::TextStyle::Body)
    }

    fn size_text_multiline(&self, ui: &mut egui::Ui, text: String) -> f32 {
        self.size_text(ui) * text.split('\n').count() as f32
    }

    fn size_internal(&self, ui: &mut egui::Ui) -> anyhow::Result<f32> {
        Ok(match &self.schema_column {
            SchemaColumnMeta::Scalar => {
                if self.sheet_column.kind() == ColumnKind::String {
                    let text = read_string(
                        self.row,
                        self.sheet_column.offset() as u32,
                        self.sheet_column.kind(),
                    )?;
                    self.size_text_multiline(ui, text)
                } else {
                    self.size_text(ui)
                }
            }
            SchemaColumnMeta::Icon => 32.0,
            SchemaColumnMeta::ModelId => self.size_text(ui),
            SchemaColumnMeta::Color => self.size_text(ui),
            SchemaColumnMeta::Link(sheets) => {
                let row_id: u32 = read_integer(
                    self.row,
                    self.sheet_column.offset() as u32,
                    self.sheet_column.kind(),
                )?;

                match self.table_context.resolve_link(sheets.clone(), row_id) {
                    Some(Some((_, table))) => {
                        if let Some(cell) =
                            table.display_field_cell(table.sheet().get_row(row_id).unwrap())
                        {
                            cell?.size_internal(ui)?
                        } else {
                            self.size_text(ui)
                        }
                    }
                    _ => self.size_text(ui),
                }
            }
            SchemaColumnMeta::ConditionalLink { column_idx, links } => {
                let (_, switch_column) = self.table_context.get_column(*column_idx)?;
                let switch_data: i32 = read_integer(
                    self.row,
                    switch_column.offset() as u32,
                    switch_column.kind(),
                )?;
                if let Some(sheets) = links.get(&switch_data) {
                    Cell {
                        row: self.row,
                        schema_column: SchemaColumnMeta::Link(sheets.clone()),
                        sheet_column: self.sheet_column,
                        table_context: self.table_context,
                    }
                    .size_internal(ui)?
                } else {
                    self.size_text(ui)
                }
            }
        })
    }

    pub fn size(&self, ui: &mut egui::Ui) -> f32 {
        self.size_internal(ui).unwrap_or_else(|err| {
            log::error!("Failed to size cell: {:?}", err);
            self.size_text(ui)
        })
    }

    pub fn size_pass(self, ui: &mut egui::Ui) -> anyhow::Result<f32> {
        let mut size_ui = ui.new_child(egui::UiBuilder::new().sizing_pass());
        self.draw(&mut size_ui)?;
        Ok(size_ui.min_rect().size().y)
    }

    pub fn show(self, ui: &mut egui::Ui) -> egui::InnerResponse<CellResponse> {
        match self.draw(ui) {
            Ok(resp) => {
                return InnerResponse::new(resp, ui.response());
            }
            Err(err) => {
                log::error!("Failed to draw cell: {:?}", err);
                let resp = ui
                    .colored_label(Color32::LIGHT_RED, "⚠")
                    .on_hover_text(err.to_string());
                return InnerResponse::new(CellResponse::None, resp);
            }
        }
    }
}

impl Widget for Cell<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        if let Err(err) = self.draw(ui) {
            log::error!("Failed to draw cell: {:?}", err);
            return ui
                .colored_label(Color32::LIGHT_RED, "⚠")
                .on_hover_text(err.to_string());
        }
        ui.response()
    }
}

fn read_string(row: ExcelRow<'_>, offset: u32, kind: ColumnKind) -> anyhow::Result<String> {
    Ok(match kind {
        ColumnKind::String => row.read_string(offset)?.format()?,
        ColumnKind::Bool => row.read_bool(offset)?.to_string(),
        ColumnKind::Int8 => row.read::<i8>(offset)?.to_string(),
        ColumnKind::UInt8 => row.read::<u8>(offset)?.to_string(),
        ColumnKind::Int16 => row.read::<i16>(offset)?.to_string(),
        ColumnKind::UInt16 => row.read::<u16>(offset)?.to_string(),
        ColumnKind::Int32 => row.read::<i32>(offset)?.to_string(),
        ColumnKind::UInt32 => row.read::<u32>(offset)?.to_string(),
        ColumnKind::Float32 => row.read::<f32>(offset)?.to_string(),
        ColumnKind::Int64 => row.read::<i64>(offset)?.to_string(),
        ColumnKind::UInt64 => row.read::<u64>(offset)?.to_string(),
        ColumnKind::PackedBool0
        | ColumnKind::PackedBool1
        | ColumnKind::PackedBool2
        | ColumnKind::PackedBool3
        | ColumnKind::PackedBool4
        | ColumnKind::PackedBool5
        | ColumnKind::PackedBool6
        | ColumnKind::PackedBool7 => row
            .read_packed_bool(
                offset,
                (u16::from(kind) - u16::from(ColumnKind::PackedBool0)) as u8,
            )?
            .to_string(),
    })
}

fn read_integer<T: num_traits::NumCast>(
    row: ExcelRow<'_>,
    offset: u32,
    kind: ColumnKind,
) -> anyhow::Result<T> {
    match kind {
        ColumnKind::Int8 => T::from(row.read::<i8>(offset)?),
        ColumnKind::UInt8 => T::from(row.read::<u8>(offset)?),
        ColumnKind::Int16 => T::from(row.read::<i16>(offset)?),
        ColumnKind::UInt16 => T::from(row.read::<u16>(offset)?),
        ColumnKind::Int32 => T::from(row.read::<i32>(offset)?),
        ColumnKind::UInt32 => T::from(row.read::<u32>(offset)?),
        ColumnKind::Int64 => T::from(row.read::<i64>(offset)?),
        ColumnKind::UInt64 => T::from(row.read::<u64>(offset)?),
        _ => bail!("Invalid column kind for integer: {:?}", kind),
    }
    .ok_or_else(|| {
        anyhow::anyhow!(
            "Failed to convert value to target type: {:?} -> {}",
            read_string(row, offset, kind).unwrap_or_default(),
            std::any::type_name::<T>()
        )
    })
}
