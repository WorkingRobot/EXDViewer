use std::ops::Deref;

use ironworks::file::exh::ColumnDefinition;
use itertools::Itertools;

use crate::excel::{base::BaseSheet, provider::ExcelHeader};

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
