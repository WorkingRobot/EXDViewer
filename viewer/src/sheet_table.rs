use egui::{Align, Direction, Id, Margin, Sense, UiBuilder, Widget};
use egui_table::TableDelegate;
use ironworks::file::exh::{ColumnDefinition, ColumnKind};
use itertools::Itertools;
use std::cell::RefCell;

use crate::{
    excel::{
        base::BaseSheet,
        provider::{ExcelHeader, ExcelRow, ExcelSheet},
    },
    schema::Schema,
};

pub struct SheetTable {
    sheet: BaseSheet,
    columns: Vec<(usize, ColumnDefinition)>,
    subrow_lookup: Option<Vec<u32>>,
    row_sizes: RefCell<Vec<f32>>,
    schema: Option<(Schema, Vec<String>)>,
}

impl SheetTable {
    pub fn new(sheet: BaseSheet, schema: Option<Schema>) -> Self {
        let schema = match schema {
            Some(schema) => {
                let paths = schema.get_paths(true, true);
                Some((schema, paths))
            }
            None => None,
        };

        let columns = sheet
            .columns()
            .iter()
            .enumerate()
            .sorted_by_key(|(_, c)| (c.offset(), c.kind() as u16))
            .map(|(i, c)| (i, c.clone()))
            .collect_vec();
        let row_sizes = RefCell::new(Vec::with_capacity(sheet.row_count() as usize));
        if sheet.has_subrows() {
            let mut subrow_lookup = Vec::with_capacity(sheet.row_count() as usize);
            let mut offset = 0u32;
            for i in 0..sheet.row_count() {
                subrow_lookup.push(offset);
                let row_id = sheet.get_row_id_at(i).unwrap();
                offset += sheet.get_row_subrow_count(row_id).unwrap_or_else(|e| {
                    log::error!("Failed to get subrow count for row {}: {:?}", i, e);
                    0
                }) as u32;
            }

            Self {
                sheet,
                columns,
                subrow_lookup: Some(subrow_lookup),
                row_sizes,
                schema,
            }
        } else {
            Self {
                sheet,
                columns,
                subrow_lookup: None,
                row_sizes,
                schema,
            }
        }
    }

    pub fn name(&self) -> &str {
        self.sheet.name()
    }

    pub fn set_schema(&mut self, schema: Schema) {
        let paths = schema.get_paths(true, true);
        self.schema = Some((schema, paths));
    }

    pub fn clear_schema(&mut self) {
        self.schema = None;
    }

    pub fn draw(&mut self, ui: &mut egui::Ui) {
        ui.push_id(Id::new(self.sheet.name()), |ui| {
            egui_table::Table::new()
                .num_rows(self.sheet.subrow_count().into())
                .columns(vec![
                    egui_table::Column::new(100.0)
                        .range(50.0..=10000.0)
                        .resizable(true);
                    self.columns.len() + 1
                ])
                .num_sticky_cols(1)
                .headers([egui_table::HeaderRow::new(
                    ui.text_style_height(&egui::TextStyle::Heading) + 4.0,
                )])
                .show(ui, self)
        });
    }

    fn get_row_id(&self, row_nr: u64) -> anyhow::Result<(u32, Option<u16>)> {
        if let Some(lookup) = &self.subrow_lookup {
            let row_idx = lookup
                .binary_search(&(row_nr as u32))
                .unwrap_or_else(|i| i - 1);
            let row_id = self.sheet.get_row_id_at(row_idx as u32)?;
            Ok((row_id, Some((row_nr - lookup[row_idx] as u64) as u16)))
        } else {
            let row_id = self.sheet.get_row_id_at(row_nr as u32)?;
            Ok((row_id, None))
        }
    }

    fn get_row_data(&self, row_id: u32, subrow_id: Option<u16>) -> anyhow::Result<ExcelRow<'_>> {
        if let Some(subrow_id) = subrow_id {
            self.sheet.get_subrow(row_id, subrow_id)
        } else {
            self.sheet.get_row(row_id)
        }
    }

    fn size_column(
        ui: &mut egui::Ui,
        row: ExcelRow<'_>,
        column: &ColumnDefinition,
    ) -> anyhow::Result<f32> {
        let mut size_ui = ui.new_child(UiBuilder::new().sizing_pass());
        let resp = egui::Frame::NONE
            .inner_margin(Margin::symmetric(4, 2))
            .show(&mut size_ui, |ui| Self::draw_column(ui, row, column));
        resp.inner?;
        Ok(size_ui.min_rect().size().y)
    }

    fn size_column_manual(
        ui: &mut egui::Ui,
        row: ExcelRow<'_>,
        column: &ColumnDefinition,
    ) -> anyhow::Result<f32> {
        Ok(match column.kind() {
            ColumnKind::String => {
                let string = row
                    .read_string(column.offset() as u32)?
                    .format()
                    .unwrap_or_else(|_| "Unknown".to_owned());
                ui.text_style_height(&egui::TextStyle::Body) * string.split('\n').count() as f32
            }
            _ => ui.text_style_height(&egui::TextStyle::Body),
        } + 4.0)
    }

    fn size_row_single_uncached(&self, ui: &mut egui::Ui, row_nr: u64) -> anyhow::Result<f32> {
        let (row_id, subrow_id) = self.get_row_id(row_nr)?;
        let row = self.get_row_data(row_id, subrow_id)?;
        let mut size = 0.0f32;
        for (_, column) in &self.columns {
            size = size.max(
                Self::size_column_manual(ui, row, column)
                    .or_else(|_| Self::size_column(ui, row, column))?,
            );
        }
        Ok(size)
    }

    fn size_row(&self, ctx: &egui::Context, row_nr: u64) -> anyhow::Result<f32> {
        let mut row_sizes = self.row_sizes.borrow_mut();
        if let Some(size) = row_sizes.get(row_nr as usize) {
            return Ok(*size);
        }
        let mut ui = egui::Ui::new(
            ctx.clone(),
            Id::new("size_row").with(row_nr),
            UiBuilder::new().sizing_pass(),
        );

        let (len, mut last_size) = (
            row_sizes.len() as u64,
            row_sizes.last().copied().unwrap_or_default(),
        );
        row_sizes.reserve((row_nr - len + 1) as usize);
        for i in len..row_nr {
            row_sizes.push(last_size);
            last_size += self.size_row_single_uncached(&mut ui, i)?;
        }
        row_sizes.push(last_size);
        Ok(row_sizes[row_nr as usize])
    }

    fn copyable_label(ui: &mut egui::Ui, text: impl ToString) {
        ui.with_layout(
            egui::Layout {
                main_dir: Direction::LeftToRight,
                main_wrap: false,
                main_align: Align::Min,
                main_justify: true,
                cross_align: Align::Center,
                cross_justify: true,
            },
            |ui| {
                let resp = egui::Label::new(text.to_string())
                    .sense(Sense::click())
                    .ui(ui);
                resp.context_menu(|ui| {
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(text.to_string());
                        ui.close_menu();
                    }
                });
            },
        );
    }

    fn draw_column(
        ui: &mut egui::Ui,
        row: ExcelRow<'_>,
        column: &ColumnDefinition,
    ) -> anyhow::Result<()> {
        match column.kind() {
            ColumnKind::String => {
                let string = row.read_string(column.offset() as u32)?;
                Self::copyable_label(ui, string.format().unwrap_or_else(|_| "Unknown".to_owned()));
            }
            ColumnKind::Bool => {
                let value = row.read_bool(column.offset() as u32);
                Self::copyable_label(ui, value);
            }
            ColumnKind::Int8 => {
                let value = row.read::<i8>(column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::UInt8 => {
                let value = row.read::<u8>(column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::Int16 => {
                let value = row.read::<i16>(column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::UInt16 => {
                let value = row.read::<u16>(column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::Int32 => {
                let value = row.read::<i32>(column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::UInt32 => {
                let value = row.read::<u32>(column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::Float32 => {
                let value = row.read::<f32>(column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::Int64 => {
                let value = row.read::<i64>(column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::UInt64 => {
                let value = row.read::<u64>(column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::PackedBool0
            | ColumnKind::PackedBool1
            | ColumnKind::PackedBool2
            | ColumnKind::PackedBool3
            | ColumnKind::PackedBool4
            | ColumnKind::PackedBool5
            | ColumnKind::PackedBool6
            | ColumnKind::PackedBool7 => {
                let value = row.read_packed_bool(
                    column.offset() as u32,
                    (u16::from(column.kind()) - u16::from(ColumnKind::PackedBool0)) as u8,
                );
                Self::copyable_label(ui, value);
            }
        };
        Ok(())
    }
}

impl TableDelegate for SheetTable {
    fn header_cell_ui(&mut self, ui: &mut egui::Ui, cell_inf: &egui_table::HeaderCellInfo) {
        let egui_table::HeaderCellInfo { col_range, .. } = cell_inf;

        let column_id = if col_range.start == 0 {
            None
        } else {
            Some(col_range.start - 1)
        };
        let column = column_id.and_then(|c| Some((c, self.columns.get(c)?)));

        let margin = 4;

        egui::Frame::NONE
            .inner_margin(Margin::symmetric(margin, 2))
            .show(ui, |ui| {
                if let Some((column_id, (column_sheet_id, column))) = column {
                    let column_path = self
                        .schema
                        .as_ref()
                        .and_then(|(_, paths)| paths.get(column_id));
                    let column_name = match column_path {
                        Some(path) => path,
                        None => &format!("Unknown{column_id}"),
                    };
                    ui.heading(column_name).on_hover_text(format!(
                        "Id: {}\nIndex: {}\nOffset: {}\nKind: {:?}",
                        column_sheet_id,
                        column_id,
                        column.offset(),
                        column.kind()
                    ));
                } else {
                    ui.heading("Row");
                }
            });
    }

    fn cell_ui(&mut self, ui: &mut egui::Ui, cell_info: &egui_table::CellInfo) {
        let egui_table::CellInfo { row_nr, col_nr, .. } = *cell_info;

        let column_id = if col_nr == 0 { None } else { Some(col_nr - 1) };

        let row_data = self
            .get_row_id(row_nr)
            .and_then(|(r, s)| Ok((r, s, self.get_row_data(r, s)?)));
        let (row_id, subrow_id, row_data) = match row_data {
            Ok(row_data) => row_data,
            Err(error) => {
                log::error!("Failed to get row data: {error:?}");
                return;
            }
        };

        if row_nr % 2 == 1 {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, ui.visuals().faint_bg_color);
        }

        egui::Frame::NONE
            .inner_margin(Margin::symmetric(4, 2))
            .show(ui, |ui| {
                if let Some(column_id) = column_id {
                    let (_, column) = &self.columns.get(column_id).unwrap();
                    match Self::draw_column(ui, row_data, column) {
                        Ok(()) => {}
                        Err(error) => {
                            log::error!(
                                "Failed to read column (kind {:?}, offset {}): {:?}",
                                column.kind(),
                                column.offset(),
                                error
                            );
                        }
                    }
                } else {
                    if let Some(subrow_id) = subrow_id {
                        ui.label(format!("{row_id}.{subrow_id}"))
                            .on_hover_text(format!("Row {row_id}, Subrow {subrow_id}"));
                    } else {
                        ui.label(row_id.to_string())
                            .on_hover_text(format!("Row {row_id}"));
                    }
                }
            });
    }

    fn row_top_offset(&self, ctx: &egui::Context, _table_id: Id, row_nr: u64) -> f32 {
        match self.size_row(ctx, row_nr) {
            Ok(size) => size,
            Err(error) => {
                log::error!("Failed to size row {}: {:?}", row_nr, error);
                0.0
            }
        }
    }
}
