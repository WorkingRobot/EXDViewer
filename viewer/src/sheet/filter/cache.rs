use std::{
    cell::{LazyCell, RefCell},
    num::NonZeroU32,
};

use either::Either;
use lru::LruCache;

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
    },
    utils::FuzzyMatcher,
};

pub struct FilterCache {
    wildcard_cache: LazyCell<RefCell<LruCache<Wildcard, Vec<u32>>>>,
    matcher: FuzzyMatcher,
}

impl FilterCache {
    pub fn new() -> Self {
        Self {
            wildcard_cache: LazyCell::new(|| {
                RefCell::new(LruCache::new(std::num::NonZeroUsize::new(256).unwrap()))
            }),
            matcher: FuzzyMatcher::new(),
        }
    }

    pub fn compile(
        &self,
        input: &FilterInput,
        options: MatchOptions,
        ctx: &TableContext,
    ) -> anyhow::Result<CompiledFilterInput> {
        if input.is_empty() {
            return Ok(CompiledFilterInput::new(None, options));
        }
        let data = match input {
            FilterInput::Equals(s) => Self::compile_equals(s),
            FilterInput::Contains(s) => Self::compile_contains(s),
            FilterInput::Complex(f) => self.compile_complex(f, ctx)?,
        };

        Ok(CompiledFilterInput::new(Some(data), options))
    }

    pub fn invalidate_cache(&self) {
        self.wildcard_cache.borrow_mut().clear();
    }

    fn compile_equals(filter: impl Into<String>) -> CompiledComplexFilter {
        CompiledComplexFilter {
            filter: CompiledFilterPart::KeyEquals(
                0,
                FilterValue::Equals(Either::Left(filter.into())),
            ),
            lookup: vec![CompiledFilterKey::AllColumns],
            has_fuzzy: false,
        }
    }

    fn compile_contains(filter: impl Into<String>) -> CompiledComplexFilter {
        CompiledComplexFilter {
            filter: CompiledFilterPart::KeyEquals(0, FilterValue::Contains(filter.into())),
            lookup: vec![CompiledFilterKey::AllColumns],
            has_fuzzy: false,
        }
    }

    fn compile_complex(
        &self,
        filter: &ComplexFilter,
        ctx: &TableContext,
    ) -> anyhow::Result<CompiledComplexFilter> {
        let mut lookup = (Vec::new(), Vec::new());
        let compiled_filter = self.compile_complex_part(&filter, &mut lookup, ctx)?;
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
        ctx: &TableContext,
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
                    let compiled_key = self.compile_complex_key(key, ctx)?;
                    lookup.0.push(key.clone());
                    lookup.1.push(compiled_key);
                    assert_eq!(lookup.0.len(), lookup.1.len());
                    lookup.1.len() - 1
                };
                CompiledFilterPart::KeyEquals(compiled_key_idx as u32, value.clone())
            }
            ComplexFilter::And(parts) => CompiledFilterPart::And(
                parts
                    .into_iter()
                    .map(|p| self.compile_complex_part(p, lookup, ctx))
                    .collect::<anyhow::Result<Vec<_>>>()?,
            ),
            ComplexFilter::Or(parts) => CompiledFilterPart::Or(
                parts
                    .into_iter()
                    .map(|p| self.compile_complex_part(p, lookup, ctx))
                    .collect::<anyhow::Result<Vec<_>>>()?,
            ),
            ComplexFilter::Not(part) => {
                CompiledFilterPart::Not(Box::new(self.compile_complex_part(part, lookup, ctx)?))
            }
        })
    }

    fn compile_complex_key(
        &self,
        key: &FilterKey,
        ctx: &TableContext,
    ) -> anyhow::Result<CompiledFilterKey> {
        Ok(match key {
            FilterKey::RowId => CompiledFilterKey::RowId,
            FilterKey::Column(wildcard) if wildcard.is_catch_all() => CompiledFilterKey::AllColumns,
            FilterKey::Column(wildcard) => CompiledFilterKey::Column(
                self.wildcard_cache
                    .borrow_mut()
                    .get_or_insert_ref(&wildcard, || {
                        self.compile_complex_column_uncached(&wildcard, ctx)
                            .unwrap_or_default()
                    })
                    .clone(),
            ),
        })
    }

    fn compile_complex_column_uncached(
        &self,
        key: &Wildcard,
        ctx: &TableContext,
    ) -> anyhow::Result<Vec<u32>> {
        ctx.columns().map(|cols| {
            cols.into_iter()
                .enumerate()
                .filter_map(|(idx, (schema_col, _))| {
                    key.matches(schema_col.name()).then_some(idx as u32)
                })
                .collect()
        })
    }

    #[inline]
    pub fn match_cell(&self, cell: &CellValue, value: &FilterValue, options: MatchOptions) -> bool {
        self.match_cell_score(cell, value, options).is_some()
    }

    #[inline]
    pub fn match_cell_score(
        &self,
        cell: &CellValue,
        value: &FilterValue,
        options: MatchOptions,
    ) -> Option<NonZeroU32> {
        if let FilterValue::Fuzzy(v) = value {
            return self.matcher.score_one(&*v, &cell.coerce_string());
        }

        let matches = match value {
            FilterValue::Equals(Either::Left(v)) => {
                filter_string(cell, v, options.case_insensitive, |a, b| a == b)
            }
            FilterValue::Equals(Either::Right(v)) => {
                cell.coerce_integer().map_or(false, |i| i == *v)
            }
            FilterValue::StartsWith(v) => {
                filter_string(cell, v, options.case_insensitive, |a, b| a.starts_with(b))
            }
            FilterValue::EndsWith(v) => {
                filter_string(cell, v, options.case_insensitive, |a, b| a.ends_with(b))
            }
            FilterValue::Contains(v) => {
                filter_string(cell, v, options.case_insensitive, |a, b| a.contains(b))
            }
            FilterValue::Fuzzy(_) => unreachable!(),
            FilterValue::Wildcard(v) => v.matches(&cell.coerce_string()),
            FilterValue::Regex(v) => v.is_match(&cell.coerce_string()),
            FilterValue::Range(v) => cell.coerce_integer().map_or(false, |i| v.contains(i)),
        };

        matches.then_some(NonZeroU32::new(1).unwrap())
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
        f(&a.to_uppercase(), &b.to_uppercase())
    } else {
        f(&a, b)
    }
}
