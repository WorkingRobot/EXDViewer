use std::{borrow::Cow, rc::Rc};

use crate::{
    excel::provider::ExcelRow,
    sheet::{
        TableContext,
        cell::{Cell, CellValue},
        schema_column::SchemaColumn,
        sheet_column::SheetColumnDefinition,
    },
    stopwatch::stopwatches::{FILTER_CELL_CREATE_STOPWATCH, FILTER_CELL_READ_STOPWATCH},
};

pub struct CellIter<'a> {
    table: &'a TableContext,
    row: ExcelRow<'a>,
    columns: Rc<Vec<(SchemaColumn, SheetColumnDefinition)>>,
    idx: usize,
    resolve_display_field: bool,
}

impl<'a> CellIter<'a> {
    pub fn new(
        table: &'a TableContext,
        row: ExcelRow<'a>,
        columns: Rc<Vec<(SchemaColumn, SheetColumnDefinition)>>,
        resolve_display_field: bool,
    ) -> Self {
        Self {
            table,
            row,
            columns,
            idx: 0,
            resolve_display_field,
        }
    }
}

impl<'a> Iterator for CellIter<'a> {
    type Item = anyhow::Result<CellValue>;

    fn next(&mut self) -> Option<Self::Item> {
        let (schema_column, sheet_column) = self.columns.get(self.idx)?;
        let _sw = FILTER_CELL_CREATE_STOPWATCH.start();
        self.idx += 1;
        let cell = Cell::new(
            self.row,
            Cow::Borrowed(schema_column),
            sheet_column,
            self.table,
        );
        drop(_sw);
        let _sw = FILTER_CELL_READ_STOPWATCH.start();
        Some(cell.read(self.resolve_display_field))
    }
}
