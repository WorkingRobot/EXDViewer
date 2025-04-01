use std::{cell::RefCell, collections::HashMap, sync::Arc};

use anyhow::bail;

use crate::{
    excel::{
        base::BaseSheet,
        provider::{ExcelProvider, ExcelRow, ExcelSheet},
    },
    schema::{Schema, provider::SchemaProvider},
    utils::{CloneableResult, ConvertiblePromise, TrackedPromise},
};

use super::{
    cell::Cell, global_context::GlobalContext, schema_column::SchemaColumn,
    sheet_column::SheetColumnDefinition,
};

#[derive(Clone)]
pub struct TableContext(Arc<TableContextImpl>);

pub struct TableContextImpl {
    global: GlobalContext,

    sheet: BaseSheet,

    sheet_columns: Vec<SheetColumnDefinition>,
    schema_columns: RefCell<Vec<SchemaColumn>>,
    display_column_idx: std::cell::Cell<Option<u32>>,

    referenced_sheets: RefCell<
        HashMap<
            String,
            ConvertiblePromise<
                TrackedPromise<anyhow::Result<(BaseSheet, Option<Schema>)>>,
                CloneableResult<TableContext>,
            >,
        >,
    >,
}

impl TableContext {
    pub fn new(global: GlobalContext, sheet: BaseSheet, schema: Option<Schema>) -> Self {
        let sheet_columns = SheetColumnDefinition::from_sheet(&sheet);
        let schema_columns = SchemaColumn::from_blank(sheet_columns.len() as u32);

        let ret = Self(Arc::new(TableContextImpl {
            global,
            sheet,
            sheet_columns,
            schema_columns: RefCell::new(schema_columns),
            display_column_idx: std::cell::Cell::new(None),
            referenced_sheets: RefCell::new(HashMap::new()),
        }));
        if let Err(e) = ret.set_schema(schema) {
            log::error!("Failed to set schema: {:?}", e);
        }
        ret
    }

    pub fn sheet(&self) -> &BaseSheet {
        &self.0.sheet
    }

    pub fn global(&self) -> &GlobalContext {
        &self.0.global
    }

    pub fn get_column(
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

    pub fn set_schema(&self, schema: Option<Schema>) -> anyhow::Result<()> {
        let ret = schema
            .map(|s| SchemaColumn::from_schema(&s, true, true))
            .map(|s| {
                s.and_then(|r| {
                    if r.0.len() != self.0.sheet_columns.len() {
                        bail!(
                            "Schema column count does not match sheet column count: {} != {}",
                            r.0.len(),
                            self.0.sheet_columns.len()
                        )
                    }
                    Ok(r)
                })
            })
            .unwrap_or_else(|| {
                Ok((
                    SchemaColumn::from_blank(self.0.sheet_columns.len() as u32),
                    None,
                ))
            })?;
        self.0.schema_columns.replace(ret.0);
        self.0.display_column_idx.replace(ret.1);
        Ok(())
    }

    pub fn try_get_sheet(&self, name: String) -> Option<anyhow::Result<TableContext>> {
        let mut sheets = self.0.referenced_sheets.borrow_mut();
        let entry = sheets.entry(name).or_insert_with_key(|name| {
            let ctx = self.0.global.clone();
            let name = name.clone();
            ConvertiblePromise::new_promise(TrackedPromise::spawn_local(
                ctx.ctx().clone(),
                async move {
                    let sheet_future = ctx.backend().excel().get_sheet(&name, ctx.language());
                    let schema_future = ctx.backend().schema().get_schema_text(&name);
                    Ok(futures_util::try_join!(
                        async move { sheet_future.await },
                        async move {
                            Ok(schema_future
                                .await
                                .and_then(|s| Schema::from_str(&s))
                                .map(|a| a.ok())
                                .ok()
                                .flatten())
                        }
                    )?)
                },
            ))
        });

        entry
            .get(|result| {
                result
                    .map(|(sheet, schema)| TableContext::new(self.0.global.clone(), sheet, schema))
                    .map_err(|e| e.into())
            })
            .map(|result| result.as_ref().cloned().map_err(|e| e.clone().into()))
    }

    pub fn resolve_link(
        &self,
        sheets: Vec<String>,
        row_id: u32,
    ) -> Option<Option<(String, TableContext)>> {
        sheets
            .into_iter()
            .find_map(|s| match self.try_get_sheet(s.clone()) {
                None => Some(None),
                Some(Ok(table)) => {
                    if table.sheet().get_row(row_id).is_ok() {
                        Some(Some((s, table)))
                    } else {
                        None
                    }
                }
                Some(Err(err)) => {
                    log::error!("Failed to retrieve linked sheet: {:?}", err);
                    None
                }
            })
    }

    pub fn cell<'a>(&'a self, row: ExcelRow<'a>, column_idx: u32) -> anyhow::Result<Cell<'a>> {
        let (schema_column, sheet_column) = self.get_column(column_idx)?;
        Ok(Cell::new(row, schema_column.meta, sheet_column, self))
    }

    pub fn display_field_cell<'a>(&'a self, row: ExcelRow<'a>) -> Option<anyhow::Result<Cell<'a>>> {
        Some(self.cell(row, self.0.display_column_idx.get()?))
    }
}
