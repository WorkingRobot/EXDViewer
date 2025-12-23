use std::{
    cell::{LazyCell, RefCell},
    collections::HashMap,
    num::NonZeroU32,
    rc::Rc,
};

use either::Either;
use itertools::Itertools;

use crate::{
    sheet::{
        TableContext,
        cell::{CellValue, MatchOptions},
        filter::{
            FilterValue,
            compiled_filter::{CompiledComplexFilter, CompiledFilterKey, CompiledFilterPart},
            complex_filter::{ComplexFilter, FilterKey, Wildcard},
            input::{CompiledFilterInput, FilterInput},
        },
        schema_column::SchemaColumn,
        sheet_column::SheetColumnDefinition,
    },
    stopwatch::stopwatches::FILTER_MATCH_STOPWATCH,
    utils::FuzzyMatcher,
};

pub struct FilterCache {
    wildcard_cache:
        LazyCell<RefCell<HashMap<Wildcard, Rc<Vec<(SchemaColumn, SheetColumnDefinition)>>>>>,
    columns: RefCell<Rc<Vec<(SchemaColumn, SheetColumnDefinition)>>>,
    matcher: FuzzyMatcher,
}

impl FilterCache {
    pub fn new(schema_columns: &[SchemaColumn], sheet_columns: &[SheetColumnDefinition]) -> Self {
        Self {
            wildcard_cache: LazyCell::new(|| RefCell::new(HashMap::with_capacity(256))),
            columns: RefCell::new(Rc::new(
                schema_columns
                    .iter()
                    .zip(sheet_columns)
                    .map(|(a, b)| (a.clone(), b.clone()))
                    .collect_vec(),
            )),
            matcher: FuzzyMatcher::new(),
        }
    }

    pub fn compile(
        &self,
        input: &FilterInput,
        options: MatchOptions,
    ) -> anyhow::Result<CompiledFilterInput> {
        if input.is_empty() {
            return Ok(CompiledFilterInput::new(None, options));
        }
        let data = match input {
            FilterInput::Equals(s) => self.compile_equals(s),
            FilterInput::Contains(s) => self.compile_contains(s),
            FilterInput::Complex(f) => self.compile_complex(f)?,
        };

        Ok(CompiledFilterInput::new(Some(data), options))
    }

    pub fn invalidate_cache(&self, ctx: &TableContext) -> anyhow::Result<()> {
        self.wildcard_cache.borrow_mut().clear();
        *self.columns.borrow_mut() = Rc::new(ctx.columns()?);
        Ok(())
    }

    fn columns(&self) -> Rc<Vec<(SchemaColumn, SheetColumnDefinition)>> {
        self.columns.borrow().clone()
    }

    fn compile_equals(&self, filter: impl Into<String>) -> CompiledComplexFilter {
        CompiledComplexFilter {
            filter: CompiledFilterPart::KeyEquals(
                0,
                FilterValue::Equals(Either::Left(filter.into())),
            ),
            lookup: vec![CompiledFilterKey::Column(self.columns(), false)],
            has_fuzzy: false,
        }
    }

    fn compile_contains(&self, filter: impl Into<String>) -> CompiledComplexFilter {
        CompiledComplexFilter {
            filter: CompiledFilterPart::KeyEquals(0, FilterValue::Contains(filter.into())),
            lookup: vec![CompiledFilterKey::Column(self.columns(), false)],
            has_fuzzy: false,
        }
    }

    fn compile_complex(&self, filter: &ComplexFilter) -> anyhow::Result<CompiledComplexFilter> {
        let mut lookup = (Vec::new(), Vec::new());
        let compiled_filter = self.compile_complex_part(filter, &mut lookup)?;
        Ok(CompiledComplexFilter {
            filter: compiled_filter,
            lookup: lookup.1,
            has_fuzzy: filter.has_fuzzy(),
        })
    }

    fn compile_complex_part(
        &self,
        filter: &ComplexFilter,
        lookup: &mut (Vec<FilterKey>, Vec<CompiledFilterKey>),
    ) -> anyhow::Result<CompiledFilterPart> {
        Ok(match filter {
            ComplexFilter::KeyEquals(key, value) => {
                let compiled_key_idx = lookup
                    .0
                    .iter()
                    .enumerate()
                    .find_map(|(i, k)| (k == key).then_some(i));
                let compiled_key_idx = if let Some(idx) = compiled_key_idx {
                    idx
                } else {
                    let compiled_key = self.compile_complex_key(key);
                    lookup.0.push(key.clone());
                    lookup.1.push(compiled_key);
                    assert_eq!(lookup.0.len(), lookup.1.len());
                    lookup.1.len() - 1
                };
                CompiledFilterPart::KeyEquals(compiled_key_idx as u32, value.clone())
            }
            ComplexFilter::And(parts) => CompiledFilterPart::And(
                parts
                    .iter()
                    .map(|p| self.compile_complex_part(p, lookup))
                    .collect::<anyhow::Result<Vec<_>>>()?,
            ),
            ComplexFilter::Or(parts) => CompiledFilterPart::Or(
                parts
                    .iter()
                    .map(|p| self.compile_complex_part(p, lookup))
                    .collect::<anyhow::Result<Vec<_>>>()?,
            ),
            ComplexFilter::Not(part) => {
                CompiledFilterPart::Not(Box::new(self.compile_complex_part(part, lookup)?))
            }
        })
    }

    fn compile_complex_key(&self, key: &FilterKey) -> CompiledFilterKey {
        match key {
            FilterKey::RowId => CompiledFilterKey::RowId,
            FilterKey::Column(wildcard, is_strict) if wildcard.is_catch_all() => {
                CompiledFilterKey::Column(self.columns(), *is_strict)
            }
            FilterKey::Column(wildcard, is_strict) => CompiledFilterKey::Column(
                self.wildcard_cache
                    .borrow_mut()
                    .entry(wildcard.clone())
                    .or_insert_with_key(|wildcard| self.compile_complex_column_uncached(wildcard))
                    .clone(),
                *is_strict,
            ),
        }
    }

    fn compile_complex_column_uncached(
        &self,
        key: &Wildcard,
    ) -> Rc<Vec<(SchemaColumn, SheetColumnDefinition)>> {
        Rc::new(
            self.columns()
                .iter()
                .filter(|(schema_col, _)| key.matches(schema_col.name()))
                .cloned()
                .collect(),
        )
    }

    #[inline]
    pub fn match_cell(&self, cell: &CellValue, value: &FilterValue, options: MatchOptions) -> bool {
        let _sw = FILTER_MATCH_STOPWATCH.start();
        match value {
            FilterValue::Equals(Either::Left(v)) => {
                filter_string(cell, v, options.case_insensitive, |a, b| a == b)
            }
            FilterValue::Equals(Either::Right(v)) => cell.coerce_integer() == Some(*v),
            FilterValue::StartsWith(v) => {
                filter_string(cell, v, options.case_insensitive, |a, b| a.starts_with(b))
            }
            FilterValue::EndsWith(v) => {
                filter_string(cell, v, options.case_insensitive, |a, b| a.ends_with(b))
            }
            FilterValue::Contains(v) => {
                filter_string(cell, v, options.case_insensitive, |a, b| a.contains(b))
            }
            FilterValue::Fuzzy(v) => self.matcher.score_one(v, &cell.coerce_string()).is_some(),
            FilterValue::Wildcard(v) => v.matches(&cell.coerce_string()),
            FilterValue::Regex(v) => v.is_match(&cell.coerce_string()),
            FilterValue::Range(v) => cell.coerce_integer().is_some_and(|i| v.contains(i)),
        }
    }

    #[inline]
    pub fn match_cell_score(
        &self,
        cell: &CellValue,
        value: &FilterValue,
        options: MatchOptions,
    ) -> Option<NonZeroU32> {
        if let FilterValue::Fuzzy(v) = value {
            self.matcher.score_one(v, &cell.coerce_string())
        } else {
            self.match_cell(cell, value, options)
                .then_some(NonZeroU32::new(1).unwrap())
        }
    }
}

#[inline]
fn filter_string(
    cell: &CellValue,
    b: &str,
    case_insensitive: bool,
    f: impl FnOnce(&str, &str) -> bool,
) -> bool {
    let a = cell.coerce_string();
    if case_insensitive {
        f(&a.to_lowercase(), &b.to_lowercase())
    } else {
        f(&a, b)
    }
}
