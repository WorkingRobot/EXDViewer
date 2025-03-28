use anyhow::bail;
use egui::{Align, Direction, Id, Margin, Sense, UiBuilder, Widget};
use egui_table::TableDelegate;
use ironworks::file::exh::{ColumnDefinition, ColumnKind};
use itertools::Itertools;
use std::{cell::RefCell, collections::HashMap, ops::Deref};

use crate::{
    excel::{
        base::BaseSheet,
        provider::{ExcelHeader, ExcelRow, ExcelSheet},
    },
    schema::{Field, FieldType, Schema},
    utils::TrackedPromise,
};

pub struct SheetTable {
    sheet: BaseSheet,
    columns: Vec<SheetColumnDefinition>,
    subrow_lookup: Option<Vec<u32>>,
    row_sizes: RefCell<Vec<f32>>,
    schema_columns: Vec<SchemaColumn>,
    referenced_sheets: RefCell<HashMap<String, TrackedPromise<BaseSheet>>>,
}

impl SheetTable {
    pub fn new(sheet: BaseSheet, schema: Option<Schema>) -> Self {
        let schema_columns = schema
            .as_ref()
            .and_then(|schema| SchemaColumn::from_schema(schema, true, true).ok())
            .unwrap_or_else(|| SchemaColumn::from_blank(sheet.columns().len() as u32));

        let columns = SheetColumnDefinition::from_sheet(&sheet);
        let row_sizes = RefCell::new(Vec::with_capacity(sheet.row_count() as usize));

        let subrow_lookup = if sheet.has_subrows() {
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
            Some(subrow_lookup)
        } else {
            None
        };

        Self {
            sheet,
            columns,
            subrow_lookup,
            row_sizes,
            schema_columns,
            referenced_sheets: RefCell::new(HashMap::new()),
        }
    }

    pub fn name(&self) -> &str {
        self.sheet.name()
    }

    pub fn set_schema(&mut self, schema: Option<Schema>) -> anyhow::Result<()> {
        self.schema_columns = schema
            .map(|s| SchemaColumn::from_schema(&s, true, true))
            .map(|s| {
                s.map(|r| {
                    if r.len() == self.sheet.columns().len() {
                        Ok(r)
                    } else {
                        bail!(
                            "Schema column count does not match sheet column count: {} != {}",
                            r.len(),
                            self.sheet.columns().len()
                        )
                    }
                })?
            })
            .unwrap_or_else(|| Ok(SchemaColumn::from_blank(self.sheet.columns().len() as u32)))?;
        Ok(())
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
        &self,
        ui: &mut egui::Ui,
        row: ExcelRow<'_>,
        sheet_column: &SheetColumnDefinition,
        schema_column: &SchemaColumn,
    ) -> anyhow::Result<f32> {
        let mut size_ui = ui.new_child(UiBuilder::new().sizing_pass());
        let resp = egui::Frame::NONE
            .inner_margin(Margin::symmetric(4, 2))
            .show(&mut size_ui, |ui| {
                self.draw_column(ui, row, sheet_column, schema_column)
            });
        resp.inner?;
        Ok(size_ui.min_rect().size().y)
    }

    fn size_column_manual(
        &self,
        ui: &mut egui::Ui,
        row: ExcelRow<'_>,
        sheet_column: &SheetColumnDefinition,
        schema_column: &SchemaColumn,
    ) -> anyhow::Result<Option<f32>> {
        let col_height = match sheet_column.kind() {
            ColumnKind::String => {
                let string = row
                    .read_string(sheet_column.offset() as u32)?
                    .format()
                    .unwrap_or_else(|_| "Unknown".to_owned());
                ui.text_style_height(&egui::TextStyle::Body) * string.split('\n').count() as f32
            }
            _ => ui.text_style_height(&egui::TextStyle::Body),
        };
        Ok(Some(col_height + 4.0)) // symmetric inner y margin of 2.0
    }

    fn size_row_single_uncached(&self, ui: &mut egui::Ui, row_nr: u64) -> anyhow::Result<f32> {
        let (row_id, subrow_id) = self.get_row_id(row_nr)?;
        let row = self.get_row_data(row_id, subrow_id)?;
        let mut size = 0.0f32;
        for (sheet_column, schema_column) in self.columns.iter().zip_eq(&self.schema_columns) {
            let mut col_size = self.size_column_manual(ui, row, sheet_column, schema_column)?;
            if col_size.is_none() {
                col_size = Some(self.size_column(ui, row, sheet_column, schema_column)?);
            }
            size = size.max(col_size.unwrap());
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
        &self,
        ui: &mut egui::Ui,
        row: ExcelRow<'_>,
        sheet_column: &SheetColumnDefinition,
        schema_column: &SchemaColumn,
    ) -> anyhow::Result<()> {
        match sheet_column.kind() {
            ColumnKind::String => {
                let string = row.read_string(sheet_column.offset() as u32)?;
                Self::copyable_label(ui, string.format().unwrap_or_else(|_| "Unknown".to_owned()));
            }
            ColumnKind::Bool => {
                let value = row.read_bool(sheet_column.offset() as u32);
                Self::copyable_label(ui, value);
            }
            ColumnKind::Int8 => {
                let value = row.read::<i8>(sheet_column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::UInt8 => {
                let value = row.read::<u8>(sheet_column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::Int16 => {
                let value = row.read::<i16>(sheet_column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::UInt16 => {
                let value = row.read::<u16>(sheet_column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::Int32 => {
                let value = row.read::<i32>(sheet_column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::UInt32 => {
                let value = row.read::<u32>(sheet_column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::Float32 => {
                let value = row.read::<f32>(sheet_column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::Int64 => {
                let value = row.read::<i64>(sheet_column.offset() as u32)?;
                Self::copyable_label(ui, value);
            }
            ColumnKind::UInt64 => {
                let value = row.read::<u64>(sheet_column.offset() as u32)?;
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
                    sheet_column.offset() as u32,
                    (u16::from(sheet_column.kind()) - u16::from(ColumnKind::PackedBool0)) as u8,
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
        let column =
            column_id.and_then(|c| Some((c, (self.columns.get(c)?, self.schema_columns.get(c)?))));

        let margin = 4;

        egui::Frame::NONE
            .inner_margin(Margin::symmetric(margin, 2))
            .show(ui, |ui| {
                if let Some((column_id, (sheet_column, schema_column))) = column {
                    ui.heading(&schema_column.name).on_hover_text(format!(
                        "Id: {}\nIndex: {}\nOffset: {}\nKind: {:?}",
                        sheet_column.id,
                        column_id,
                        sheet_column.offset(),
                        sheet_column.kind()
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
                    let sheet_column = self.columns.get(column_id).unwrap();
                    let schema_column = self.schema_columns.get(column_id).unwrap();
                    match self.draw_column(ui, row_data, sheet_column, schema_column) {
                        Ok(()) => {}
                        Err(error) => {
                            log::error!(
                                "Failed to read column (kind {:?}, offset {}, name {}): {:?}",
                                sheet_column.kind(),
                                sheet_column.offset(),
                                schema_column.name,
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

struct SheetColumnDefinition {
    pub column: ColumnDefinition,
    pub id: u32,
}

impl SheetColumnDefinition {
    pub fn from_sheet(sheet: &BaseSheet) -> Vec<Self> {
        sheet
            .columns()
            .iter()
            .enumerate()
            .sorted_by_key(|(_, c)| (c.offset(), c.kind() as u16))
            .map(|(i, c)| Self {
                id: i as u32,
                column: c.clone(),
            })
            .collect_vec()
    }
}

impl Deref for SheetColumnDefinition {
    type Target = ColumnDefinition;

    fn deref(&self) -> &Self::Target {
        &self.column
    }
}

struct SchemaColumn {
    name: String,
    meta: SchemaColumnMeta,
}

impl SchemaColumn {
    fn get_columns_inner(
        ret: &mut Vec<Self>,
        scope: String,
        fields: &[Field],
        pending_names: bool,
        is_array: bool,
    ) -> anyhow::Result<()> {
        let column_offset = ret.len() as u32;

        let mut column_placeholder = u32::MAX;
        let mut column_lookups = vec![];

        for field in fields {
            let mut scope = scope.clone();
            if is_array {
                if let Some(name) = field.name(pending_names) {
                    scope.push('.');
                    scope.push_str(name);
                }
            } else {
                scope.push_str(field.name(pending_names).unwrap_or("Unk"));
            }

            if field.r#type == FieldType::Array {
                let subfields = field.fields.as_deref();
                let subfields = match subfields {
                    Some(subfields) => subfields,
                    None => &[Field::default()],
                };
                for i in 0..(field.count.unwrap_or(1)) {
                    Self::get_columns_inner(
                        ret,
                        scope.clone() + &format!("[{}]", i),
                        subfields,
                        pending_names,
                        true,
                    )?;
                }
            } else {
                let name = scope;

                let meta = match field.r#type {
                    FieldType::Scalar => SchemaColumnMeta::Scalar,
                    FieldType::Icon => SchemaColumnMeta::Icon,
                    FieldType::ModelId => SchemaColumnMeta::ModelId,
                    FieldType::Color => SchemaColumnMeta::Color,
                    FieldType::Link => {
                        if let Some(targets) = &field.targets {
                            SchemaColumnMeta::Link(targets.clone())
                        } else if let Some(condition) = &field.condition {
                            column_placeholder -= 1;
                            column_lookups.push(&condition.switch);
                            SchemaColumnMeta::ConditionalLink {
                                column_idx: column_placeholder,
                                links: condition.cases.clone(),
                            }
                        } else {
                            bail!("Link field missing targets or condition: {:?}", field);
                        }
                    }
                    FieldType::Array => unreachable!(),
                };

                ret.push(Self { name, meta });
            }
        }

        for i in 0..ret.len() - column_offset as usize {
            let column = &ret[column_offset as usize + i];
            if let SchemaColumnMeta::ConditionalLink { column_idx, .. } = column.meta {
                let column_lookup_idx = u32::MAX - column_idx - 1;
                let column_lookup_name = match column_lookups.get(column_lookup_idx as usize) {
                    Some(&name) => name,
                    None => {
                        bail!(
                            "Failed to find column lookup name for {}'s conditional link: {}",
                            column.name,
                            column_lookup_idx
                        );
                    }
                };

                let resolved_column_idx = column_offset
                    + match ret[column_offset as usize..]
                        .iter()
                        .enumerate()
                        .find_map(|(i, c)| {
                            if c.name[scope.len()..] == *column_lookup_name {
                                Some(i as u32)
                            } else {
                                None
                            }
                        }) {
                        Some(idx) => idx,
                        None => {
                            bail!(
                                "Failed to find column index for {}'s conditional link: {}",
                                column.name,
                                column_lookup_name
                            );
                        }
                    };

                if let SchemaColumnMeta::ConditionalLink { column_idx, .. } =
                    &mut ret[column_offset as usize + i].meta
                {
                    *column_idx = resolved_column_idx;
                } else {
                    unreachable!();
                }
            }
        }

        Ok(())
    }

    fn from_schema(
        schema: &Schema,
        pending_fields: bool,
        pending_names: bool,
    ) -> anyhow::Result<Vec<Self>> {
        let fields = pending_fields
            .then_some(())
            .and(schema.pending_fields.as_ref())
            .unwrap_or(&schema.fields);

        let mut ret = vec![];
        Self::get_columns_inner(&mut ret, "".to_string(), fields, pending_names, false)?;
        Ok(ret)
    }

    fn from_blank(column_count: u32) -> Vec<Self> {
        (0..column_count)
            .map(|i| Self {
                name: format!("Column{}", i),
                meta: SchemaColumnMeta::Scalar,
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
enum SchemaColumnMeta {
    Scalar,
    Icon,
    ModelId,
    Color,
    Link(Vec<String>),
    ConditionalLink {
        column_idx: u32,
        links: HashMap<i32, Vec<String>>,
    },
}
