//! `.mtrl` materials: the shader a surface is drawn with, the textures bound to it, and the colour
//! table applied to the surfaces that reference it.
//!
//! Laid out as three regions rather than one table, because the interesting parts are different
//! shapes: identity is label/value, the colour table is a grid of swatches, and the textures are
//! images worth previewing in place.

use anyhow::Result;
use egui::{Color32, RichText, ScrollArea, Sense, Vec2, load::SizedTexture};
use ironworks::file::{
    File,
    mtrl::{self, ColorRow},
};
use std::io::Cursor;

use super::{Preview, shader_names};
use crate::assets::deps::{Dep, Deps};
use crate::backend::Backend;
use crate::sheet::draw_color;
use crate::utils::file_name;

/// Edge length a texture preview is drawn at.
const THUMBNAIL: f32 = 64.0;

/// Sampler ids are stable across the game, and each says what its texture is *for*. Names and
/// filename suffixes from Penumbra.GameData's `ShpkFile`.
fn sampler_name(id: u32) -> Option<&'static str> {
    Some(match id {
        0x2005_679F => "Table",
        0x0C5E_C1F1 => "Normal",
        0x565F_8FD8 => "Index",
        0x2B99_E025 => "Specular",
        0x1153_06BE => "Diffuse",
        0x8A4E_82B6 => "Mask",
        _ => return None,
    })
}

/// An id as its name where one is known, falling back to hex only when nothing else is available.
fn label(id: u32, known: Option<&str>) -> String {
    known
        .map(str::to_owned)
        .or_else(|| shader_names::resolve(id).map(str::to_owned))
        .unwrap_or_else(|| format!("{id:#010x}"))
}

/// Shader keys select a variant within the shader package. Only a few categories are identified;
/// the rest keep their raw id, since a wrong name is worse than a number.
fn shader_key_name(category: u32) -> Option<&'static str> {
    Some(match category {
        0xB616_DC5A => "Texture mode",
        0xC8BD_1DEF => "Specular mode",
        0xF52C_CF05 => "VertexColor mode",
        0x2427_2923 => "Decal mode",
        0xA9A3_EE25 => "Skin type",
        0x380C_AED0 => "Hair type",
        0x24AA_C207 => "Flow type",
        _ => return None,
    })
}

/// The shader flag word, bit by bit. Only two bits are named anywhere (Penumbra.GameData's
/// `ShaderFlags`), but bits 2 and 3 are set on most materials in the game, so unknown bits are
/// listed individually rather than lumped into a hex remainder that hides how many there are.
fn shader_flags(flags: u32) -> String {
    (0..u32::BITS)
        .filter(|bit| flags & (1 << bit) != 0)
        .map(|bit| match bit {
            0 => "Hide backfaces".to_owned(),
            4 => "Enable transparency".to_owned(),
            other => format!("bit {other}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A shader key or constant: what it is, its name and hash, and its value -- which for a shader key
/// is another crc-named thing rather than a number.
struct Param {
    kind: &'static str,
    name: String,
    id: u32,
    value: String,
    value_id: Option<u32>,
}

/// A material, decoded and ready to draw.
pub struct Rendered {
    /// Shader package name, and where it lives so it can be opened.
    shader: (String, String),
    identity: Vec<(&'static str, String, Option<String>)>,
    /// Shader keys and constants: kind, id or name, and value.
    params: Vec<Param>,
    /// Sampler role, texture path, and whether it is the DX11 variant.
    textures: Vec<(String, String, bool)>,
    rows: Vec<ColorRow>,
    table_kind: Option<String>,
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let material = mtrl::Material::read(Cursor::new(bytes.to_vec()))?;

    let mut params: Vec<Param> = Vec::new();
    let mut identity = vec![
        // Version is an opaque tag, so hex is the only sensible rendering.
        ("Version", format!("{:#010x}", material.version()), None),
    ];
    if material.shader_flags() != 0 {
        identity.push((
            "Shader flags",
            shader_flags(material.shader_flags()),
            Some(format!("{:#010x}", material.shader_flags())),
        ));
    }
    for set in material.uv_sets() {
        identity.push(("UV set", format!("{} (#{})", set.name(), set.index()), None));
    }
    for set in material.color_sets() {
        identity.push((
            "Colour set",
            format!("{} (#{})", set.name(), set.index()),
            None,
        ));
    }
    for key in material.shader_keys() {
        params.push(Param {
            kind: "Shader key",
            name: label(key.category(), shader_key_name(key.category())),
            id: key.category(),
            value: label(key.value(), None),
            value_id: Some(key.value()),
        });
    }
    // Constants are floats; showing the id in hex and the values as numbers beats a byte dump.
    for constant in material.constants() {
        let Some(values) = material.constant_values(constant) else {
            continue;
        };
        let values = values
            .iter()
            .map(|v| format!("{v:.3}"))
            .collect::<Vec<_>>()
            .join(", ");
        params.push(Param {
            kind: "Constant",
            name: label(constant.id(), None),
            id: constant.id(),
            value: values,
            value_id: None,
        });
    }

    let textures = material
        .samplers()
        .iter()
        .map(|sampler| {
            let texture = sampler
                .texture_index()
                .and_then(|index| material.textures().get(usize::from(index)));
            let role = sampler_name(sampler.id())
                .map_or_else(|| format!("{:#010x}", sampler.id()), str::to_owned);
            (
                role,
                texture.map_or_else(String::new, |t| t.path().to_owned()),
                texture.is_some_and(mtrl::Texture::dx11),
            )
        })
        .collect();

    let (rows, table_kind) = match material.color_table() {
        Some(table) => (
            (0..table.rows())
                .filter_map(|i| table.row_values(i))
                .collect(),
            Some(format!("{:?}, {} rows", table.kind(), table.rows())),
        ),
        None => (Vec::new(), None),
    };

    log::info!(
        "assets/mtrl: {path} shader {}, {} samplers, {} colour rows",
        material.shader(),
        material.samplers().len(),
        rows.len()
    );

    Ok(Preview::Material(Box::new(Rendered {
        shader: (
            material.shader().to_owned(),
            format!("shader/sm5/shpk/{}", material.shader()),
        ),
        identity,
        params,
        textures,
        rows,
        table_kind,
    })))
}

/// Half-float colours are linear and can exceed 1.0, so they are tone-mapped rather than clamped;
/// otherwise every bright row renders as flat white.
fn swatch(color: [f32; 3]) -> Color32 {
    let map = |v: f32| ((v / (1.0 + v)).clamp(0.0, 1.0) * 255.0) as u8;
    Color32::from_rgb(map(color[0]), map(color[1]), map(color[2]))
}

/// A section title in the main area: the details panel's weak styling at heading size.
fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).text_style(egui::TextStyle::Heading));
    ui.add_space(4.0);
}

/// A path rendered as a link: hyperlink colour, pointer cursor, and the same hover and right-click
/// menu every other path in the browser gets. Returns whether it was followed.
fn link(ui: &mut egui::Ui, text: &str, path: &str) -> bool {
    let response = ui
        .add(
            egui::Label::new(
                RichText::new(text)
                    .monospace()
                    .color(ui.visuals().hyperlink_color),
            )
            .sense(Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    crate::assets::path_context(&response, path, None);
    response.clicked()
}

pub fn ui(
    ui: &mut egui::Ui,
    material: &Rendered,
    deps: &mut Deps,
    backend: &Backend,
) -> Option<String> {
    let mut follow = None;

    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !material.textures.is_empty() {
                section(ui, "Textures");
                for (role, path, dx11) in &material.textures {
                    ui.horizontal(|ui| {
                        // A sampler the material declares but binds nothing to has no image and no
                        // path; it is a normal thing for shaders that need no texture.
                        if path.is_empty() {
                            ui.add_sized(
                                Vec2::splat(THUMBNAIL),
                                egui::Label::new(RichText::new("--").weak()),
                            );
                            ui.vertical(|ui| {
                                ui.label(RichText::new(role).strong());
                                ui.label(RichText::new("no texture bound").weak());
                            });
                            return;
                        }
                        // Thumbnail first so the row keeps its height while the fetch is in flight.
                        match deps.texture(ui.ctx(), backend, path) {
                            Dep::Ready(handle) => {
                                let size = handle.size_vec2();
                                let scale = THUMBNAIL / size.x.max(size.y).max(1.0);
                                ui.add(
                                    egui::Image::new(SizedTexture::new(handle, size * scale))
                                        .maintain_aspect_ratio(true),
                                );
                            }
                            Dep::Pending => {
                                ui.add_sized(
                                    Vec2::splat(THUMBNAIL),
                                    egui::Spinner::new().size(THUMBNAIL / 2.0),
                                );
                            }
                            Dep::Failed => {
                                ui.add_sized(
                                    Vec2::splat(THUMBNAIL),
                                    egui::Label::new(RichText::new("⚠").color(Color32::LIGHT_RED)),
                                )
                                .on_hover_text("Failed to load");
                            }
                        }
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(role).strong());
                                if *dx11 {
                                    ui.label(RichText::new("DX11").weak());
                                }
                            });
                            // The directory is almost always the material's own, so only the file
                            // name earns the space; the full path is in the tooltip.
                            if link(ui, file_name(path), path) {
                                follow = Some(path.clone());
                            }
                        });
                    });
                }
            }

            if let Some(kind) = &material.table_kind {
                ui.add_space(8.0);
                ui.separator();
                section(ui, "Color table");
                ui.label(RichText::new(kind).weak());
                egui::Grid::new("mtrl_colors")
                    .num_columns(7)
                    .striped(true)
                    .show(ui, |ui| {
                        for header in [
                            "#", "Diffuse", "Specular", "Emissive", "Rough", "Metal", "Tile",
                        ] {
                            ui.label(RichText::new(header).weak());
                        }
                        ui.end_row();
                        for (index, row) in material.rows.iter().enumerate() {
                            ui.label(RichText::new(index.to_string()).monospace());
                            for color in [row.diffuse, row.specular, row.emissive] {
                                ui.scope(|ui| {
                                    ui.set_max_size(Vec2::new(40.0, 16.0));
                                    draw_color(ui, swatch(color));
                                });
                            }
                            ui.label(format!("{:.2}", row.roughness));
                            ui.label(format!("{:.2}", row.metalness));
                            ui.label(row.tile_index.to_string());
                            ui.end_row();
                        }
                    });
            }
        });
    follow
}

impl Rendered {
    pub fn has_params(&self) -> bool {
        true
    }
}

/// Shader keys and constants, drawn into the browser's Details panel.
pub fn details_ui(ui: &mut egui::Ui, material: &Rendered, follow: &mut Option<String>) {
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        egui::Grid::new("mtrl_identity")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                ui.label(RichText::new("Shader").weak());
                if link(ui, &material.shader.0, &material.shader.1) {
                    *follow = Some(material.shader.1.clone());
                }
                ui.end_row();
                for (label, value, raw) in &material.identity {
                    ui.label(RichText::new(*label).weak());
                    let shown = ui.label(RichText::new(value).monospace());
                    if let Some(raw) = raw {
                        shown.on_hover_text(raw);
                    }
                    ui.end_row();
                }
            });

        if material.params.is_empty() {
            return;
        }
        ui.add_space(8.0);
        ui.separator();
        ui.label(RichText::new("Parameters").weak());
        ui.add_space(4.0);
        egui::Grid::new("mtrl_params_grid")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for param in &material.params {
                    let name = ui.add(
                        egui::Label::new(RichText::new(&param.name).monospace())
                            .sense(Sense::click()),
                    );
                    crate::assets::crc_context(&name, param.kind, &param.name, param.id);
                    match param.value_id {
                        Some(id) => {
                            let value = ui.add(
                                egui::Label::new(RichText::new(&param.value).monospace())
                                    .sense(Sense::click()),
                            );
                            crate::assets::crc_context(&value, "Value", &param.value, id);
                        }
                        None => {
                            ui.label(RichText::new(&param.value).monospace());
                        }
                    }
                    ui.end_row();
                }
            });
    });
}
