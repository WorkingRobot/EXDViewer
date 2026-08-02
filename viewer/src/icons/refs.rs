use std::{cell::Cell, rc::Rc};

use anyhow::Result;
use compact_str::CompactString;
use futures_util::{StreamExt, stream};
use ironworks::excel::Language;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};
#[cfg(target_arch = "wasm32")]
use web_time::{Duration, Instant};

use crate::{
    backend::Backend,
    excel::{
        base::BaseSheet,
        provider::{ExcelHeader, ExcelProvider, ExcelSheet},
    },
    schema::{Schema, provider::SchemaProvider},
    sheet::{SchemaColumn, SchemaColumnMeta, SheetColumnDefinition, read_integer},
    utils::yield_to_ui,
};

const MAX_FRAME_TIME: Duration = Duration::from_millis(250);
/// Schema reads in flight at once. One request per sheet is the only shape the source offers, so
/// the wait is entirely round trips.
const SCHEMA_READS: usize = 8;

/// One sheet row naming an icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Use {
    pub sheet: u16,
    pub subrow: u16,
    pub row: u32,
}

/// Which rows name each icon, as a sorted id list indexed into a flat run of uses.
pub struct IconRefs {
    sheets: Vec<CompactString>,
    /// Distinct icons per sheet, positional with `sheets`.
    counts: Vec<u32>,
    ids: Vec<u32>,
    starts: Vec<u32>,
    uses: Vec<Use>,
}

impl IconRefs {
    pub fn sheets(&self) -> impl Iterator<Item = (u16, &str, u32)> {
        self.sheets
            .iter()
            .enumerate()
            .map(|(i, name)| (i as u16, name.as_str(), self.counts[i]))
    }

    pub fn sheet_name(&self, sheet: u16) -> &str {
        &self.sheets[sheet as usize]
    }

    pub fn uses(&self, icon_id: u32) -> &[Use] {
        match self.ids.binary_search(&icon_id) {
            Ok(i) => &self.uses[self.starts[i] as usize..self.starts[i + 1] as usize],
            Err(_) => &[],
        }
    }

    pub fn referenced(&self) -> usize {
        self.ids.len()
    }

    pub fn total(&self) -> usize {
        self.uses.len()
    }

    /// Every icon the given sheet names, ascending.
    pub fn icons_of(&self, sheet: u16) -> Vec<u32> {
        self.ids
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                self.uses[self.starts[*i] as usize..self.starts[i + 1] as usize]
                    .iter()
                    .any(|use_| use_.sheet == sheet)
            })
            .map(|(_, id)| *id)
            .collect()
    }

    pub fn is_referenced(&self, icon_id: u32) -> bool {
        self.ids.binary_search(&icon_id).is_ok()
    }

    fn build(sheets: Vec<CompactString>, mut flat: Vec<(u32, Use)>) -> Self {
        flat.sort_unstable_by_key(|(id, use_)| (*id, *use_));
        flat.dedup();

        let mut counts = vec![0u32; sheets.len()];
        let mut ids = Vec::new();
        let mut starts = Vec::new();
        let mut uses = Vec::with_capacity(flat.len());

        let mut seen_by = Vec::new();
        for (id, use_) in flat {
            if ids.last() != Some(&id) {
                for sheet in seen_by.drain(..) {
                    counts[sheet as usize] += 1;
                }
                ids.push(id);
                starts.push(uses.len() as u32);
            }
            if !seen_by.contains(&use_.sheet) {
                seen_by.push(use_.sheet);
            }
            uses.push(use_);
        }
        for sheet in seen_by {
            counts[sheet as usize] += 1;
        }
        starts.push(uses.len() as u32);

        Self {
            sheets,
            counts,
            ids,
            starts,
            uses,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    /// The walk counts schemas first and rows second, over different totals.
    pub reading_rows: bool,
}

/// Which columns of one sheet hold icon ids. Schema field `i` belongs to the `i`-th column in
/// offset order, not to `columns()[i]`; pairing them any other way reads plausible wrong values.
fn icon_columns(schema: &Schema, sheet: &BaseSheet) -> Vec<SheetColumnDefinition> {
    let Ok((columns, _)) = SchemaColumn::from_schema(schema) else {
        return Vec::new();
    };
    let ordered = SheetColumnDefinition::from_sheet(sheet);
    if columns.len() != ordered.len() {
        return Vec::new();
    }
    columns
        .iter()
        .zip(ordered)
        .filter(|(schema_column, _)| matches!(schema_column.meta(), SchemaColumnMeta::Icon))
        .map(|(_, column)| column)
        .collect()
}

fn names_icons(schema: &Schema) -> bool {
    let Ok((columns, _)) = SchemaColumn::from_schema(schema) else {
        return false;
    };
    columns
        .iter()
        .any(|column| matches!(column.meta(), SchemaColumnMeta::Icon))
}

async fn walk_sheet(
    backend: &Backend,
    name: &str,
    schema: &Schema,
    sheet_idx: u16,
    flat: &mut Vec<(u32, Use)>,
) -> Result<()> {
    // Icon columns read the same in every language, so any one the install ships will do. The
    // declared set is not that: a sheet names languages it has no pages for, and a localized one
    // usually omits `None` entirely, so asking for either can fail.
    let available = backend.excel().get_available_languages(name).await?;
    let language = [Language::None, Language::English]
        .into_iter()
        .find(|language| available.contains(language))
        .or_else(|| available.iter().copied().min_by_key(|l| u8::from(*l)))
        .unwrap_or(Language::None);

    let sheet = backend.excel().get_sheet(name, language).await?;
    let columns = icon_columns(schema, &sheet);
    if columns.is_empty() {
        return Ok(());
    }

    let mut at = Instant::now();
    let subrows = sheet.has_subrows();
    for row_id in sheet.get_row_ids() {
        let subrow_ids = if subrows {
            0..sheet.get_row_subrow_count(row_id).unwrap_or(0)
        } else {
            0..1
        };
        for subrow in subrow_ids {
            let Ok(row) = sheet.get_subrow(row_id, subrow) else {
                continue;
            };
            for column in &columns {
                let Ok(icon_id) =
                    read_integer::<i64>(row, u32::from(column.offset()), column.kind())
                else {
                    continue;
                };
                if icon_id > 0
                    && let Ok(icon_id) = u32::try_from(icon_id)
                {
                    flat.push((
                        icon_id,
                        Use {
                            sheet: sheet_idx,
                            subrow,
                            row: row_id,
                        },
                    ));
                }
            }
        }
        if at.elapsed() >= MAX_FRAME_TIME {
            yield_to_ui().await;
            at = Instant::now();
        }
    }
    Ok(())
}

/// Read every schema, then read the rows of only those sheets whose schema names an icon field.
pub async fn walk(backend: Backend, progress: Rc<Cell<Progress>>) -> Result<IconRefs> {
    let mut names: Vec<String> = backend
        .excel()
        .get_entries()
        .iter()
        .filter(|(_, id)| **id >= 0)
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    progress.set(Progress {
        total: names.len(),
        ..Progress::default()
    });

    let mut wanted: Vec<(String, Schema)> = Vec::new();
    let mut at = Instant::now();
    let mut done = 0;
    let schemas = backend.schema();
    let mut reads = stream::iter(
        names
            .iter()
            .map(|name| async move { (name, schemas.get_schema_text(name).await) }),
    )
    .buffer_unordered(SCHEMA_READS);
    while let Some((name, text)) = reads.next().await {
        done += 1;
        match text {
            Ok(text) => {
                if let Ok(Ok(schema)) = Schema::from_str(&text)
                    && names_icons(&schema)
                {
                    wanted.push((name.clone(), schema));
                }
            }
            Err(error) => log::warn!("icons/walk: {name}: {error}"),
        }
        if at.elapsed() >= MAX_FRAME_TIME {
            progress.set(Progress {
                done,
                total: names.len(),
                reading_rows: false,
            });
            yield_to_ui().await;
            at = Instant::now();
        }
    }
    // Sheets are numbered by their place here, so the walk has to settle in one order.
    wanted.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut sheets = Vec::with_capacity(wanted.len());
    let mut flat = Vec::new();
    for (done, (name, schema)) in wanted.iter().enumerate() {
        progress.set(Progress {
            done,
            total: wanted.len(),
            reading_rows: true,
        });
        let sheet_idx = sheets.len() as u16;
        sheets.push(CompactString::from(name.as_str()));
        if let Err(error) = walk_sheet(&backend, name, schema, sheet_idx, &mut flat).await {
            log::warn!("icons/walk: {name}: {error}");
        }
        yield_to_ui().await;
    }
    progress.set(Progress {
        done: wanted.len(),
        total: wanted.len(),
        reading_rows: true,
    });

    log::info!(
        "icons/walk: {} uses over {} sheets",
        flat.len(),
        sheets.len()
    );
    Ok(IconRefs::build(sheets, flat))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn use_(sheet: u16, row: u32) -> Use {
        Use {
            sheet,
            subrow: 0,
            row,
        }
    }

    #[test]
    fn csr_groups_uses_by_icon() {
        let refs = IconRefs::build(
            vec!["Item".into(), "Action".into(), "Empty".into()],
            vec![
                (30, use_(1, 7)),
                (10, use_(0, 1)),
                (30, use_(0, 4)),
                (10, use_(1, 2)),
                // A row naming the same icon in two columns is one use, not two.
                (10, use_(0, 1)),
                (
                    20,
                    Use {
                        sheet: 1,
                        subrow: 3,
                        row: 5,
                    },
                ),
            ],
        );

        assert_eq!(refs.referenced(), 3);
        assert_eq!(refs.total(), 5);
        assert_eq!(refs.uses(10), [use_(0, 1), use_(1, 2)]);
        assert_eq!(refs.uses(30), [use_(0, 4), use_(1, 7)]);
        assert!(refs.uses(11).is_empty());
        assert!(!refs.is_referenced(11));

        assert_eq!(refs.icons_of(0), [10, 30]);
        assert_eq!(refs.icons_of(1), [10, 20, 30]);
        assert!(refs.icons_of(2).is_empty());

        let counts: Vec<(&str, u32)> = refs.sheets().map(|(_, name, n)| (name, n)).collect();
        assert_eq!(counts, [("Item", 2), ("Action", 3), ("Empty", 0)]);
    }

    #[test]
    fn empty_walk_answers_nothing() {
        let refs = IconRefs::build(Vec::new(), Vec::new());
        assert_eq!(refs.referenced(), 0);
        assert!(refs.uses(1).is_empty());
    }
}
