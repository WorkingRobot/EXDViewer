use egui::{Id, InnerResponse, Margin, Modal, Spinner, UiBuilder, Vec2};
use egui_table::TableDelegate;
use itertools::Itertools;
use std::cell::RefCell;

use crate::{
    excel::provider::{ExcelHeader, ExcelProvider, ExcelRow, ExcelSheet},
    settings::{SORTED_BY_OFFSET, TEMP_HIGHLIGHTED_ROW_NR},
    utils::{ManagedIcon, TrackedPromise},
};

use super::{cell::CellResponse, table_context::TableContext};

pub struct SheetTable {
    context: TableContext,
    subrow_lookup: Option<Vec<u32>>,
    row_sizes: RefCell<Vec<f32>>,
    modal_image: Option<u32>,
}

impl SheetTable {
    pub fn new(context: TableContext) -> Self {
        let sheet = context.sheet();
        let row_sizes = RefCell::new(Vec::with_capacity(sheet.row_count() as usize));

        let subrow_lookup = if sheet.has_subrows() {
            let mut subrow_lookup = Vec::with_capacity(sheet.row_count() as usize);
            let mut offset = 0u32;
            for i in 0..sheet.row_count() {
                subrow_lookup.push(offset);
                let row_id = sheet.get_row_id_at(i).unwrap();
                offset += sheet.get_row_subrow_count(row_id).unwrap_or_else(|e| {
                    log::error!("Failed to get subrow count for row {}: {:?}", i, e);
                    0
                }) as u32;
            }
            Some(subrow_lookup)
        } else {
            None
        };

        Self {
            context,
            subrow_lookup,
            row_sizes,
            modal_image: None,
        }
    }

    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        mutator: impl FnOnce(egui_table::Table) -> egui_table::Table,
    ) {
        let id = Id::new(self.context.sheet().name());
        ui.push_id(id, |ui| {
            let table = egui_table::Table::new()
                .num_rows(self.context.sheet().subrow_count().into())
                .columns(vec![
                    egui_table::Column::new(100.0)
                        .range(50.0..=10000.0)
                        .resizable(true);
                    self.context.sheet().columns().len() + 1
                ])
                .num_sticky_cols(1)
                .headers([egui_table::HeaderRow::new(
                    ui.text_style_height(&egui::TextStyle::Heading) + 4.0,
                )]);
            mutator(table).show(ui, self)
        });

        if let Some(icon_id) = &self.modal_image {
            let icon_id = *icon_id;
            let resp = Modal::new(Id::new("icon_modal"))
                .area(Modal::default_area(Id::new(format!("icon_modal{icon_id}"))))
                .show(ui.ctx(), |ui| {
                    let (excel, icon_mgr) = (
                        self.context.global().backend().excel().clone(),
                        &self.context.global().icon_manager(),
                    );
                    let resp = icon_mgr.get_or_insert_icon(icon_id, true, ui.ctx(), move || {
                        log::debug!("Hires icon not found in cache: {icon_id}");
                        TrackedPromise::spawn_local(
                            async move { excel.get_icon(icon_id, true).await },
                        )
                    });
                    match resp {
                        ManagedIcon::Loaded(icon) => {
                            ui.add(egui::Image::new(icon).fit_to_exact_size(ui.available_size()))
                        }
                        ManagedIcon::Failed(e) => {
                            ui.label("Failed to load icon").on_hover_text(e.to_string())
                        }
                        ManagedIcon::Loading => {
                            let height = ui.text_style_height(&egui::TextStyle::Heading) * 2.0;
                            ui.add_sized(Vec2::splat(height), Spinner::new())
                        }
                        ManagedIcon::NotLoaded => ui.label("Icon not loaded"),
                    }
                });
            if resp.should_close() {
                self.modal_image = None;
            }
        }
    }

    pub fn context(&self) -> &TableContext {
        &self.context
    }

    pub fn get_row_nr(&self, row_id: u32, subrow_id: Option<u16>) -> anyhow::Result<u64> {
        let max = self.context.sheet().subrow_count() as u64;
        let result = (0..max).collect_vec().binary_search_by(|i| {
            let (i_row, i_subrow) = self.get_row_id(*i).unwrap();
            i_row.cmp(&row_id).then_with(|| i_subrow.cmp(&subrow_id))
        });
        match result {
            Ok(idx) => Ok(idx as u64),
            Err(idx) => Err(anyhow::anyhow!("Row ID not found: {row_id} => {idx}")),
        }
    }

    fn get_row_id(&self, row_nr: u64) -> anyhow::Result<(u32, Option<u16>)> {
        if let Some(lookup) = &self.subrow_lookup {
            let row_idx = lookup
                .binary_search(&(row_nr as u32))
                .unwrap_or_else(|i| i - 1);
            let row_id = self.context.sheet().get_row_id_at(row_idx as u32)?;
            Ok((row_id, Some((row_nr - lookup[row_idx] as u64) as u16)))
        } else {
            let row_id = self.context.sheet().get_row_id_at(row_nr as u32)?;
            Ok((row_id, None))
        }
    }

    fn get_row_data(&self, row_id: u32, subrow_id: Option<u16>) -> anyhow::Result<ExcelRow<'_>> {
        if let Some(subrow_id) = subrow_id {
            self.context.sheet().get_subrow(row_id, subrow_id)
        } else {
            self.context.sheet().get_row(row_id)
        }
    }

    fn size_row_single_uncached(&self, ui: &mut egui::Ui, row_nr: u64) -> anyhow::Result<f32> {
        let (row_id, subrow_id) = self.get_row_id(row_nr)?;
        let row = self.get_row_data(row_id, subrow_id)?;
        let size = (0..self.context.sheet().columns().len())
            .filter_map(|column_idx| self.context.cell_by_offset(row, column_idx as u32).ok())
            .map(|c| c.size(ui))
            .reduce(|a, b| a.max(b));
        Ok(size.unwrap_or_default() + 4.0)
    }

    fn size_row(&self, ctx: &egui::Context, row_nr: u64) -> anyhow::Result<f32> {
        let mut row_sizes = self.row_sizes.borrow_mut();
        if let Some(size) = row_sizes.get(row_nr as usize) {
            return Ok(*size);
        }
        let mut ui = egui::Ui::new(
            ctx.clone(),
            Id::new("size_row").with(row_nr),
            UiBuilder::new().sizing_pass(),
        );

        let (len, mut last_size) = (
            row_sizes.len() as u64,
            row_sizes.last().copied().unwrap_or_default(),
        );
        row_sizes.reserve((row_nr - len + 1) as usize);
        for i in len..row_nr {
            row_sizes.push(last_size);
            last_size += self.size_row_single_uncached(&mut ui, i)?;
        }
        row_sizes.push(last_size);
        Ok(row_sizes[row_nr as usize])
    }
}

impl TableDelegate for SheetTable {
    fn header_cell_ui(&mut self, ui: &mut egui::Ui, cell_inf: &egui_table::HeaderCellInfo) {
        let egui_table::HeaderCellInfo { col_range, .. } = cell_inf;

        let column_idx = if col_range.start == 0 {
            None
        } else {
            Some(col_range.start - 1)
        };

        let sorted_by_offset = SORTED_BY_OFFSET.get(ui.ctx());

        let column = column_idx.and_then(|c| {
            Some((
                c,
                if sorted_by_offset {
                    self.context.get_column_by_offset(c as u32)
                } else {
                    self.context.get_column_by_index(c as u32)
                }
                .ok()?,
            ))
        });

        let margin = 4;

        egui::Frame::NONE
            .inner_margin(Margin::symmetric(margin, 2))
            .show(ui, |ui| {
                if let Some((column_id, (schema_column, sheet_column))) = column {
                    ui.heading(schema_column.name).on_hover_text(format!(
                        "Id: {}\nIndex: {}\nOffset: {}\nKind: {:?}",
                        sheet_column.id,
                        column_id,
                        sheet_column.offset(),
                        sheet_column.kind()
                    ));
                } else {
                    ui.heading("Row");
                }
            });
    }

    fn cell_ui(&mut self, ui: &mut egui::Ui, cell_info: &egui_table::CellInfo) {
        let egui_table::CellInfo { row_nr, col_nr, .. } = *cell_info;

        let column_idx = if col_nr == 0 { None } else { Some(col_nr - 1) };

        let row_data = self
            .get_row_id(row_nr)
            .and_then(|(r, s)| Ok((r, s, self.get_row_data(r, s)?)));
        let (row_id, subrow_id, row_data) = match row_data {
            Ok(row_data) => row_data,
            Err(error) => {
                log::error!("Failed to get row data: {error:?}");
                return;
            }
        };

        let sorted_by_offset = SORTED_BY_OFFSET.get(ui.ctx());

        if TEMP_HIGHLIGHTED_ROW_NR.try_get(ui.ctx()) == Some(row_nr) {
            ui.painter().rect_filled(
                ui.max_rect(),
                0.0,
                ui.visuals().warn_fg_color.linear_multiply(0.2),
            );
        }

        if row_nr % 2 == 1 {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, ui.visuals().faint_bg_color);
        }

        let resp = egui::Frame::NONE
            .inner_margin(Margin::symmetric(4, 2))
            .show(ui, |ui| {
                if let Some(column_idx) = column_idx {
                    let cell = if sorted_by_offset {
                        self.context.cell_by_offset(row_data, column_idx as u32)
                    } else {
                        self.context.cell_by_index(row_data, column_idx as u32)
                    };
                    match cell {
                        Ok(cell) => cell.show(ui),
                        Err(e) => {
                            log::error!("Failed to get column {column_idx}: {e:?}");
                            InnerResponse::new(crate::sheet::cell::CellResponse::None, ui.label(""))
                        }
                    }
                } else {
                    let resp = if let Some(subrow_id) = subrow_id {
                        ui.label(format!("{row_id}.{subrow_id}"))
                            .on_hover_text(format!("Row {row_id}, Subrow {subrow_id}"))
                    } else {
                        ui.label(row_id.to_string())
                            .on_hover_text(format!("Row {row_id}"))
                    };
                    InnerResponse::new(crate::sheet::cell::CellResponse::None, resp)
                }
            })
            .inner;
        if let CellResponse::Icon(icon_id) = resp.inner {
            self.modal_image = Some(icon_id);
        }
    }

    fn row_top_offset(&self, ctx: &egui::Context, _table_id: Id, row_nr: u64) -> f32 {
        match self.size_row(ctx, row_nr) {
            Ok(size) => size,
            Err(error) => {
                log::error!("Failed to size row {}: {:?}", row_nr, error);
                0.0
            }
        }
    }
}
