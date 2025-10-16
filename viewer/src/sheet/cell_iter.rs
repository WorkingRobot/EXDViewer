use std::borrow::Cow;

use crate::{
    excel::provider::ExcelRow,
    sheet::{
        TableContext,
        cell::{Cell, CellValue},
        schema_column::SchemaColumn,
        sheet_column::SheetColumnDefinition,
    },
};

pub struct CellIter<'a, I: Iterator<Item = &'a (SchemaColumn, &'a SheetColumnDefinition)>> {
    table: &'a TableContext,
    row: ExcelRow<'a>,
    columns: I,
    resolve_display_field: bool,
}

impl<'a, I: Iterator<Item = &'a (SchemaColumn, &'a SheetColumnDefinition)>> CellIter<'a, I> {
    pub fn new(
        table: &'a TableContext,
        row: ExcelRow<'a>,
        columns: I,
        resolve_display_field: bool,
    ) -> Self {
        Self {
            table,
            row,
            columns,
            resolve_display_field,
        }
    }
}

impl<'a, I: Iterator<Item = &'a (SchemaColumn, &'a SheetColumnDefinition)>> Iterator
    for CellIter<'a, I>
{
    type Item = anyhow::Result<CellValue>;

    fn next(&mut self) -> Option<Self::Item> {
        let (schema_column, sheet_column) = self.columns.next()?;
        let cell = Cell::new(
            self.row,
            Cow::Borrowed(schema_column),
            sheet_column,
            self.table,
        );
        Some(cell.read(self.resolve_display_field))
    }
}
