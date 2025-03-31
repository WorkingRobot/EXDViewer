use anyhow::bail;
use egui::{
    Align, Color32, Direction, Id, Layout, Margin, Sense, UiBuilder, Vec2, Widget,
    color_picker::show_color_at, ecolor::HexColor, mutex::RwLock,
};
use egui_table::TableDelegate;
use either::Either::{self, Left, Right};
use ironworks::{
    excel::Language,
    file::exh::{ColumnDefinition, ColumnKind},
};
use itertools::Itertools;
use std::{
    cell::{OnceCell, RefCell},
    collections::HashMap,
    ops::Deref,
    sync::Arc,
};

use crate::{
    backend::Backend,
    excel::{
        base::BaseSheet,
        get_icon_path,
        provider::{ExcelHeader, ExcelProvider, ExcelRow, ExcelSheet},
    },
    schema::{Field, FieldType, Schema, provider::SchemaProvider},
    utils::{CloneableResult, IconManager, TrackedPromise},
};

#[derive(Clone)]
pub struct SheetTable(Arc<RwLock<SheetTableImpl>>);

pub struct SheetTableImpl {
    sheet: BaseSheet,
    columns: Vec<SheetColumnDefinition>,
    subrow_lookup: Option<Vec<u32>>,
    row_sizes: RefCell<Vec<f32>>,
    schema_columns: Vec<SchemaColumn>,
    display_column_idx: Option<u32>,
    referenced_sheets: RefCell<
        HashMap<
            String,
            Either<
                TrackedPromise<anyhow::Result<(BaseSheet, Option<Schema>)>>,
                CloneableResult<SheetTable>,
            >,
        >,
    >,
    draw_state: OnceCell<DrawState>,
}

#[derive(Clone)]
struct DrawState {
    backend: Backend,
    language: Language,
    icon_manager: IconManager,
}

impl SheetTable {
    pub fn new(sheet: BaseSheet, schema: Option<Schema>) -> Self {
        Self(Arc::new(RwLock::new(SheetTableImpl::new(sheet, schema))))
    }

    fn new_with_state(sheet: BaseSheet, schema: Option<Schema>, state: DrawState) -> Self {
        let table = SheetTableImpl::new(sheet, schema);
        if table.draw_state.set(state).is_err() {
            panic!("Failed to set draw state");
        }
        Self(Arc::new(RwLock::new(table)))
    }

    pub fn name(&self) -> String {
        self.0.read().name().to_owned()
    }

    pub fn sheet(&self) -> BaseSheet {
        self.0.read().sheet.clone()
    }

    pub fn set_schema(&self, schema: Option<Schema>) -> anyhow::Result<()> {
        self.0.write().set_schema(schema)
    }

    pub fn draw(
        &self,
        backend: &Backend,
        language: Language,
        icon_manager: &IconManager,
        ui: &mut egui::Ui,
    ) {
        self.0.write().draw(backend, language, icon_manager, ui)
    }

    fn draw_display_column(
        &self,
        ui: &mut egui::Ui,
        row_id: u32,
        row: ExcelRow<'_>,
    ) -> anyhow::Result<()> {
        self.0.read().draw_display_column(ui, row_id, row)
    }
}

impl SheetTableImpl {
    pub fn new(sheet: BaseSheet, schema: Option<Schema>) -> Self {
        let (schema_columns, display_column_idx) = schema
            .as_ref()
            .and_then(|schema| SchemaColumn::from_schema(schema, true, true).ok())
            .unwrap_or_else(|| (SchemaColumn::from_blank(sheet.columns().len() as u32), None));

        let columns = SheetColumnDefinition::from_sheet(&sheet);
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
            sheet,
            columns,
            subrow_lookup,
            row_sizes,
            schema_columns,
            display_column_idx,
            referenced_sheets: RefCell::new(HashMap::new()),
            draw_state: OnceCell::new(),
        }
    }

    pub fn name(&self) -> &str {
        self.sheet.name()
    }

    pub fn set_schema(&mut self, schema: Option<Schema>) -> anyhow::Result<()> {
        (self.schema_columns, self.display_column_idx) = schema
            .map(|s| SchemaColumn::from_schema(&s, true, true))
            .map(|s| {
                s.map(|r| {
                    if r.0.len() == self.sheet.columns().len() {
                        Ok(r)
                    } else {
                        bail!(
                            "Schema column count does not match sheet column count: {} != {}",
                            r.0.len(),
                            self.sheet.columns().len()
                        )
                    }
                })?
            })
            .unwrap_or_else(|| {
                Ok((
                    SchemaColumn::from_blank(self.sheet.columns().len() as u32),
                    None,
                ))
            })?;
        Ok(())
    }

    pub fn set_draw_state(
        &mut self,
        backend: &Backend,
        language: Language,
        icon_manager: &IconManager,
    ) {
        self.draw_state.get_or_init(|| DrawState {
            backend: backend.clone(),
            language,
            icon_manager: icon_manager.clone(),
        });
    }

    pub fn draw(
        &mut self,
        backend: &Backend,
        language: Language,
        icon_manager: &IconManager,
        ui: &mut egui::Ui,
    ) {
        self.set_draw_state(backend, language, icon_manager);
        ui.push_id(Id::new(self.sheet.name()), |ui| {
            egui_table::Table::new()
                .num_rows(self.sheet.subrow_count().into())
                .columns(vec![
                    egui_table::Column::new(100.0)
                        .range(50.0..=10000.0)
                        .resizable(true);
                    self.columns.len() + 1
                ])
                .num_sticky_cols(1)
                .headers([egui_table::HeaderRow::new(
                    ui.text_style_height(&egui::TextStyle::Heading) + 4.0,
                )])
                .show(ui, self)
        });
    }

    fn get_row_id(&self, row_nr: u64) -> anyhow::Result<(u32, Option<u16>)> {
        if let Some(lookup) = &self.subrow_lookup {
            let row_idx = lookup
                .binary_search(&(row_nr as u32))
                .unwrap_or_else(|i| i - 1);
            let row_id = self.sheet.get_row_id_at(row_idx as u32)?;
            Ok((row_id, Some((row_nr - lookup[row_idx] as u64) as u16)))
        } else {
            let row_id = self.sheet.get_row_id_at(row_nr as u32)?;
            Ok((row_id, None))
        }
    }

    fn get_row_data(&self, row_id: u32, subrow_id: Option<u16>) -> anyhow::Result<ExcelRow<'_>> {
        if let Some(subrow_id) = subrow_id {
            self.sheet.get_subrow(row_id, subrow_id)
        } else {
            self.sheet.get_row(row_id)
        }
    }

    // vvv All functions below require DrawState vvv

    fn try_retrieve_linked_sheet(
        &self,
        ctx: &egui::Context,
        name: String,
    ) -> Option<anyhow::Result<SheetTable>> {
        let state = self.draw_state.get().unwrap();

        let mut sheets = self.referenced_sheets.borrow_mut();
        let entry = sheets.entry(name).or_insert_with_key(|name| {
            let backend = state.backend.clone();
            let name = name.clone();
            let language = state.language;
            Left(TrackedPromise::spawn_local(ctx.clone(), async move {
                let sheet_future = backend.excel().get_sheet(&name, language);
                let schema_future = backend.schema().get_schema_text(&name);
                Ok(futures_util::try_join!(
                    async move { sheet_future.await }, //.map_err(|e| CloneableError::from(e)) },
                    async move {
                        Ok(schema_future
                            .await
                            .and_then(|s| Schema::from_str(&s))
                            .map(|a| a.ok())
                            .ok()
                            .flatten())
                    }
                )?)
            }))
        });

        let should_swap = if let Left(promise) = entry {
            promise.ready().is_some()
        } else {
            false
        };

        if should_swap {
            let mut replaced_data = Right(Err(anyhow::anyhow!("Failed to swap back!").into()));
            std::mem::swap(entry, &mut replaced_data);
            let promise = match replaced_data {
                Left(promise) => promise,
                Right(_) => unreachable!(),
            };
            let result = poll_promise::Promise::from(promise).block_and_take();
            let new_result = result
                .and_then(|(sheet, schema)| {
                    Ok(SheetTable::new_with_state(
                        sheet.clone(),
                        schema,
                        self.draw_state.get().unwrap().clone(),
                    ))
                })
                .map_err(|e| e.into());
            replaced_data = Right(new_result);
            std::mem::swap(entry, &mut replaced_data);
        }

        match entry {
            Left(_) => None,
            Right(result) => Some(result.as_ref().cloned().map_err(|e| e.clone().into())),
        }
    }

    fn size_column(
        &self,
        ui: &mut egui::Ui,
        row: ExcelRow<'_>,
        sheet_column: &SheetColumnDefinition,
        schema_column: &SchemaColumn,
    ) -> anyhow::Result<f32> {
        let mut size_ui = ui.new_child(UiBuilder::new().sizing_pass());
        let resp = egui::Frame::NONE
            .inner_margin(Margin::symmetric(4, 2))
            .show(&mut size_ui, |ui| {
                self.draw_column(ui, row, sheet_column, schema_column)
            });
        resp.inner?;
        Ok(size_ui.min_rect().size().y)
    }

    fn size_column_manual(
        &self,
        ui: &mut egui::Ui,
        row: ExcelRow<'_>,
        sheet_column: &SheetColumnDefinition,
        schema_column: &SchemaColumn,
    ) -> anyhow::Result<Option<f32>> {
        match &schema_column.meta {
            SchemaColumnMeta::Scalar
            | SchemaColumnMeta::ModelId
            | SchemaColumnMeta::Color
            | SchemaColumnMeta::Link(_)
            | SchemaColumnMeta::ConditionalLink { .. } => {
                let col_height = match sheet_column.kind() {
                    ColumnKind::String => {
                        let string = row
                            .read_string(sheet_column.offset() as u32)?
                            .format()
                            .unwrap_or_else(|_| "Unknown".to_owned());
                        ui.text_style_height(&egui::TextStyle::Body)
                            * string.split('\n').count() as f32
                    }
                    _ => ui.text_style_height(&egui::TextStyle::Body),
                };
                Ok(Some(col_height + 4.0)) // symmetric inner y margin of 2.0
            }
            SchemaColumnMeta::Icon => Ok(Some(32.0 + 4.0)),
        }
    }

    fn size_row_single_uncached(&self, ui: &mut egui::Ui, row_nr: u64) -> anyhow::Result<f32> {
        let (row_id, subrow_id) = self.get_row_id(row_nr)?;
        let row = self.get_row_data(row_id, subrow_id)?;
        let mut size = 0.0f32;
        for (sheet_column, schema_column) in self.columns.iter().zip_eq(&self.schema_columns) {
            let mut col_size = self.size_column_manual(ui, row, sheet_column, schema_column)?;
            if col_size.is_none() {
                col_size = Some(self.size_column(ui, row, sheet_column, schema_column)?);
            }
            size = size.max(col_size.unwrap());
        }
        Ok(size)
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

    fn copyable_label(ui: &mut egui::Ui, text: impl ToString) {
        ui.with_layout(
            Layout::centered_and_justified(Direction::LeftToRight).with_main_align(Align::Min),
            |ui| {
                let resp = egui::Label::new(text.to_string())
                    .sense(Sense::click())
                    .ui(ui);
                resp.context_menu(|ui| {
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(text.to_string());
                        ui.close_menu();
                    }
                });
            },
        );
    }

    fn draw_icon(&self, ui: &mut egui::Ui, icon_id: u32) {
        let state = self.draw_state.get().unwrap();

        let image_source = state.icon_manager.get_icon(icon_id).or_else(|| {
            log::info!("Icon not found in cache: {icon_id}");
            match state.backend.excel().get_icon(icon_id) {
                Ok(Left(url)) => Some(state.icon_manager.insert_icon_url(icon_id, url)),
                Ok(Right(image)) => Some(state.icon_manager.insert_icon_texture(
                    icon_id,
                    ui.ctx(),
                    image,
                )),
                Err(err) => {
                    log::error!("Failed to get icon: {:?}", err);
                    None
                }
            }
        });
        let resp = if let Some(source) = image_source {
            ui.with_layout(
                Layout::centered_and_justified(Direction::LeftToRight),
                |ui| {
                    egui::Image::new(source)
                        .sense(Sense::click())
                        .maintain_aspect_ratio(true)
                        .fit_to_exact_size(Vec2::new(f32::INFINITY, 32.0))
                        .ui(ui)
                },
            )
            .inner
        } else {
            ui.label("Icon not found")
        };
        resp.on_hover_text(format!(
            "Id: {icon_id}\nPath: {}",
            get_icon_path(icon_id, true)
        ))
        .context_menu(|ui| {
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(icon_id.to_string());
                ui.close_menu();
            }
            // ui.add_enabled_ui(image_source.is_some(), |ui| {
            //     if ui.button("Save").clicked() {
            //         image_source.unwrap().load(ctx, texture_options, size_hint)
            //     }
            // });
        });
    }

    fn draw_column(
        &self,
        ui: &mut egui::Ui,
        row: ExcelRow<'_>,
        sheet_column: &SheetColumnDefinition,
        schema_column: &SchemaColumn,
    ) -> anyhow::Result<()> {
        match &schema_column.meta {
            SchemaColumnMeta::Scalar => {
                let value = match sheet_column.kind() {
                    ColumnKind::String => row
                        .read_string(sheet_column.offset() as u32)?
                        .format()
                        .unwrap_or_else(|_| "Unknown".to_owned()),
                    ColumnKind::Bool => row.read_bool(sheet_column.offset() as u32).to_string(),
                    ColumnKind::Int8 => row.read::<i8>(sheet_column.offset() as u32)?.to_string(),
                    ColumnKind::UInt8 => row.read::<u8>(sheet_column.offset() as u32)?.to_string(),
                    ColumnKind::Int16 => row.read::<i16>(sheet_column.offset() as u32)?.to_string(),
                    ColumnKind::UInt16 => {
                        row.read::<u16>(sheet_column.offset() as u32)?.to_string()
                    }
                    ColumnKind::Int32 => row.read::<i32>(sheet_column.offset() as u32)?.to_string(),
                    ColumnKind::UInt32 => {
                        row.read::<u32>(sheet_column.offset() as u32)?.to_string()
                    }
                    ColumnKind::Float32 => {
                        row.read::<f32>(sheet_column.offset() as u32)?.to_string()
                    }
                    ColumnKind::Int64 => row.read::<i64>(sheet_column.offset() as u32)?.to_string(),
                    ColumnKind::UInt64 => {
                        row.read::<u64>(sheet_column.offset() as u32)?.to_string()
                    }
                    ColumnKind::PackedBool0
                    | ColumnKind::PackedBool1
                    | ColumnKind::PackedBool2
                    | ColumnKind::PackedBool3
                    | ColumnKind::PackedBool4
                    | ColumnKind::PackedBool5
                    | ColumnKind::PackedBool6
                    | ColumnKind::PackedBool7 => row
                        .read_packed_bool(
                            sheet_column.offset() as u32,
                            (u16::from(sheet_column.kind()) - u16::from(ColumnKind::PackedBool0))
                                as u8,
                        )
                        .to_string(),
                };
                Self::copyable_label(ui, value);
            }
            SchemaColumnMeta::Icon => {
                let icon_id: u32 = match sheet_column.kind() {
                    ColumnKind::Int8 => row.read::<i8>(sheet_column.offset() as u32)?.try_into()?,
                    ColumnKind::UInt8 => row.read::<u8>(sheet_column.offset() as u32)?.into(),
                    ColumnKind::Int16 => {
                        row.read::<i16>(sheet_column.offset() as u32)?.try_into()?
                    }
                    ColumnKind::UInt16 => row.read::<u16>(sheet_column.offset() as u32)?.into(),
                    ColumnKind::Int32 => {
                        row.read::<i32>(sheet_column.offset() as u32)?.try_into()?
                    }
                    ColumnKind::UInt32 => row.read::<u32>(sheet_column.offset() as u32)?.into(),
                    // ColumnKind::Int64 => row.read::<i64>(sheet_column.offset() as u32)?.to_string(),
                    // ColumnKind::UInt64 => {
                    //     row.read::<u64>(sheet_column.offset() as u32)?.to_string()
                    // }
                    _ => {
                        bail!("Invalid column kind for icon: {:?}", sheet_column.kind());
                    }
                };
                self.draw_icon(ui, icon_id);
            }
            SchemaColumnMeta::ModelId => {
                let model_id: u64 = match sheet_column.kind() {
                    ColumnKind::UInt32 => row.read::<u32>(sheet_column.offset() as u32)?.into(),
                    ColumnKind::UInt64 => row.read::<u64>(sheet_column.offset() as u32)?,
                    _ => {
                        bail!(
                            "Invalid column kind for model id: {:?}",
                            sheet_column.kind()
                        );
                    }
                };
                Self::copyable_label(ui, model_id);
            }
            SchemaColumnMeta::Color => {
                let color: u32 = match sheet_column.kind() {
                    ColumnKind::UInt32 => row.read::<u32>(sheet_column.offset() as u32)?,
                    _ => {
                        bail!("Invalid column kind for color: {:?}", sheet_column.kind());
                    }
                };
                let color_bytes = u32::to_le_bytes(color);
                let (a, r, g, b) = color_bytes.iter().collect_tuple().unwrap();
                let color = Color32::from_rgba_unmultiplied(*r, *g, *b, *a);
                let resp = {
                    let (rect, response) =
                        ui.allocate_at_least(ui.available_size_before_wrap(), Sense::click());
                    if ui.is_rect_visible(rect) {
                        show_color_at(ui.painter(), color, rect);
                    }
                    response
                };
                let hex = if color.a() == u8::MAX {
                    HexColor::Hex6(color)
                } else {
                    HexColor::Hex8(color)
                };
                resp.on_hover_text(hex.to_string()).context_menu(|ui| {
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(hex.to_string());
                        ui.close_menu();
                    }
                });
            }
            SchemaColumnMeta::Link(sheets) => {
                let row_id: u32 = match sheet_column.kind() {
                    ColumnKind::Int8 => row.read::<i8>(sheet_column.offset() as u32)?.try_into()?,
                    ColumnKind::UInt8 => row.read::<u8>(sheet_column.offset() as u32)?.into(),
                    ColumnKind::Int16 => {
                        row.read::<i16>(sheet_column.offset() as u32)?.try_into()?
                    }
                    ColumnKind::UInt16 => row.read::<u16>(sheet_column.offset() as u32)?.into(),
                    ColumnKind::Int32 => {
                        row.read::<i32>(sheet_column.offset() as u32)?.try_into()?
                    }
                    ColumnKind::UInt32 => row.read::<u32>(sheet_column.offset() as u32)?.into(),
                    ColumnKind::Int64 => {
                        row.read::<i64>(sheet_column.offset() as u32)?.try_into()?
                    }
                    ColumnKind::UInt64 => {
                        row.read::<u64>(sheet_column.offset() as u32)?.try_into()?
                    }
                    _ => {
                        bail!("Invalid column kind for link: {:?}", sheet_column.kind());
                    }
                };

                let mut drawn = false;
                for sheet_name in sheets {
                    if let Some(result) =
                        self.try_retrieve_linked_sheet(ui.ctx(), sheet_name.clone())
                    {
                        match result {
                            Ok(table) => {
                                if let Ok(row) = table.sheet().get_row(row_id) {
                                    table.draw_display_column(ui, row_id, row)?;
                                    drawn = true;
                                    break;
                                }
                            }
                            Err(err) => {
                                log::error!("Failed to retrieve linked sheet: {:?}", err);
                            }
                        }
                    } else {
                        Self::copyable_label(ui, format!("...#{row_id}"));
                        drawn = true;
                        break;
                    }
                }

                if !drawn {
                    Self::copyable_label(ui, format!("???#{row_id}"));
                }
            }
            SchemaColumnMeta::ConditionalLink { column_idx, links } => {
                let switch_column = self.columns.get(*column_idx as usize).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Failed to find column index for conditional link: {}",
                        column_idx
                    )
                })?;
                let switch_data: i32 = match switch_column.kind() {
                    ColumnKind::Int8 => row.read::<i8>(switch_column.offset() as u32)?.into(),
                    ColumnKind::UInt8 => row.read::<u8>(switch_column.offset() as u32)?.into(),
                    ColumnKind::Int16 => row.read::<i16>(switch_column.offset() as u32)?.into(),
                    ColumnKind::UInt16 => row.read::<u16>(switch_column.offset() as u32)?.into(),
                    ColumnKind::Int32 => row.read::<i32>(switch_column.offset() as u32)?,
                    ColumnKind::UInt32 => {
                        row.read::<u32>(switch_column.offset() as u32)?.try_into()?
                    }
                    ColumnKind::Int64 => {
                        row.read::<i64>(switch_column.offset() as u32)?.try_into()?
                    }
                    ColumnKind::UInt64 => {
                        row.read::<u64>(switch_column.offset() as u32)?.try_into()?
                    }
                    _ => {
                        bail!(
                            "Invalid column kind for condition link's switch: {:?}",
                            switch_column.kind()
                        );
                    }
                };
                if let Some(sheets) = links.get(&switch_data) {
                    self.draw_column(
                        ui,
                        row,
                        sheet_column,
                        &SchemaColumn {
                            name: schema_column.name.clone(),
                            meta: SchemaColumnMeta::Link(sheets.clone()),
                        },
                    )?;
                } else {
                    Self::copyable_label(ui, format!("???#{switch_data}"));
                }
            }
        }
        Ok(())
    }

    fn draw_display_column(
        &self,
        ui: &mut egui::Ui,
        row_id: u32,
        row: ExcelRow<'_>,
    ) -> anyhow::Result<()> {
        if let Some(column_idx) = self.display_column_idx {
            if let (Some(sheet_column), Some(schema_column)) = (
                self.columns.get(column_idx as usize),
                self.schema_columns.get(column_idx as usize),
            ) {
                return self.draw_column(ui, row, sheet_column, schema_column);
            }
        }
        Self::copyable_label(ui, format!("{}#{row_id}", self.name()));
        Ok(())
    }
}

impl TableDelegate for SheetTableImpl {
    fn header_cell_ui(&mut self, ui: &mut egui::Ui, cell_inf: &egui_table::HeaderCellInfo) {
        let egui_table::HeaderCellInfo { col_range, .. } = cell_inf;

        let column_id = if col_range.start == 0 {
            None
        } else {
            Some(col_range.start - 1)
        };
        let column =
            column_id.and_then(|c| Some((c, (self.columns.get(c)?, self.schema_columns.get(c)?))));

        let margin = 4;

        egui::Frame::NONE
            .inner_margin(Margin::symmetric(margin, 2))
            .show(ui, |ui| {
                if let Some((column_id, (sheet_column, schema_column))) = column {
                    ui.heading(&schema_column.name).on_hover_text(format!(
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

        let column_id = if col_nr == 0 { None } else { Some(col_nr - 1) };

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

        if row_nr % 2 == 1 {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, ui.visuals().faint_bg_color);
        }

        egui::Frame::NONE
            .inner_margin(Margin::symmetric(4, 2))
            .show(ui, |ui| {
                if let Some(column_id) = column_id {
                    let sheet_column = self.columns.get(column_id).unwrap();
                    let schema_column = self.schema_columns.get(column_id).unwrap();
                    match self.draw_column(ui, row_data, sheet_column, schema_column) {
                        Ok(()) => {}
                        Err(error) => {
                            log::error!(
                                "Failed to read column (kind {:?}, offset {}, name {}): {:?}",
                                sheet_column.kind(),
                                sheet_column.offset(),
                                schema_column.name,
                                error
                            );
                        }
                    }
                } else {
                    if let Some(subrow_id) = subrow_id {
                        ui.label(format!("{row_id}.{subrow_id}"))
                            .on_hover_text(format!("Row {row_id}, Subrow {subrow_id}"));
                    } else {
                        ui.label(row_id.to_string())
                            .on_hover_text(format!("Row {row_id}"));
                    }
                }
            });
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

struct SchemaColumn {
    name: String,
    meta: SchemaColumnMeta,
}

impl SchemaColumn {
    fn get_columns_inner(
        ret: &mut Vec<Self>,
        scope: String,
        fields: &[Field],
        pending_names: bool,
        is_array: bool,
    ) -> anyhow::Result<()> {
        let column_offset = ret.len() as u32;

        let mut column_placeholder = u32::MAX;
        let mut column_lookups = vec![];

        for field in fields {
            let mut scope = scope.clone();
            if is_array {
                if let Some(name) = field.name(pending_names) {
                    scope.push('.');
                    scope.push_str(name);
                }
            } else {
                scope.push_str(field.name(pending_names).unwrap_or("Unk"));
            }

            if field.r#type == FieldType::Array {
                let subfields = field.fields.as_deref();
                let subfields = match subfields {
                    Some(subfields) => subfields,
                    None => &[Field::default()],
                };
                for i in 0..(field.count.unwrap_or(1)) {
                    Self::get_columns_inner(
                        ret,
                        scope.clone() + &format!("[{}]", i),
                        subfields,
                        pending_names,
                        true,
                    )?;
                }
            } else {
                let name = scope;

                let meta = match field.r#type {
                    FieldType::Scalar => SchemaColumnMeta::Scalar,
                    FieldType::Icon => SchemaColumnMeta::Icon,
                    FieldType::ModelId => SchemaColumnMeta::ModelId,
                    FieldType::Color => SchemaColumnMeta::Color,
                    FieldType::Link => {
                        if let Some(targets) = &field.targets {
                            SchemaColumnMeta::Link(targets.clone())
                        } else if let Some(condition) = &field.condition {
                            column_placeholder -= 1;
                            column_lookups.push(&condition.switch);
                            SchemaColumnMeta::ConditionalLink {
                                column_idx: column_placeholder,
                                links: condition.cases.clone(),
                            }
                        } else {
                            bail!("Link field missing targets or condition: {:?}", field);
                        }
                    }
                    FieldType::Array => unreachable!(),
                };

                ret.push(Self { name, meta });
            }
        }

        for i in 0..ret.len() - column_offset as usize {
            let column = &ret[column_offset as usize + i];
            if let SchemaColumnMeta::ConditionalLink { column_idx, .. } = column.meta {
                let column_lookup_idx = u32::MAX - column_idx - 1;
                let column_lookup_name = match column_lookups.get(column_lookup_idx as usize) {
                    Some(&name) => name,
                    None => {
                        bail!(
                            "Failed to find column lookup name for {}'s conditional link: {}",
                            column.name,
                            column_lookup_idx
                        );
                    }
                };

                let resolved_column_idx = column_offset
                    + match ret[column_offset as usize..]
                        .iter()
                        .enumerate()
                        .find_map(|(i, c)| {
                            if c.name[scope.len()..] == *column_lookup_name {
                                Some(i as u32)
                            } else {
                                None
                            }
                        }) {
                        Some(idx) => idx,
                        None => {
                            bail!(
                                "Failed to find column index for {}'s conditional link: {}",
                                column.name,
                                column_lookup_name
                            );
                        }
                    };

                if let SchemaColumnMeta::ConditionalLink { column_idx, .. } =
                    &mut ret[column_offset as usize + i].meta
                {
                    *column_idx = resolved_column_idx;
                } else {
                    unreachable!();
                }
            }
        }

        Ok(())
    }

    fn from_schema(
        schema: &Schema,
        pending_fields: bool,
        pending_names: bool,
    ) -> anyhow::Result<(Vec<Self>, Option<u32>)> {
        let fields = pending_fields
            .then_some(())
            .and(schema.pending_fields.as_ref())
            .unwrap_or(&schema.fields);

        let mut ret = vec![];
        Self::get_columns_inner(&mut ret, "".to_string(), fields, pending_names, false)?;

        let display_idx = if let Some(display_field) = &schema.display_field {
            ret.iter()
                .find_position(|c| c.name == *display_field)
                .map(|f| f.0 as u32)
        } else {
            None
        };

        Ok((ret, display_idx))
    }

    fn from_blank(column_count: u32) -> Vec<Self> {
        (0..column_count)
            .map(|i| Self {
                name: format!("Column{}", i),
                meta: SchemaColumnMeta::Scalar,
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
enum SchemaColumnMeta {
    Scalar,
    Icon,
    ModelId,
    Color,
    Link(Vec<String>),
    ConditionalLink {
        column_idx: u32,
        links: HashMap<i32, Vec<String>>,
    },
}
