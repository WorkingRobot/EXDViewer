//! Per-type views of a selected asset.
//!
//! Each viewer lives in its own module and is reached only through [`Viewer`], so adding a type is a
//! new file plus an arm here rather than edits threaded through the browser.

use egui::{
    Color32, Label, RichText, ScrollArea, TextureHandle, Vec2,
};

use super::{Bytes, Channels, MAX_TEXT_PREVIEW};

pub mod png;
pub mod material;
pub mod shader_names;
pub mod texture;

pub enum Preview {
    Text(String),
    Image {
        texture: TextureHandle,
        size: [usize; 2],
        /// Slice count for a volume texture; one for everything else.
        depth: u16,
        /// Channels the source format carries, which is what the channel toggles should offer.
        components: u8,
        /// Label/value pairs describing the source file.
        facts: Vec<(&'static str, String)>,
        mips: Vec<Mip>,
    },
    /// A parsed material, rendered as its own layout rather than a flat table.
    Material(Box<material::Rendered>),
    /// Nothing to render; an empty message means the type simply has no viewer.
    Failed(String),
}


impl Preview {
    pub fn decode(
        ctx: &egui::Context,
        path: &str,
        bytes: &[u8],
        viewer: Viewer,
        mip: u8,
        channels: Channels,
    ) -> Self {
        let result = match viewer {
            Viewer::Text => Ok(Self::Text(
                String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_TEXT_PREVIEW)]).into_owned(),
            )),
            Viewer::Image => png::decode(ctx, path, bytes, channels),
            Viewer::Texture => texture::decode(ctx, path, bytes, mip, channels),
            Viewer::Material => material::decode(path, bytes),
            Viewer::Raw => return Self::Failed(String::new()),
        };
        result.unwrap_or_else(|e| Self::Failed(e.to_string()))
    }

    /// Draws the preview. Returns a path when the user follows a link out of it, such as a
    /// material's texture.
    pub fn ui(
        &self,
        ui: &mut egui::Ui,
        slice: u16,
        deps: &mut crate::assets::deps::Deps,
        backend: &crate::backend::Backend,
    ) -> Option<String> {
        let mut follow = None;
        match self {
            Self::Material(material) => {
                follow = material::ui(ui, material, deps, backend);
            }
            Self::Failed(e) if e.is_empty() => {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("No viewer for this file type. Use Raw bytes.").weak());
                });
            }
            Self::Failed(e) => {
                ui.centered_and_justified(|ui| {
                    ui.colored_label(Color32::RED, format!("Could not render this file: {e}"));
                });
            }
            Self::Text(text) => {
                ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.add(Label::new(RichText::new(text).monospace()).selectable(true));
                });
            }
            Self::Image {
                texture,
                size,
                depth,
                ..
            } => {
                // The whole volume is resident, so changing slice is a different uv rect rather
                // than another decode and upload -- scrubbing costs nothing.
                let depth = f32::from((*depth).max(1));
                let top = f32::from(slice) / depth;
                let uv = egui::Rect::from_min_max(
                    egui::pos2(0.0, top),
                    egui::pos2(1.0, top + 1.0 / depth),
                );
                // `uv` only changes what is sampled; the widget still sizes itself from the whole
                // texture unless the source size is stated as one slice.
                let slice_size =
                    egui::vec2(size[0] as f32, (size[1] as f32 / depth).max(1.0));
                ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add(
                            egui::Image::new(egui::load::SizedTexture::new(texture.id(), slice_size))
                                .uv(uv)
                                .maintain_aspect_ratio(true)
                                .fit_to_original_size(1.0)
                                .max_width(ui.available_width().max(slice_size.x)),
                        );
                    });
                });
            }
        }
        follow
    }

    /// The info sidebar: property table, channel toggles, then the mipmap picker. Returns the new
    /// (level, channels) if either changed.
    /// Whether this preview has anything for the Details panel.
    pub fn has_details(&self) -> bool {
        match self {
            Self::Image { .. } => true,
            Self::Material(material) => material.has_params(),
            _ => false,
        }
    }

    pub fn info_ui(
        &self,
        ui: &mut egui::Ui,
        mip: u8,
        slice: u16,
        channels: Channels,
        follow: &mut Option<String>,
    ) -> Option<(u8, u16, Channels)> {
        if let Self::Material(material) = self {
            material::details_ui(ui, material, follow);
            return None;
        }
        let Self::Image {
            facts,
            mips,
            depth,
            components,
            ..
        } = self
        else {
            return None;
        };
        let (mut level, mut slice, mut channels) = (mip, slice, channels);
        let was = (mip, slice, channels);
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("asset_facts")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    for (label, value) in facts {
                        ui.label(RichText::new(*label).weak());
                        ui.label(value);
                        // Stripes are drawn across the summed column widths, so the last column has
                        // to take the slack or they stop short of the panel edge.
                        ui.allocate_space(Vec2::new(ui.available_width(), 0.0));
                        ui.end_row();
                    }
                });

            if *components > 1 {
                ui.add_space(8.0);
                ui.separator();
                ui.label(RichText::new("Channels").weak());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    // A single-component format is drawn as grey, so it has no toggles at all.
                    let offered: &mut [(&str, &mut bool)] = match components {
                        2 => &mut [("R", &mut channels.r), ("G", &mut channels.g)],
                        3 => &mut [
                            ("R", &mut channels.r),
                            ("G", &mut channels.g),
                            ("B", &mut channels.b),
                        ],
                        _ => &mut [
                            ("R", &mut channels.r),
                            ("G", &mut channels.g),
                            ("B", &mut channels.b),
                            ("A", &mut channels.a),
                        ],
                    };
                    for (name, flag) in offered {
                        ui.toggle_value(flag, *name);
                    }
                });
            }

            if *depth > 1 {
                ui.add_space(8.0);
                ui.separator();
                ui.label(RichText::new(if *depth == 6 { "Face" } else { "Slice" }).weak());
                ui.add_space(4.0);
                ui.add(egui::Slider::new(&mut slice, 0..=depth.saturating_sub(1)));
            }

            if mips.len() > 1 {
                ui.add_space(8.0);
                ui.separator();
                ui.label(RichText::new("Mipmaps").weak());
                ui.add_space(4.0);
                for entry in mips {
                    let label = format!(
                        "{}: {} x {}  ({})",
                        entry.level,
                        entry.width,
                        entry.height,
                        Bytes(entry.bytes)
                    );
                    if ui.selectable_label(entry.level == mip, label).clicked() {
                        level = entry.level;
                    }
                }
            }
        });
        ((level, slice, channels) != was).then_some((level, slice, channels))
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Viewer {
    Texture,
    Image,
    Material,
    Text,
    Raw,
}

impl Viewer {
    /// Everything except `Raw`, which the dropdown offers separately. Fixed order, so a given
    /// viewer sits in the same place whatever file is selected.
    pub const RENDERED: [Self; 4] = [Self::Texture, Self::Image, Self::Material, Self::Text];

    pub fn label(self) -> &'static str {
        match self {
            Self::Texture => "Texture",
            Self::Image => "Image",
            Self::Material => "Material",
            Self::Text => "Text",
            Self::Raw => "Raw bytes",
        }
    }

    /// What a path is shown with unless the dropdown says otherwise.
    pub fn recommended(path: &str) -> Self {
        match path.rsplit('.').next().unwrap_or_default() {
            "tex" | "atex" => Self::Texture,
            "png" => Self::Image,
            "mtrl" => Self::Material,
            "txt" | "csv" => Self::Text,
            _ => Self::Raw,
        }
    }
}

/// One mipmap level, for the picker under the info table.
pub struct Mip {
    pub level: u8,
    pub width: u16,
    pub height: u16,
    pub bytes: usize,
}

/// Build the image preview both the image and texture viewers end at.
pub(super) fn upload(
    ctx: &egui::Context,
    path: &str,
    image: image::DynamicImage,
    depth: u16,
    components: u8,
    facts: Vec<(&'static str, String)>,
    mips: Vec<Mip>,
    channels: Channels,
) -> Preview {
    let mut rgba = image.to_rgba8();
    channels.apply(&mut rgba);
    let size = [rgba.width() as usize, rgba.height() as usize];
    let texture = ctx.load_texture(
        format!("asset:{path}"),
        egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_flat_samples().as_slice()),
        // Nearest keeps a zoomed-in mipmap readable as actual texels rather than a blur.
        egui::TextureOptions::NEAREST,
    );
    Preview::Image {
        texture,
        size,
        depth,
        components,
        facts,
        mips,
    }
}
