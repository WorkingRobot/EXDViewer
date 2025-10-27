use std::rc::Rc;

use compact_str::format_compact;

use crate::{
    excel::provider::ExcelRow,
    sheet::{
        TableContext, cell::CellValue, cell_iter::CellIter, schema_column::SchemaColumn,
        sheet_column::SheetColumnDefinition,
    },
    stopwatch::stopwatches::FILTER_CELL_ITER_STOPWATCH,
};

pub enum KeyCellIter<'a> {
    Columns(CellIter<'a>),
    RowId(u32),
    SubrowId(u32, u16),
    Done,
}

impl<'a> KeyCellIter<'a> {
    pub fn column(
        table: &'a TableContext,
        row: ExcelRow<'a>,
        columns: Rc<Vec<(SchemaColumn, SheetColumnDefinition)>>,
        resolve_display_field: bool,
    ) -> Self {
        Self::Columns(CellIter::new(table, row, columns, resolve_display_field))
    }

    pub fn row_id(row_id: u32, subrow_id: Option<u16>) -> Self {
        if let Some(subrow_id) = subrow_id {
            Self::SubrowId(row_id, subrow_id)
        } else {
            Self::RowId(row_id)
        }
    }
}

impl<'a> Iterator for KeyCellIter<'a> {
    type Item = anyhow::Result<CellValue>;

    fn next(&mut self) -> Option<Self::Item> {
        let _sw = FILTER_CELL_ITER_STOPWATCH.start();
        match self {
            KeyCellIter::Columns(iter) => iter.next(),
            KeyCellIter::RowId(row_id) => {
                let value = CellValue::Integer(*row_id as i128);
                *self = KeyCellIter::Done;
                Some(Ok(value))
            }
            KeyCellIter::SubrowId(row_id, subrow_id) => {
                let value = CellValue::String(format_compact!("{}.{}", row_id, subrow_id).into());
                *self = KeyCellIter::Done;
                Some(Ok(value))
            }
            KeyCellIter::Done => None,
        }
    }
}
