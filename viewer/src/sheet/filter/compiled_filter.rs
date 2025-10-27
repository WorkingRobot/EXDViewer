use std::rc::Rc;

use crate::sheet::{
    filter::complex_filter::FilterValue, schema_column::SchemaColumn,
    sheet_column::SheetColumnDefinition,
};

#[derive(Debug, Clone)]
pub struct CompiledComplexFilter {
    pub filter: CompiledFilterPart,
    pub lookup: Vec<CompiledFilterKey>,
    pub has_fuzzy: bool,
}

impl PartialEq for CompiledComplexFilter {
    fn eq(&self, other: &Self) -> bool {
        self.filter == other.filter
    }
}

impl Eq for CompiledComplexFilter {}

impl std::hash::Hash for CompiledComplexFilter {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.filter.hash(state);
    }
}

#[derive(Debug, Clone)]
pub enum CompiledFilterKey {
    RowId,
    Column(Rc<Vec<(SchemaColumn, SheetColumnDefinition)>>, bool),
}

impl CompiledFilterKey {
    pub fn is_strict(&self) -> bool {
        match self {
            CompiledFilterKey::RowId => true,
            CompiledFilterKey::Column(_, is_strict) => *is_strict,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CompiledFilterPart {
    /// A simple key-value filter
    /// (u32 is the lookup index in `CompiledComplexFilter.lookup`)
    KeyEquals(u32, FilterValue),
    /// Combine two filters with logical AND
    And(Vec<CompiledFilterPart>),
    // Combine two filters with logical OR
    Or(Vec<CompiledFilterPart>),
    /// Negate a filter with logical NOT
    Not(Box<CompiledFilterPart>),
}
