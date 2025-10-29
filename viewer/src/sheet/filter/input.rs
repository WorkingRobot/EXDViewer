use std::{fmt::Display, num::NonZeroU32};

use anyhow::bail;
use serde::{Deserialize, Serialize};

use crate::{
    sheet::{
        cell::{CellValue, MatchOptions},
        filter::{
            FilterCache,
            compiled_filter::{CompiledComplexFilter, CompiledFilterKey, CompiledFilterPart},
            complex_filter::ComplexFilter,
        },
    },
    stopwatch::stopwatches::FILTER_KEY_STOPWATCH,
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FilterInputType {
    Equals,
    #[default]
    Contains,
    Complex,
}

impl Display for FilterInputType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FilterInputType::Equals => "Equals",
            FilterInputType::Contains => "Contains",
            FilterInputType::Complex => "Complex",
        })
    }
}

impl FilterInputType {
    pub fn emoji(self) -> &'static str {
        match self {
            FilterInputType::Equals => "=",
            FilterInputType::Contains => "≈",
            FilterInputType::Complex => "\u{ff0a}",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FilterInput {
    Equals(String),
    Contains(String),
    Complex(ComplexFilter),
}

impl FilterInput {
    pub fn is_empty(&self) -> bool {
        match self {
            FilterInput::Equals(s) | FilterInput::Contains(s) => s.is_empty(),
            FilterInput::Complex(f) => {
                matches!(f, ComplexFilter::And(v) | ComplexFilter::Or(v) if v.is_empty())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompiledFilterInput(Option<CompiledComplexFilter>, MatchOptions);

impl CompiledFilterInput {
    pub fn new(data: Option<CompiledComplexFilter>, options: MatchOptions) -> Self {
        Self(data, options)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    pub fn input(&self) -> &Option<CompiledComplexFilter> {
        &self.0
    }

    pub fn options(&self) -> &MatchOptions {
        &self.1
    }

    pub fn matches<I: Iterator<Item = anyhow::Result<CellValue>>>(
        &self,
        cell_grabber: impl Fn(&CompiledFilterKey, bool) -> I,
        is_in_progress: &mut bool,
        cache: &FilterCache,
    ) -> anyhow::Result<bool> {
        let Some(filter) = &self.0 else {
            bail!("No filter to match against");
        };

        let cell_grabber = |key: u32| {
            filter
                .lookup
                .get(key as usize)
                .map(|key| (cell_grabber(key, self.1.use_display_field), key.is_strict()))
        };
        Self::match_part(&filter.filter, &cell_grabber, self.1, is_in_progress, cache)
    }

    fn match_part<I: Iterator<Item = anyhow::Result<CellValue>>>(
        part: &CompiledFilterPart,
        cell_grabber: &impl Fn(u32) -> Option<(I, bool)>,
        options: MatchOptions,
        is_in_progress: &mut bool,
        cache: &FilterCache,
    ) -> anyhow::Result<bool> {
        Ok(match part {
            CompiledFilterPart::KeyEquals(key, value) => {
                let Some((cell_iter, is_strict)) = cell_grabber(*key) else {
                    unreachable!("Invalid lookup key: {key}");
                };
                for cell in cell_iter {
                    let _sw = FILTER_KEY_STOPWATCH.start();
                    let cell = cell?;
                    if cell.is_in_progress() {
                        *is_in_progress = true;
                    }
                    let is_match = cache.match_cell(&cell, value, options);
                    if is_match && !is_strict {
                        return Ok(true);
                    }
                    if !is_match && is_strict {
                        return Ok(false);
                    }
                }
                is_strict
            }
            CompiledFilterPart::And(parts) => {
                for part in parts {
                    if !Self::match_part(part, cell_grabber, options, is_in_progress, cache)? {
                        return Ok(false);
                    }
                }
                true
            }
            CompiledFilterPart::Or(parts) => {
                for part in parts {
                    if Self::match_part(part, cell_grabber, options, is_in_progress, cache)? {
                        return Ok(true);
                    }
                }
                false
            }
            CompiledFilterPart::Not(part) => {
                !Self::match_part(part, cell_grabber, options, is_in_progress, cache)?
            }
        })
    }

    pub fn score<I: Iterator<Item = anyhow::Result<CellValue>>>(
        &self,
        cell_grabber: impl Fn(&CompiledFilterKey, bool) -> I,
        is_in_progress: &mut bool,
        cache: &FilterCache,
    ) -> anyhow::Result<Option<NonZeroU32>> {
        let Some(filter) = &self.0 else {
            bail!("No filter to match against");
        };

        let cell_grabber = |key: u32| {
            filter
                .lookup
                .get(key as usize)
                .map(|key| (cell_grabber(key, self.1.use_display_field), key.is_strict()))
        };
        Self::score_part(&filter.filter, &cell_grabber, self.1, is_in_progress, cache)
    }

    fn score_part<I: Iterator<Item = anyhow::Result<CellValue>>>(
        part: &CompiledFilterPart,
        cell_grabber: &impl Fn(u32) -> Option<(I, bool)>,
        options: MatchOptions,
        is_in_progress: &mut bool,
        cache: &FilterCache,
    ) -> anyhow::Result<Option<NonZeroU32>> {
        Ok(match part {
            CompiledFilterPart::KeyEquals(key, value) => {
                let Some((cell_iter, is_strict)) = cell_grabber(*key) else {
                    unreachable!("Invalid lookup key: {key}");
                };
                let mut score = 0;
                for cell in cell_iter {
                    let cell = cell?;
                    if cell.is_in_progress() {
                        *is_in_progress = true;
                    }
                    if let Some(s) = cache.match_cell_score(&cell, value, options) {
                        score += s.get();
                    } else if is_strict {
                        return Ok(None);
                    }
                }
                NonZeroU32::new(score)
            }
            CompiledFilterPart::And(parts) => {
                let mut score = 0;
                for part in parts {
                    if let Some(s) =
                        Self::score_part(part, cell_grabber, options, is_in_progress, cache)?
                    {
                        score += s.get();
                    } else {
                        return Ok(None);
                    }
                }
                NonZeroU32::new(score)
            }
            CompiledFilterPart::Or(parts) => {
                let mut score = 0;
                for part in parts {
                    if let Some(s) =
                        Self::score_part(part, cell_grabber, options, is_in_progress, cache)?
                    {
                        score += s.get();
                    }
                }
                NonZeroU32::new(score)
            }
            CompiledFilterPart::Not(part) => NonZeroU32::new(
                (!Self::match_part(part, cell_grabber, options, is_in_progress, cache)?) as u32,
            ),
        })
    }
}
