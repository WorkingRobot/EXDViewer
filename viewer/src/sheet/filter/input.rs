use crate::sheet::{
    cell::{CellValue, MatchOptions},
    filter::{
        FilterCache,
        compiled_filter::{CompiledComplexFilter, CompiledFilterKey, CompiledFilterPart},
        complex_filter::ComplexFilter,
    },
};

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
        cache: &FilterCache,
    ) -> anyhow::Result<bool> {
        let Some(filter) = &self.0 else {
            return Ok(true);
        };

        let cell_grabber = |key: u32| {
            filter
                .lookup
                .get(key as usize)
                .map(|key| cell_grabber(key, self.1.use_display_field))
        };
        Self::match_part(&filter.filter, &cell_grabber, self.1, cache)
    }

    fn match_part<I: Iterator<Item = anyhow::Result<CellValue>>>(
        part: &CompiledFilterPart,
        cell_grabber: &impl Fn(u32) -> Option<I>,
        options: MatchOptions,
        cache: &FilterCache,
    ) -> anyhow::Result<bool> {
        Ok(match part {
            CompiledFilterPart::KeyEquals(key, value) => {
                let Some(cell_iter) = cell_grabber(*key) else {
                    unreachable!("Invalid lookup key: {key}");
                };
                for cell in cell_iter {
                    let cell = cell?;
                    if !cache.match_cell(&cell, value, options) {
                        return Ok(false);
                    }
                }
                true
            }
            CompiledFilterPart::And(parts) => {
                for part in parts {
                    if !Self::match_part(part, cell_grabber, options, cache)? {
                        return Ok(false);
                    }
                }
                true
            }
            CompiledFilterPart::Or(parts) => {
                for part in parts {
                    if Self::match_part(part, cell_grabber, options, cache)? {
                        return Ok(true);
                    }
                }
                false
            }
            CompiledFilterPart::Not(part) => !Self::match_part(part, cell_grabber, options, cache)?,
        })
    }
}
