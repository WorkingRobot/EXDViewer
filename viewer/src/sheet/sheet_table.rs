use egui::{Align, Color32, Id, InnerResponse, Layout, Margin, Modal, Spinner, UiBuilder};
use egui_table::TableDelegate;
use itertools::Itertools;
use lru::LruCache;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};
use std::{
    cell::{Cell, RefCell},
    num::NonZero,
    rc::Rc,
};
#[cfg(target_arch = "wasm32")]
use web_time::{Duration, Instant};

use crate::{
    excel::provider::{ExcelHeader, ExcelProvider, ExcelRow, ExcelSheet},
    settings::{SORTED_BY_OFFSET, TEMP_HIGHLIGHTED_ROW},
    stopwatch::Stopwatch,
    utils::{ManagedIcon, PromiseKind, TrackedPromise, yield_to_ui},
};

use super::{cell::CellResponse, table_context::TableContext};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct FilterKey {
    pub text: String,
    pub resolve_display_field: bool,
}

type FilterPromise = TrackedPromise<anyhow::Result<FilterOutput>>;
struct FilterOutput {
    // Filtered rows (by row_nr)
    filtered_rows: Vec<u32>,
    is_in_progress: bool,
}
struct FilterValue {
    filter_result: anyhow::Result<FilterOutput>,
    // Cached row offsets, indexed by row_nr
    row_offsets: Rc<RefCell<Vec<f32>>>,
}

pub struct SheetTable {
    context: TableContext,
    // Accumulated subrow count (row_nr), indexed by row index (not ID)
    // This is used to map row_nr to row_id and subrow_id
    subrow_lookup: Option<Vec<u32>>,
    // Precomputed row sizes, indexed by row_nr
    row_sizes: Vec<f32>,

    modal_image: Option<u32>,

    clicked_cell: Option<CellResponse>,

    filtered_rows: RefCell<LruCache<FilterKey, FilterValue>>,
    unfiltered_row_offsets: Rc<RefCell<Vec<f32>>>,
    last_filter: Option<FilterKey>,
    current_filter: Option<FilterKey>,
    current_filter_promise: Option<FilterPromise>,
    current_filter_cancel_token: Option<Rc<Cell<bool>>>,
}

impl SheetTable {
    pub fn new(context: TableContext, ui: &mut egui::Ui) -> Self {
        let sheet = context.sheet();

        let unfiltered_row_offsets = Rc::new(RefCell::new(Vec::with_capacity(
            sheet.subrow_count() as usize,
        )));
        let filtered_rows = RefCell::new(LruCache::new(NonZero::new(8).unwrap()));

        let subrow_lookup = if sheet.has_subrows() {
            let mut subrow_lookup = Vec::with_capacity(sheet.row_count() as usize);
            let mut offset = 0u32;
            for row_id in sheet.get_row_ids() {
                subrow_lookup.push(offset);
                offset += sheet.get_row_subrow_count(row_id).unwrap() as u32;
            }
            Some(subrow_lookup)
        } else {
            None
        };

        let mut row_sizes = Vec::with_capacity(sheet.subrow_count() as usize);
        {
            let _stop = Stopwatch::new(format!("Sizing - {}", sheet.name()));
            let mut sizing_ui = ui.new_child(UiBuilder::new().sizing_pass());
            for (row_id, subrow_id) in sheet.get_subrow_ids() {
                row_sizes.push(
                    context.size_row(sheet.get_subrow(row_id, subrow_id).unwrap(), &mut sizing_ui),
                );
            }
        }

        Self {
            context,
            subrow_lookup,
            row_sizes,
            modal_image: None,
            clicked_cell: None,
            filtered_rows,
            unfiltered_row_offsets,
            last_filter: None,
            current_filter: None,
            current_filter_promise: None,
            current_filter_cancel_token: None,
        }
    }

    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        scroll_to: Option<((u32, Option<u16>), u16)>,
    ) -> CellResponse {
        self.tick_filter();

        let id = Id::new(self.context.sheet().name());
        ui.push_id(id, |ui| {
            let mut table = egui_table::Table::new()
                .num_rows(self.get_filtered_row_count() as u64)
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
            if let Some(((row_id, subrow_id), column_id)) = scroll_to {
                if let Some(row_nr) = self.search_filtered_row_nr(row_id, subrow_id) {
                    table = table.scroll_to_row(row_nr, Some(Align::Center));
                }
                let sorted_by_offset = SORTED_BY_OFFSET.get(ui.ctx());
                let column_nr = if sorted_by_offset {
                    self.context
                        .convert_column_index_to_offset_index(column_id.into())
                        .ok()
                } else {
                    Some(column_id.into())
                };
                if let Some(col_nr) = column_nr {
                    table = table.scroll_to_column(col_nr as usize, Some(Align::Center));
                }
            }
            table.show(ui, self)
        });

        if let Some(icon_id) = &self.modal_image {
            let icon_id = *icon_id;
            let resp = Modal::new(Id::new("icon-modal"))
                .area(Modal::default_area(Id::new(format!(
                    "icon-modal-{icon_id}"
                ))))
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
                            let (rect, _) =
                                ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
                            ui.allocate_new_ui(
                                UiBuilder::new()
                                    .max_rect(rect)
                                    .layout(Layout::centered_and_justified(ui.layout().main_dir())),
                                |ui| {
                                    ui.add(Spinner::new().size(
                                        ui.text_style_height(&egui::TextStyle::Heading) * 3.0,
                                    ))
                                },
                            )
                            .inner
                        }
                        ManagedIcon::NotLoaded => ui.label("Icon not loaded"),
                    }
                });
            if resp.should_close() {
                self.modal_image = None;
            }
        }

        self.clicked_cell.take().unwrap_or_default()
    }

    pub fn context(&self) -> &TableContext {
        &self.context
    }

    fn search_filtered_row_nr(&mut self, row_id: u32, subrow_id: Option<u16>) -> Option<u64> {
        let max = self.get_filtered_row_count() as u64;
        let result = (0..max).collect_vec().binary_search_by(|i| {
            let (i_row, i_subrow) = self.get_row_id(self.get_filtered_row_nr(*i)).unwrap();
            i_row.cmp(&row_id).then_with(|| i_subrow.cmp(&subrow_id))
        });
        result.ok().map(|i| i as u64)
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

    fn get_filtered_row_offset(&self, filtered_row_nr: u64) -> anyhow::Result<f32> {
        let row_offsets = self.get_row_offsets();

        let mut row_offsets = row_offsets.borrow_mut();
        if let Some(offset) = row_offsets.get(filtered_row_nr as usize) {
            return Ok(*offset);
        }

        let (len, mut last_size) = (
            row_offsets.len() as u64,
            row_offsets.last().copied().unwrap_or_default(),
        );

        row_offsets.reserve((filtered_row_nr - len) as usize);
        for i in len..filtered_row_nr {
            row_offsets.push(last_size);
            last_size += self.row_sizes[self.get_filtered_row_nr(i) as usize];
        }
        row_offsets.push(last_size);
        Ok(row_offsets[filtered_row_nr as usize])
    }

    fn is_display_column(&self, column_idx: Option<usize>, sorted_by_offset: bool) -> bool {
        let mut is_display_column = false;
        if let (Some(column_idx), Some(display_idx)) =
            (column_idx, self.context.display_column_idx())
        {
            is_display_column = if sorted_by_offset {
                column_idx as u32 == display_idx
            } else {
                self.context
                    .convert_column_index_to_offset_index(column_idx as u32)
                    .map(|idx| idx == display_idx)
                    .unwrap_or_default()
            };
        }
        is_display_column
    }

    fn paint_cell_background(ui: &mut egui::Ui, color: Color32) {
        ui.painter().rect_filled(ui.max_rect(), 0.0, color);
    }

    pub fn set_filter(&mut self, filter: Option<FilterKey>) {
        if self.current_filter == filter {
            return;
        }

        if self
            .current_filter
            .as_ref()
            .map(|f| self.filtered_rows.get_mut().get(f).is_some())
            .unwrap_or(true)
        {
            self.last_filter = self.current_filter.clone();
        }

        self.current_filter = filter.clone();
        if let Some(token) = &self.current_filter_cancel_token {
            token.set(true);
        }
        self.current_filter_cancel_token.take();
        self.current_filter_promise.take();

        let filter = match filter {
            Some(f) => f,
            None => return,
        };
        if self.filtered_rows.get_mut().get(&filter).is_some() {
            return;
        }

        let token = Rc::new(Cell::new(false));
        let ctx = self.context().clone();
        let promise_token = token.clone();
        let promise = TrackedPromise::spawn_local(async move {
            let columns = ctx.columns()?;

            let batch_count = 0x4000usize.div_euclid(columns.len().max(1)).max(1);

            let iter: Box<dyn Iterator<Item = anyhow::Result<ExcelRow<'_>>>> =
                if ctx.sheet().has_subrows() {
                    Box::new(ctx.sheet().get_row_ids().flat_map(|row_id| {
                        let subrow_count = ctx
                            .sheet()
                            .get_row_subrow_count(row_id)
                            .expect("Row should exist");
                        let sheet = ctx.sheet();
                        (0..subrow_count).map(move |subrow_id| sheet.get_subrow(row_id, subrow_id))
                    }))
                } else {
                    Box::new(
                        ctx.sheet()
                            .get_row_ids()
                            .map(|row_id| ctx.sheet().get_row(row_id)),
                    )
                };

            let FilterKey {
                text: filter,
                resolve_display_field,
            } = filter;
            let mut filtered_rows = Vec::new();
            let mut is_in_progress = false;

            let mut last_now = Instant::now();
            let mut iters = 0;
            const MAX_FRAME_TIME: Duration = Duration::from_millis(250);
            for chunk in &iter.enumerate().chunks(batch_count) {
                for (row_nr, row) in chunk {
                    let (matches, in_progress) =
                        ctx.filter_row(&columns, &row?, &filter, resolve_display_field)?;
                    if in_progress {
                        is_in_progress = true;
                    }
                    if matches {
                        filtered_rows.push(row_nr as u32);
                    }
                }

                if promise_token.get() {
                    log::info!("Filter cancelled");
                    return Err(anyhow::anyhow!("Filter cancelled"));
                }

                let now = Instant::now();
                if now.duration_since(last_now) >= MAX_FRAME_TIME {
                    iters += 1;
                    last_now = now;
                    yield_to_ui().await;
                }
            }
            if iters > 0 {
                log::info!("Filter yielded {iters} times");
            }
            Ok(FilterOutput {
                filtered_rows,
                is_in_progress,
            })
        });
        self.current_filter_cancel_token = Some(token);
        self.current_filter_promise = Some(promise);
    }

    fn get_filtered_row_count(&mut self) -> usize {
        if let Some(current_filter) = &self.current_filter {
            if let Some(filter_value) = self.filtered_rows.get_mut().get(current_filter) {
                if let Ok(filter_output) = &filter_value.filter_result {
                    return filter_output.filtered_rows.len();
                }
            }
            if let Some(last_filter) = &self.last_filter {
                if let Some(filter_value) = self.filtered_rows.get_mut().get(last_filter) {
                    if let Ok(filter_output) = &filter_value.filter_result {
                        return filter_output.filtered_rows.len();
                    }
                }
            }
        }
        self.context.sheet().subrow_count() as usize
    }

    fn get_filtered_row_nr(&self, filtered_row_nr: u64) -> u64 {
        if let Some(current_filter) = &self.current_filter {
            if let Some(filter_value) = self.filtered_rows.borrow_mut().get(current_filter) {
                if let Ok(filter_output) = &filter_value.filter_result {
                    if let Some(&filtered_row_nr) =
                        filter_output.filtered_rows.get(filtered_row_nr as usize)
                    {
                        return filtered_row_nr.into();
                    }
                }
            }
            if let Some(last_filter) = &self.last_filter {
                if let Some(filter_value) = self.filtered_rows.borrow_mut().get(last_filter) {
                    if let Ok(filter_output) = &filter_value.filter_result {
                        if let Some(&filtered_row_nr) =
                            filter_output.filtered_rows.get(filtered_row_nr as usize)
                        {
                            return filtered_row_nr.into();
                        }
                    }
                }
            }
        }

        filtered_row_nr
    }

    fn get_row_offsets(&self) -> Rc<RefCell<Vec<f32>>> {
        self.current_filter
            .as_ref()
            .and_then(|f| {
                let mut rows = self.filtered_rows.borrow_mut();
                rows.get(f).map(|v| v.row_offsets.clone()).or_else(|| {
                    self.last_filter
                        .as_ref()
                        .and_then(|f| rows.get(f).map(|v| v.row_offsets.clone()))
                })
            })
            .unwrap_or_else(|| self.unfiltered_row_offsets.clone())
    }

    fn tick_filter(&mut self) {
        if let Some(promise) = self.current_filter_promise.take_if(|p| p.ready()) {
            let result = promise.block_and_take();
            self.filtered_rows.get_mut().push(
                self.current_filter.clone().unwrap(),
                FilterValue {
                    filter_result: result,
                    row_offsets: Rc::new(RefCell::new(Vec::new())),
                },
            );
        }
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

        let is_display_column = self.is_display_column(column_idx, sorted_by_offset);

        if is_display_column {
            Self::paint_cell_background(ui, Color32::LIGHT_BLUE.gamma_multiply(0.05));
        }

        egui::Frame::NONE
            .inner_margin(Margin::symmetric(4, 2))
            .show(ui, |ui| {
                if let Some((column_id, (schema_column, sheet_column))) = column {
                    ui.heading(schema_column.name()).on_hover_text(format!(
                        "Id: {}\nIndex: {}\nOffset: {}\nKind: {:?}{}{}",
                        sheet_column.id,
                        column_id,
                        sheet_column.offset(),
                        sheet_column.kind(),
                        if is_display_column {
                            "\nDisplay Field"
                        } else {
                            ""
                        },
                        schema_column
                            .comment()
                            .map(|x| format!("\nComment: {x}"))
                            .unwrap_or_default(),
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
            .get_row_id(self.get_filtered_row_nr(row_nr))
            .and_then(|(r, s)| {
                Ok((
                    r,
                    s,
                    self.context.sheet().get_subrow(r, s.unwrap_or_default())?,
                ))
            });
        let (row_id, subrow_id, row_data) = match row_data {
            Ok(row_data) => row_data,
            Err(error) => {
                log::error!("Failed to get row data: {error:?}");
                return;
            }
        };

        let sorted_by_offset = SORTED_BY_OFFSET.get(ui.ctx());

        if row_nr % 2 == 1 {
            Self::paint_cell_background(ui, ui.visuals().faint_bg_color);
        }

        if TEMP_HIGHLIGHTED_ROW.try_get(ui.ctx()) == Some((row_id, subrow_id)) {
            Self::paint_cell_background(ui, Color32::GOLD.gamma_multiply(0.2));
        }

        if self.is_display_column(column_idx, sorted_by_offset) {
            Self::paint_cell_background(ui, Color32::LIGHT_BLUE.gamma_multiply(0.05));
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
                            InnerResponse::new(CellResponse::None, ui.label(""))
                        }
                    }
                } else {
                    let resp = ui
                        .with_layout(
                            egui::Layout::centered_and_justified(egui::Direction::LeftToRight)
                                .with_main_align(egui::Align::Center),
                            |ui| {
                                if let Some(subrow_id) = subrow_id {
                                    ui.label(format!("{row_id}.{subrow_id}"))
                                        .on_hover_text(format!("Row {row_id}, Subrow {subrow_id}"))
                                } else {
                                    ui.label(row_id.to_string())
                                        .on_hover_text(format!("Row {row_id}"))
                                }
                            },
                        )
                        .inner
                        .on_hover_cursor(egui::CursorIcon::Copy);
                    let cell_resp = if resp.clicked() {
                        CellResponse::Row((
                            self.context.sheet().name().to_string(),
                            (row_id, subrow_id),
                        ))
                    } else {
                        CellResponse::None
                    };
                    InnerResponse::new(cell_resp, resp)
                }
            })
            .inner
            .inner;

        match resp {
            CellResponse::None => {}
            CellResponse::Icon(icon_id) => {
                self.modal_image = Some(icon_id);
            }
            CellResponse::Link(_) | CellResponse::Row(_) => {}
        }

        if !matches!(resp, CellResponse::None) {
            self.clicked_cell = Some(resp);
        }
    }

    fn row_top_offset(&self, _ctx: &egui::Context, _table_id: Id, row_nr: u64) -> f32 {
        match self.get_filtered_row_offset(row_nr) {
            Ok(size) => size,
            Err(error) => {
                log::error!("Failed to size row {}: {:?}", row_nr, error);
                0.0
            }
        }
    }
}
