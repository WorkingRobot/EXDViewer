use std::rc::Rc;

use compact_str::{CompactString, ToCompactString};
use itertools::Itertools;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};
#[cfg(target_arch = "wasm32")]
use web_time::{Duration, Instant};

use crate::{
    excel::provider::{ExcelHeader, ExcelRow, ExcelSheet},
    sheet::{
        TableContext,
        cell::CellValue,
        cell_iter::CellIter,
        schema_column::{SchemaColumn, SchemaColumnMeta},
        sheet_column::SheetColumnDefinition,
    },
    utils::yield_to_ui,
};

const MAX_FRAME_TIME: Duration = Duration::from_millis(250);
const MAX_LINK_PASSES: usize = 16;

type Columns = Rc<Vec<(SchemaColumn, SheetColumnDefinition)>>;

pub async fn export_csv(
    context: TableContext,
    resolve_display_field: bool,
) -> anyhow::Result<Vec<u8>> {
    let columns: Columns = Rc::new(
        (0..context.column_count() as u32)
            .map(|column_idx| {
                let (column, _) = context.get_column_by_index(column_idx)?;
                Ok((column.0, column.1.clone()))
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
    );

    if resolve_display_field {
        warm_links(&context, &columns).await;
    }

    let has_subrows = context.sheet().has_subrows();

    let mut writer = csv::Writer::from_writer(Vec::new());
    let mut header = Vec::with_capacity(columns.len() + 2);
    header.push("Row");
    if has_subrows {
        header.push("Subrow");
    }
    header.extend(columns.iter().map(|(column, _)| column.name()));
    writer.write_record(&header)?;

    let mut record: Vec<CompactString> = Vec::with_capacity(header.len());
    walk_rows(&context, |row_id, subrow_id, row| {
        record.clear();
        record.push(row_id.to_compact_string());
        if let Some(subrow_id) = subrow_id {
            record.push(subrow_id.to_compact_string());
        }
        for value in CellIter::new(&context, row, columns.clone(), resolve_display_field) {
            record.push(value?.coerce_string());
        }
        writer.write_record(record.iter().map(|field| field.as_bytes()))?;
        Ok(())
    })
    .await?;

    writer
        .into_inner()
        .map_err(|e| anyhow::anyhow!("Failed to flush CSV: {e}"))
}

async fn walk_rows(
    context: &TableContext,
    mut inspector: impl FnMut(u32, Option<u16>, ExcelRow<'_>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let sheet = context.sheet();
    let batch_count = 0x4000usize.div_euclid(context.column_count().max(1)).max(1);

    let iter: Box<dyn Iterator<Item = (u32, Option<u16>)> + '_> = if sheet.has_subrows() {
        Box::new(
            sheet
                .get_subrow_ids()
                .map(|(row_id, subrow_id)| (row_id, Some(subrow_id))),
        )
    } else {
        Box::new(sheet.get_row_ids().map(|row_id| (row_id, None)))
    };

    let mut last_now = Instant::now();
    for chunk in &iter.chunks(batch_count) {
        for (row_id, subrow_id) in chunk {
            inspector(
                row_id,
                subrow_id,
                sheet.get_subrow(row_id, subrow_id.unwrap_or(0))?,
            )?;
        }

        let now = Instant::now();
        if now.duration_since(last_now) >= MAX_FRAME_TIME {
            last_now = now;
            yield_to_ui().await;
        }
    }

    Ok(())
}

/// Reads every link cell until each referenced sheet has landed, so that the export pass
/// resolves display fields instead of falling back to raw row ids.
async fn warm_links(context: &TableContext, columns: &Columns) {
    let link_columns: Columns = Rc::new(
        columns
            .iter()
            .filter(|(column, _)| {
                matches!(
                    column.meta(),
                    SchemaColumnMeta::Link(_) | SchemaColumnMeta::ConditionalLink { .. }
                )
            })
            .cloned()
            .collect_vec(),
    );
    if link_columns.is_empty() {
        return;
    }

    for _ in 0..MAX_LINK_PASSES {
        let mut pending = false;
        // Every pass must re-read every link cell: a referenced sheet is only converted
        // from a promise into a table when a cell asks it to resolve.
        let result = walk_rows(context, |_, _, row| {
            for value in CellIter::new(context, row, link_columns.clone(), true) {
                pending |= is_pending(&value?);
            }
            Ok(())
        })
        .await;

        if let Err(e) = result {
            log::error!("Failed to resolve display fields: {e:?}");
            return;
        }
        if !pending {
            return;
        }

        while context.has_pending_references() {
            yield_to_ui().await;
        }
    }

    log::warn!(
        "Referenced sheets did not settle after {MAX_LINK_PASSES} passes; some links will export as row ids"
    );
}

fn is_pending(value: &CellValue) -> bool {
    match value {
        CellValue::InProgressLink(_) => true,
        CellValue::ValidLink {
            value: Some(value), ..
        } => is_pending(value),
        _ => false,
    }
}
