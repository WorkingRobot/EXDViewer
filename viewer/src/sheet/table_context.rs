use std::{borrow::Cow, cell::RefCell, collections::HashMap, rc::Rc};

use anyhow::bail;
use ironworks::sestring::SeString;
use itertools::Itertools;

use crate::{
    excel::{
        base::BaseSheet,
        provider::{ExcelHeader, ExcelProvider, ExcelRow, ExcelSheet},
    },
    schema::{Schema, provider::SchemaProvider},
    sheet::{
        cell::MatchOptions,
        cell_iter::CellIter,
        filter::{CompiledFilterInput, CompiledFilterKey, FilterCache, FilterInput},
    },
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

    filter_cache: FilterCache,
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
            filter_cache: FilterCache::new(),
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
        Ok(Cell::new(
            row,
            Cow::Owned(schema_column),
            sheet_column,
            self,
        ))
    }

    pub fn cell_by_index<'a>(
        &'a self,
        row: ExcelRow<'a>,
        column_idx: u32,
    ) -> anyhow::Result<Cell<'a>> {
        let ((schema_column, sheet_column), _offset_idx) = self.get_column_by_index(column_idx)?;
        Ok(Cell::new(
            row,
            Cow::Owned(schema_column),
            sheet_column,
            self,
        ))
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
        row_id: u32,
        subrow_id: Option<u16>,
        row: &ExcelRow<'_>,
        filter: &CompiledFilterInput,
    ) -> anyhow::Result<(bool, bool)> {
        if filter.is_empty() {
            return Ok((true, false));
        }

        let mut is_in_progress = false;

        let cell_grabber = |key: &CompiledFilterKey,
                            resolve_display_field: bool|
         -> Box<dyn Iterator<Item = anyhow::Result<CellValue>>> {
            match key {
                CompiledFilterKey::AllColumns => Box::new(CellIter::new(
                    self,
                    *row,
                    columns.iter(),
                    resolve_display_field,
                )),
                CompiledFilterKey::RowId => Box::new(std::iter::once(Ok(if subrow_id.is_some() {
                    CellValue::String(SeString::new(
                        format!("{}.{}", row_id, subrow_id.unwrap()).into_bytes(),
                    ))
                } else {
                    CellValue::Integer(row_id as i128)
                }))),
                CompiledFilterKey::Column(indices) => {
                    let col_iter = indices
                        .clone()
                        .into_iter()
                        .map(|idx| &columns[idx as usize]);
                    Box::new(CellIter::new(self, *row, col_iter, resolve_display_field))
                }
            }
        };

        let matches = filter.matches(cell_grabber, &self.0.filter_cache)?;
        Ok((matches, is_in_progress))
    }

    pub fn compile_filter(
        &self,
        input: &FilterInput,
        options: MatchOptions,
    ) -> anyhow::Result<CompiledFilterInput> {
        self.0.filter_cache.compile(input, options, self)
    }
}
