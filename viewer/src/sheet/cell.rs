use anyhow::bail;
use egui::{
    Color32, CursorIcon, Direction, InnerResponse, Layout, Sense, Vec2, Widget,
    color_picker::show_color_at, ecolor::HexColor,
};
use either::Either;
use ironworks::{file::exh::ColumnKind, sestring::SeString};

use crate::{
    excel::{
        get_icon_path,
        provider::{ExcelProvider, ExcelRow, ExcelSheet},
    },
    settings::{ALWAYS_HIRES, DISPLAY_FIELD_SHOWN, EVALUATE_STRINGS, TEXT_MAX_LINES},
    sheet::{string_label_wrapped, wrap_string_lines},
    utils::{ManagedIcon, TrackedPromise},
};

use super::{
    GlobalContext, copyable_label,
    schema_column::{SchemaColumn, SchemaColumnMeta},
    sheet_column::SheetColumnDefinition,
    table_context::TableContext,
};

pub struct Cell<'a> {
    row: ExcelRow<'a>,
    // This can be either a SchemaColumn or a SchemaColumnMeta::Link to a vector of strings (as a reference)
    schema_column: Either<SchemaColumn, &'a Vec<String>>,
    sheet_column: &'a SheetColumnDefinition,
    table_context: &'a TableContext,
}

pub type SheetRef = (
    String,             // sheet name
    (u32, Option<u16>), // row id, subrow id
);

#[derive(Default)]
pub enum CellResponse {
    #[default]
    None,
    Icon(u32),
    Link(SheetRef),
    Row(SheetRef),
}

pub enum CellValue {
    String(SeString<'static>),
    Integer(i128),
    Float(f32),
    Boolean(bool),
    Icon(u32),
    ModelId(Either<u32, u64>),
    Color(Color32),
    InvalidLink(i128),
    InProgressLink(i128),
    ValidLink {
        sheet_name: String,
        row_id: u32,
        value: Option<Box<CellValue>>,
    },
}

impl<'a> Cell<'a> {
    pub fn new(
        row: ExcelRow<'a>,
        schema_column: SchemaColumn,
        sheet_column: &'a SheetColumnDefinition,
        table_context: &'a TableContext,
    ) -> Self {
        Self {
            row,
            schema_column: Either::Left(schema_column),
            sheet_column,
            table_context,
        }
    }

    fn draw(self, ui: &mut egui::Ui) -> anyhow::Result<InnerResponse<CellResponse>> {
        self.read(DISPLAY_FIELD_SHOWN.get(ui.ctx()))
            .map(|value| value.show(ui, self.table_context.global()))
    }

    fn size_text(&self, ui: &mut egui::Ui) -> f32 {
        ui.text_style_height(&egui::TextStyle::Body)
    }

    fn size_text_multiline(&self, ui: &mut egui::Ui, text: String) -> f32 {
        let max_lines = TEXT_MAX_LINES.get(ui.ctx());
        let mut line_count = wrap_string_lines(ui, &text);
        if let Some(max_lines) = max_lines {
            line_count = line_count.min(max_lines.get().into());
        }
        self.size_text(ui) * line_count as f32
    }

    fn size_internal_link(&self, ui: &mut egui::Ui, sheets: &[String]) -> anyhow::Result<f32> {
        let row_id: isize = read_integer(
            self.row,
            self.sheet_column.offset() as u32,
            self.sheet_column.kind(),
        )?;

        Ok(
            match row_id
                .try_into()
                .ok()
                .and_then(|id| self.table_context.resolve_link(sheets, id))
            {
                Some(Some((_, table))) => {
                    if let Some(cell) =
                        table.display_field_cell(table.sheet().get_row(row_id as u32).unwrap())
                    {
                        cell?.size_internal(ui)?
                    } else {
                        self.size_text(ui)
                    }
                }
                _ => self.size_text(ui),
            },
        )
    }

    fn size_internal(&self, ui: &mut egui::Ui) -> anyhow::Result<f32> {
        Ok(match &self.schema_column {
            Either::Left(schema_column) => match schema_column.meta() {
                SchemaColumnMeta::Scalar => {
                    if self.sheet_column.kind() == ColumnKind::String {
                        let text = read_string(
                            self.row,
                            self.sheet_column.offset() as u32,
                            self.sheet_column.kind(),
                            ui,
                        )?;
                        self.size_text_multiline(ui, text)
                    } else {
                        self.size_text(ui)
                    }
                }
                SchemaColumnMeta::Icon => 32.0,
                SchemaColumnMeta::ModelId => self.size_text(ui),
                SchemaColumnMeta::Color => self.size_text(ui),
                SchemaColumnMeta::Link(sheets) => self.size_internal_link(ui, sheets)?,
                SchemaColumnMeta::ConditionalLink { column_idx, links } => {
                    let (_, switch_column) =
                        self.table_context.get_column_by_offset(*column_idx)?;
                    let switch_data: i32 = read_integer(
                        self.row,
                        switch_column.offset() as u32,
                        switch_column.kind(),
                    )?;
                    if let Some(sheets) = links.get(&switch_data) {
                        Cell {
                            row: self.row,
                            schema_column: Either::Right(sheets),
                            sheet_column: self.sheet_column,
                            table_context: self.table_context,
                        }
                        .size_internal(ui)?
                    } else {
                        self.size_text(ui)
                    }
                }
            },
            Either::Right(sheets) => self.size_internal_link(ui, sheets)?,
        })
    }

    pub fn size(&self, ui: &mut egui::Ui, row_location: (u32, Option<u16>)) -> f32 {
        self.size_internal(ui).unwrap_or_else(|err| {
            log::error!(
                "Failed to size cell (row {row_location:?}, col {}): {:?}",
                self.sheet_column.id,
                err
            );
            self.size_text(ui)
        })
    }

    pub fn size_pass(self, ui: &mut egui::Ui) -> anyhow::Result<f32> {
        let mut size_ui = ui.new_child(egui::UiBuilder::new().sizing_pass());
        self.draw(&mut size_ui)?;
        Ok(size_ui.min_rect().size().y)
    }

    pub fn show(self, ui: &mut egui::Ui) -> InnerResponse<CellResponse> {
        match self.draw(ui) {
            Ok(resp) => resp,
            Err(err) => {
                log::error!("Failed to draw cell: {err:?}");
                let resp = ui
                    .colored_label(Color32::LIGHT_RED, "⚠")
                    .on_hover_text(err.to_string());
                InnerResponse::new(CellResponse::None, resp)
            }
        }
    }

    fn read_internal_link(
        &self,
        resolve_display_field: bool,
        sheets: &[String],
    ) -> anyhow::Result<CellValue> {
        let row_id: i128 = read_integer(
            self.row,
            self.sheet_column.offset() as u32,
            self.sheet_column.kind(),
        )?;

        Ok(
            match row_id.try_into().ok().and_then(|id| {
                self.table_context
                    .resolve_link(sheets, id)
                    .map(|r| r.map(|(s, t)| (s, t, id)))
            }) {
                Some(Some((sheet_name, table, row_id))) => {
                    let display_field_cell = resolve_display_field
                        .then(|| table.display_field_cell(table.sheet().get_row(row_id).unwrap()))
                        .flatten();

                    CellValue::ValidLink {
                        sheet_name,
                        row_id,
                        value: display_field_cell
                            .map(|cell| -> anyhow::Result<Box<CellValue>> {
                                Ok(Box::new(cell?.read(resolve_display_field)?))
                            })
                            .transpose()?,
                    }
                }
                Some(None) => CellValue::InProgressLink(row_id),
                None => CellValue::InvalidLink(row_id),
            },
        )
    }

    pub fn read(&self, resolve_display_field: bool) -> anyhow::Result<CellValue> {
        Ok(match &self.schema_column {
            Either::Left(schema_column) => match schema_column.meta() {
                SchemaColumnMeta::Scalar => read_scalar(
                    self.row,
                    self.sheet_column.offset() as u32,
                    self.sheet_column.kind(),
                )?,
                SchemaColumnMeta::Icon => {
                    let icon_id: u32 = read_integer(
                        self.row,
                        self.sheet_column.offset() as u32,
                        self.sheet_column.kind(),
                    )?;
                    CellValue::Icon(icon_id)
                }
                SchemaColumnMeta::ModelId => {
                    if self.sheet_column.kind() == ColumnKind::Int64
                        || self.sheet_column.kind() == ColumnKind::UInt64
                    {
                        let model_id: u64 = read_integer(
                            self.row,
                            self.sheet_column.offset() as u32,
                            self.sheet_column.kind(),
                        )?;
                        CellValue::ModelId(Either::Right(model_id))
                    } else {
                        let model_id: u32 = read_integer(
                            self.row,
                            self.sheet_column.offset() as u32,
                            self.sheet_column.kind(),
                        )?;
                        CellValue::ModelId(Either::Left(model_id))
                    }
                }
                SchemaColumnMeta::Color => {
                    let color: u32 = read_integer(
                        self.row,
                        self.sheet_column.offset() as u32,
                        self.sheet_column.kind(),
                    )?;
                    let [r, g, b, a] = color.to_be_bytes();
                    let color = Color32::from_rgba_unmultiplied(r, g, b, a);
                    CellValue::Color(color)
                }
                SchemaColumnMeta::Link(sheets) => {
                    self.read_internal_link(resolve_display_field, sheets)?
                }
                SchemaColumnMeta::ConditionalLink { column_idx, links } => {
                    let (_, switch_column) =
                        self.table_context.get_column_by_offset(*column_idx)?;
                    let switch_data: i32 = read_integer(
                        self.row,
                        switch_column.offset() as u32,
                        switch_column.kind(),
                    )?;
                    let sheets = links.get(&switch_data);
                    let sheets = match sheets {
                        Some(sheets) => sheets,
                        None => &vec![],
                    };
                    return Cell {
                        row: self.row,
                        schema_column: Either::Right(sheets),
                        sheet_column: self.sheet_column,
                        table_context: self.table_context,
                    }
                    .read(resolve_display_field);
                }
            },
            Either::Right(sheets) => self.read_internal_link(resolve_display_field, sheets)?,
        })
    }
}

fn read_scalar(row: ExcelRow<'_>, offset: u32, kind: ColumnKind) -> anyhow::Result<CellValue> {
    Ok(match kind {
        ColumnKind::String => CellValue::String(row.read_string(offset)?.as_owned()),
        ColumnKind::Bool => CellValue::Boolean(row.read_bool(offset)?),
        ColumnKind::Int8 => CellValue::Integer(i128::from(row.read::<i8>(offset)?)),
        ColumnKind::UInt8 => CellValue::Integer(i128::from(row.read::<u8>(offset)?)),
        ColumnKind::Int16 => CellValue::Integer(i128::from(row.read::<i16>(offset)?)),
        ColumnKind::UInt16 => CellValue::Integer(i128::from(row.read::<u16>(offset)?)),
        ColumnKind::Int32 => CellValue::Integer(i128::from(row.read::<i32>(offset)?)),
        ColumnKind::UInt32 => CellValue::Integer(i128::from(row.read::<u32>(offset)?)),
        ColumnKind::Float32 => CellValue::Float(row.read::<f32>(offset)?),
        ColumnKind::Int64 => CellValue::Integer(i128::from(row.read::<i64>(offset)?)),
        ColumnKind::UInt64 => CellValue::Integer(i128::from(row.read::<u64>(offset)?)),
        ColumnKind::PackedBool0
        | ColumnKind::PackedBool1
        | ColumnKind::PackedBool2
        | ColumnKind::PackedBool3
        | ColumnKind::PackedBool4
        | ColumnKind::PackedBool5
        | ColumnKind::PackedBool6
        | ColumnKind::PackedBool7 => {
            let packed_index = (u16::from(kind) - u16::from(ColumnKind::PackedBool0)) as u8;
            CellValue::Boolean(row.read_packed_bool(offset, packed_index)?)
        }
    })
}

fn read_string(
    row: ExcelRow<'_>,
    offset: u32,
    kind: ColumnKind,
    ui: &mut egui::Ui,
) -> anyhow::Result<String> {
    match read_scalar(row, offset, kind)? {
        CellValue::String(s) => Ok(if EVALUATE_STRINGS.get(ui.ctx()) {
            s.format()
        } else {
            s.macro_string()
        }?),
        CellValue::Boolean(b) => Ok(b.to_string()),
        CellValue::Integer(i) => Ok(i.to_string()),
        CellValue::Float(f) => Ok(f.to_string()),
        _ => unreachable!(),
    }
}

fn read_integer<T: num_traits::NumCast>(
    row: ExcelRow<'_>,
    offset: u32,
    kind: ColumnKind,
) -> anyhow::Result<T> {
    match read_scalar(row, offset, kind)? {
        CellValue::Integer(i) => T::from(i).ok_or_else(|| {
            anyhow::anyhow!(
                "Failed to convert integer value: {} to target type: {}",
                i,
                std::any::type_name::<T>()
            )
        }),
        _ => bail!("Invalid column kind for integer: {kind:?}"),
    }
}

impl CellValue {
    pub fn show(self, ui: &mut egui::Ui, ctx: &GlobalContext) -> InnerResponse<CellResponse> {
        let resp = match self {
            CellValue::String(value) => string_label_wrapped(ui, &value),
            CellValue::Integer(value) => copyable_label(ui, &value),
            CellValue::Float(value) => copyable_label(ui, &value),
            CellValue::Boolean(value) => copyable_label(ui, &value),
            CellValue::Icon(icon_id) => {
                let resp = draw_icon(ctx, ui, icon_id).on_hover_cursor(CursorIcon::PointingHand);
                if resp.clicked() {
                    return InnerResponse::new(CellResponse::Icon(icon_id), resp);
                }
                resp
            }
            CellValue::ModelId(model_id) => {
                let label = model_id.map_either(
                    |model_id| {
                        let model = (model_id & 0xFFFF) as u16;
                        let variant = ((model_id >> 16) & 0xFF) as u8;
                        let stain = ((model_id >> 24) & 0xFF) as u8;
                        format!("{model}, {variant}, {stain}")
                    },
                    |weapon_id| {
                        let skeleton = (weapon_id & 0xFFFF) as u16;
                        let model = ((weapon_id >> 16) & 0xFFFF) as u16;
                        let variant = ((weapon_id >> 32) & 0xFFFF) as u16;
                        let stain = ((weapon_id >> 48) & 0xFFFF) as u16;
                        format!("{skeleton}, {model}, {variant}, {stain}")
                    },
                );
                copyable_label(ui, &label)
            }
            CellValue::Color(color) => draw_color(ui, color),
            CellValue::InProgressLink(row_id) => copyable_label(ui, &format!("...#{row_id}")),
            CellValue::InvalidLink(row_id) => copyable_label(ui, &format!("???#{row_id}")),
            CellValue::ValidLink {
                sheet_name,
                row_id,
                value,
            } => {
                let resp = if let Some(cell) = value {
                    let mut resp = cell.show(ui, ctx);
                    resp.response = resp
                        .response
                        .on_hover_text(format!("{sheet_name}#{row_id}"));
                    if !matches!(resp.inner, CellResponse::None) {
                        return resp;
                    }
                    resp.response
                } else {
                    copyable_label(ui, &format!("{sheet_name}#{row_id}"))
                }
                .on_hover_cursor(CursorIcon::Alias);

                if resp.clicked() {
                    return InnerResponse::new(
                        CellResponse::Link((sheet_name, (row_id, None))),
                        resp,
                    );
                }
                resp
            }
        };
        InnerResponse::new(CellResponse::None, resp)
    }
}

fn draw_icon(ctx: &GlobalContext, ui: &mut egui::Ui, icon_id: u32) -> egui::Response {
    let (excel, icon_mgr) = (ctx.backend().excel().clone(), &ctx.icon_manager());
    let hires = ALWAYS_HIRES.get(ui.ctx());
    let image_source = icon_mgr.get_or_insert_icon(icon_id, hires, ui.ctx(), move || {
        log::debug!("Icon not found in cache: {icon_id}");
        TrackedPromise::spawn_local(async move { excel.get_icon(icon_id, hires).await })
    });
    let resp = match image_source {
        ManagedIcon::Loaded(source) => {
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
        }
        ManagedIcon::Failed(_) => ui.label("Failed to load icon"),
        ManagedIcon::Loading => {
            ui.with_layout(
                Layout::centered_and_justified(Direction::LeftToRight),
                |ui| ui.add(egui::Spinner::new().size(32.0)),
            )
            .inner
        }
        ManagedIcon::NotLoaded => {
            unreachable!()
        }
    };
    let resp = resp.on_hover_text(format!(
        "Id: {icon_id}\nPath: {}",
        get_icon_path(icon_id, hires)
    ));
    resp.context_menu(|ui| {
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(icon_id.to_string());
            ui.close();
        }
        // ui.add_enabled_ui(image_source.is_some(), |ui| {
        //     if ui.button("Save").clicked() {
        //         image_source.unwrap().load(ctx, texture_options, size_hint)
        //     }
        // });
    });
    resp
}

fn draw_color(ui: &mut egui::Ui, color: Color32) -> egui::Response {
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
    let resp = resp.on_hover_text(hex.to_string());
    resp.context_menu(|ui| {
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(hex.to_string());
            ui.close();
        }
    });
    resp
}
