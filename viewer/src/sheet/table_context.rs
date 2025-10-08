use std::{cell::RefCell, collections::HashMap, rc::Rc};

use anyhow::bail;
use itertools::Itertools;

use crate::{
    excel::{
        base::BaseSheet,
        provider::{ExcelHeader, ExcelProvider, ExcelRow, ExcelSheet},
    },
    schema::{Schema, provider::SchemaProvider},
    utils::{CloneableResult, ConvertiblePromise, TrackedPromise},
};

use super::{
    cell::{Cell, CellValue},
    global_context::GlobalContext,
    schema_column::SchemaColumn,
    sheet_column::SheetColumnDefinition,
};

type SheetPromise = TrackedPromise<anyhow::Result<(BaseSheet, Option<Schema>)>>;
type ConvertibleSheetPromise = ConvertiblePromise<SheetPromise, CloneableResult<TableContext>>;

#[derive(Clone)]
pub struct TableContext(Rc<TableContextImpl>);

pub struct TableContextImpl {
    global: GlobalContext,

    sheet: BaseSheet,

    // ID -> Index when ordered by offset (offset index)
    column_ordering: Vec<u32>,
    sheet_columns: Vec<SheetColumnDefinition>,
    schema_columns: RefCell<Vec<SchemaColumn>>,
    // Offset index of the displayField column
    display_column_idx: std::cell::Cell<Option<u32>>,

    referenced_sheets: RefCell<HashMap<String, ConvertibleSheetPromise>>,
}

impl TableContext {
    pub fn new(global: GlobalContext, sheet: BaseSheet, schema: Option<&Schema>) -> Self {
        let sheet_columns = SheetColumnDefinition::from_sheet(&sheet);
        let (schema_columns, display_column_idx) = schema
            .and_then(|s| SchemaColumn::from_schema(s).ok())
            .unwrap_or_else(|| (SchemaColumn::from_blank(sheet_columns.len()), None));
        let column_ordering = sheet_columns
            .iter()
            .enumerate()
            .sorted_by_key(|&(_i, p)| p.id)
            .map(|(i, _p)| i as u32)
            .collect_vec();

        Self(Rc::new(TableContextImpl {
            global,
            sheet,
            column_ordering,
            sheet_columns,
            schema_columns: RefCell::new(schema_columns),
            display_column_idx: std::cell::Cell::new(display_column_idx),
            referenced_sheets: RefCell::new(HashMap::new()),
        }))
    }

    pub fn sheet(&self) -> &BaseSheet {
        &self.0.sheet
    }

    pub fn global(&self) -> &GlobalContext {
        &self.0.global
    }

    pub fn get_column_by_offset(
        &self,
        column_idx: u32,
    ) -> anyhow::Result<(SchemaColumn, &SheetColumnDefinition)> {
        Ok((
            self.0
                .schema_columns
                .borrow()
                .get(column_idx as usize)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Column index out of bounds: {} >= {}",
                        column_idx,
                        self.0.sheet_columns.len()
                    )
                })?
                .clone(),
            self.0
                .sheet_columns
                .get(column_idx as usize)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Column index out of bounds: {} >= {}",
                        column_idx,
                        self.0.sheet_columns.len()
                    )
                })?,
        ))
    }

    pub fn convert_column_index_to_offset_index(&self, column_idx: u32) -> anyhow::Result<u32> {
        self.0
            .column_ordering
            .get(column_idx as usize)
            .copied()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Column index out of bounds: {} >= {}",
                    column_idx,
                    self.0.column_ordering.len()
                )
            })
    }

    pub fn get_column_by_index(
        &self,
        column_idx: u32,
    ) -> anyhow::Result<((SchemaColumn, &SheetColumnDefinition), u32)> {
        let offset_idx = self.convert_column_index_to_offset_index(column_idx)?;
        Ok((self.get_column_by_offset(offset_idx)?, offset_idx))
    }

    pub fn set_schema(&self, schema: Option<&Schema>) -> anyhow::Result<()> {
        let schema = schema.map_or_else(
            || {
                SchemaColumn::from_schema(&Schema::from_blank(
                    self.0.sheet.name(),
                    self.0.sheet_columns.len(),
                ))
            },
            SchemaColumn::from_schema,
        );
        let (columns, display_column_idx) = schema.and_then(|r| {
            if r.0.len() != self.0.sheet_columns.len() {
                bail!(
                    "Schema column count does not match sheet column count: {} != {}",
                    r.0.len(),
                    self.0.sheet_columns.len()
                )
            }
            Ok(r)
        })?;
        self.0.schema_columns.replace(columns);
        self.0.display_column_idx.replace(display_column_idx);
        Ok(())
    }

    pub fn try_get_sheet(&self, name: String) -> Option<anyhow::Result<TableContext>> {
        let mut sheets = self.0.referenced_sheets.borrow_mut();
        let entry = sheets.entry(name).or_insert_with_key(|name| {
            let ctx = self.0.global.clone();
            let name = name.clone();
            ConvertiblePromise::new_promise(TrackedPromise::spawn_local(async move {
                let sheet_future = ctx.backend().excel().get_sheet(&name, ctx.language());
                let schema_future = ctx.backend().schema().get_schema_text(&name);
                Ok(futures_util::try_join!(sheet_future, async move {
                    Ok(schema_future
                        .await
                        .and_then(|s| Schema::from_str(&s))
                        .map(|a| a.ok())
                        .ok()
                        .flatten())
                })?)
            }))
        });

        entry
            .get_mut(|result| {
                result
                    .map(|(sheet, schema)| {
                        TableContext::new(self.0.global.clone(), sheet, schema.as_ref())
                    })
                    .map_err(|e| e.into())
            })
            .map(|result| result.as_ref().cloned().map_err(|e| e.clone().into()))
    }

    pub fn resolve_link(
        &self,
        sheets: &[String],
        row_id: u32,
    ) -> Option<Option<(String, TableContext)>> {
        sheets
            .iter()
            .map(|s| (s, self.try_get_sheet(s.clone())))
            .collect_vec()
            .into_iter()
            .find_map(|(s, result)| match result {
                None => Some(None),
                Some(Ok(table)) => {
                    if table.sheet().get_row(row_id).is_ok() {
                        Some(Some((s.clone(), table)))
                    } else {
                        None
                    }
                }
                Some(Err(err)) => {
                    log::error!("Failed to retrieve linked sheet: {err:?}");
                    None
                }
            })
    }

    pub fn columns(&self) -> anyhow::Result<Vec<(SchemaColumn, &SheetColumnDefinition)>> {
        (0..self.0.sheet_columns.len() as u32)
            .map(|i| self.get_column_by_offset(i))
            .collect::<anyhow::Result<Vec<_>>>()
    }

    pub fn cell_by_offset<'a>(
        &'a self,
        row: ExcelRow<'a>,
        column_idx: u32,
    ) -> anyhow::Result<Cell<'a>> {
        let (schema_column, sheet_column) = self.get_column_by_offset(column_idx)?;
        Ok(Cell::new(row, schema_column, sheet_column, self))
    }

    pub fn cell_by_index<'a>(
        &'a self,
        row: ExcelRow<'a>,
        column_idx: u32,
    ) -> anyhow::Result<Cell<'a>> {
        let ((schema_column, sheet_column), _offset_idx) = self.get_column_by_index(column_idx)?;
        Ok(Cell::new(row, schema_column, sheet_column, self))
    }

    pub fn display_column_idx(&self) -> Option<u32> {
        self.0.display_column_idx.get()
    }

    pub fn display_field_cell<'a>(&'a self, row: ExcelRow<'a>) -> Option<anyhow::Result<Cell<'a>>> {
        Some(self.cell_by_offset(row, self.0.display_column_idx.get()?))
    }

    pub fn size_row(
        &self,
        row: ExcelRow<'_>,
        ui: &mut egui::Ui,
        row_location: (u32, Option<u16>),
    ) -> f32 {
        let size = (0..self.sheet().columns().len())
            .filter_map(|column_idx| self.cell_by_offset(row, column_idx as u32).ok())
            .map(|c| c.size(ui, row_location))
            .reduce(f32::max);
        size.unwrap_or_default() + 4.0
    }

    pub fn filter_row(
        &self,
        columns: &[(SchemaColumn, &SheetColumnDefinition)],
        row: &ExcelRow<'_>,
        filter: &str,
        resolve_display_field: bool,
    ) -> anyhow::Result<(bool, bool)> {
        if filter.is_empty() {
            return Ok((true, false));
        }

        let mut is_in_progress = false;
        for column in columns {
            let (schema_column, sheet_column) = column;
            let cell = Cell::new(*row, schema_column.clone(), sheet_column, self);
            let value = cell.read(resolve_display_field)?;
            let (matches, in_progress) = Self::filter_value(&value, filter);
            if in_progress {
                is_in_progress = true;
            }
            if matches {
                return Ok((true, is_in_progress));
            }
        }
        Ok((false, is_in_progress))
    }

    fn filter_value(value: &CellValue, filter: &str) -> (bool, bool) {
        let resp = match value {
            CellValue::String(s) => s
                .macro_string()
                .unwrap_or_default()
                .to_lowercase()
                .contains(&filter.to_lowercase()),
            CellValue::Integer(i) => i.to_string().contains(&filter.to_lowercase()),
            CellValue::Float(f) => f.to_string().contains(&filter.to_lowercase()),
            CellValue::Boolean(b) => b.to_string().contains(&filter.to_lowercase()),
            CellValue::Icon(id) => id.to_string().contains(&filter.to_lowercase()),
            CellValue::ModelId(id) => {
                let label = id.map_either(
                    |model_id| {
                        let model = (model_id & 0xFFFF) as u16;
                        let variant = ((model_id >> 16) & 0xFF) as u8;
                        let stain = ((model_id >> 24) & 0xFF) as u8;
                        format!("{model}, {variant}, {stain}")
                    },
                    |weapon_id| {
                        let skeleton = (weapon_id & 0xFFFF) as u16;
                        let model = ((weapon_id >> 16) & 0xFFFF) as u16;
                        let variant = ((weapon_id >> 32) & 0xFFFF) as u16;
                        let stain = ((weapon_id >> 48) & 0xFFFF) as u16;
                        format!("{skeleton}, {model}, {variant}, {stain}")
                    },
                );
                label.contains(&filter.to_lowercase())
            }
            CellValue::Color(color) => color.to_hex().contains(&filter.to_lowercase()),
            CellValue::InvalidLink(id) => id.to_string().contains(&filter.to_lowercase()),
            CellValue::InProgressLink(id) => {
                return (id.to_string().contains(&filter.to_lowercase()), true);
            }
            CellValue::ValidLink { row_id, value, .. } => {
                let ret = row_id.to_string().contains(&filter.to_lowercase());
                if !ret {
                    return value
                        .as_ref()
                        .map(|v| Self::filter_value(v.as_ref(), filter))
                        .unwrap_or_default();
                }
                ret
            }
        };
        (resp, false)
    }
}
