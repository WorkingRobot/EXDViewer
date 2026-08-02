//! `.mdl` models, drawn.
//!
//! Geometry comes off the file when it is decoded; the materials it names are fetched afterwards and
//! land on meshes already on screen, so a model shows as untextured geometry first and dresses
//! itself as its textures arrive.
//!
//! The shading approximates the game's rather than reproducing it: a color table row is picked the
//! way the game picks one and drives a diffuse color, a specular color and a specular exponent, the
//! mask map scales all three, and everything is lit by three lights that follow the camera instead
//! of by the scene's. Skinning, shape keys, submesh visibility, dyes and decals are all absent, so
//! a character stands in bind pose with every part of it visible.

mod gpu;
mod material;

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use egui::{Color32, RichText, ScrollArea, Sense, TextureHandle, TextureOptions};
use glam::{Mat3, Mat4, Vec3};
use ironworks::file::{
    File,
    mdl::{Lod, MeshKind, ModelContainer, VertexAttributeKind, VertexFormat, VertexValues},
};
use std::io::Cursor;

use super::{Preview, facts, link, section};
use crate::assets::Bytes;
use crate::backend::Backend;
use crate::data::DecodedTexture;
use crate::utils::TrackedPromise;

use material::{Material, Role};

/// Longest edge a model's textures are decoded to.
const TEXTURE_SIZE: u16 = 512;

/// Decoded texture bytes one model may hold. Past it the rest of its surfaces draw untextured.
const TEXTURE_BUDGET: usize = 64 << 20;

/// Vertical field of view.
const FOV: f32 = 40.0_f32.to_radians();

/// How much of the model's radius the initial framing leaves as margin.
const MARGIN: f32 = 1.25;

/// Where the key light stands, in the model's own space. Anchored rather than carried with the
/// camera: a rig that turns with the eye shades every angle alike, so orbiting reveals no form.
const KEY: Vec3 = Vec3::new(-0.45, 0.78, 0.44);

/// A vertex as the shader reads it. `#[repr(C)]` with no padding, so a mesh uploads as its own
/// slice.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    tangent: [f32; 4],
    uv: [f32; 2],
    color: [u8; 4],
}

/// Where the camera is looking from.
#[derive(Clone, Copy)]
struct Camera {
    yaw: f32,
    pitch: f32,
    distance: f32,
    target: Vec3,
}

impl Camera {
    fn eye(&self) -> Vec3 {
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        self.target + self.distance * Vec3::new(cos_pitch * sin_yaw, sin_pitch, cos_pitch * cos_yaw)
    }
}

/// One mesh of the model, as far as the browser cares about it.
struct Mesh {
    material: usize,
    vertices: usize,
    triangles: usize,
}

/// A texture, from the moment it is asked for to the moment it can be bound.
enum Texture {
    Fetching(TrackedPromise<Result<DecodedTexture>>),
    Ready(TextureHandle),
    /// It would not load, or the model had already spent its budget.
    Absent,
}

/// A material, from the moment it is asked for to the moment it can be drawn with.
enum Slot {
    Fetching(TrackedPromise<Result<Vec<u8>>>),
    Ready(Box<Material>),
    Failed(String),
}

/// One detail level's geometry, and everything the browser says about it.
struct Level {
    identity: Vec<(&'static str, String)>,
    meshes: Vec<Mesh>,
    /// Material paths, in the order meshes index them.
    materials: Vec<String>,
    /// Meshes the file lists but whose vertices would not read, with why.
    unreadable: Vec<(usize, String)>,
    /// Framing the model starts at, so the view can be put back.
    home: Camera,
    /// Half the bounding box's diagonal, which the depth range is cut to.
    radius: f32,
    gpu: Arc<Mutex<gpu::Model>>,
}

/// A model, decoded and ready to draw. Everything a detail level owns is rebuilt when one is
/// picked; the camera and the fetched materials and textures are not, so switching neither moves
/// the view nor asks for anything twice.
pub struct Rendered {
    path: String,
    bytes: Vec<u8>,
    lod: Cell<u8>,
    level: RefCell<Level>,
    slots: RefCell<Vec<Option<Slot>>>,
    textures: RefCell<BTreeMap<String, Texture>>,
    camera: Cell<Camera>,
    /// Decoded texture bytes handed to egui so far.
    resident: Cell<usize>,
    debug: Cell<gpu::Debug>,
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let level = read_level(path, bytes, 0)?;
    let camera = level.home;
    Ok(Preview::Model(Box::new(Rendered {
        path: path.to_owned(),
        bytes: bytes.to_vec(),
        lod: Cell::new(0),
        slots: RefCell::new((0..level.materials.len()).map(|_| None).collect()),
        level: RefCell::new(level),
        textures: Default::default(),
        camera: Cell::new(camera),
        resident: Cell::new(0),
        debug: Cell::new(gpu::Debug::None),
    })))
}

fn read_level(path: &str, bytes: &[u8], lod: u8) -> Result<Level> {
    let container = ModelContainer::read(Cursor::new(bytes.to_vec()))?;
    let level = match lod {
        0 => Lod::High,
        1 => Lod::Medium,
        _ => Lod::Low,
    };
    let model = container.model(level);

    let mut names: Vec<String> = Vec::new();
    let mut meshes = Vec::new();
    let mut unreadable = Vec::new();
    let mut pending = gpu::Pending::default();
    let mut low = Vec3::splat(f32::INFINITY);
    let mut high = Vec3::splat(f32::NEG_INFINITY);

    let mut skipped: Vec<MeshKind> = Vec::new();
    for (index, mesh) in model.meshes().into_iter().enumerate() {
        if !mesh.kinds().contains(&MeshKind::Standard) {
            for kind in mesh.kinds() {
                if !skipped.contains(kind) {
                    skipped.push(*kind);
                }
            }
            continue;
        }
        let built = match (mesh.attributes(), mesh.indices()) {
            (Ok(attributes), Ok(indices)) => build(&attributes, indices),
            (Err(why), _) | (_, Err(why)) => Err(why.to_string()),
        };
        let (vertices, indices) = match built {
            Ok(built) => built,
            Err(why) => {
                unreadable.push((index, why));
                continue;
            }
        };

        for vertex in &vertices {
            let position = Vec3::from_array(vertex.position);
            low = low.min(position);
            high = high.max(position);
        }

        let name = mesh.material().unwrap_or_default();
        let resolved = material::path(&name).unwrap_or(name);
        let material = names
            .iter()
            .position(|held| *held == resolved)
            .unwrap_or_else(|| {
                names.push(resolved);
                names.len() - 1
            });
        meshes.push(Mesh {
            material,
            vertices: vertices.len(),
            triangles: indices.len() / 3,
        });
        pending.meshes.push((vertices, indices));
    }

    if meshes.is_empty() {
        let why = match (unreadable.first(), skipped.is_empty()) {
            (Some((_, why)), _) => format!("no mesh of this model could be read: {why}"),
            (None, false) => format!(
                "this model draws nothing on its own: every mesh is {}",
                skipped
                    .iter()
                    .map(|kind| match kind {
                        MeshKind::Water => "water",
                        MeshKind::Shadow => "shadow",
                        MeshKind::Terrain => "terrain shadow",
                        MeshKind::VerticalFog => "vertical fog",
                        MeshKind::LightShaft => "light shaft",
                        MeshKind::Glass => "glass",
                        MeshKind::MaterialChange => "material change",
                        MeshKind::CrestChange => "crest change",
                        MeshKind::Standard => "standard",
                    })
                    .collect::<Vec<_>>()
                    .join(" or ")
            ),
            (None, true) => {
                "this model holds no standard meshes at its highest detail level".to_owned()
            }
        };
        anyhow::bail!(why);
    }

    let center = (low + high) * 0.5;
    let radius = ((high - low).length() * 0.5).max(0.01);
    let home = Camera {
        yaw: 0.0,
        pitch: 0.15,
        distance: radius / (FOV * 0.5).tan() * MARGIN,
        target: center,
    };

    let vertices: usize = meshes.iter().map(|mesh| mesh.vertices).sum();
    let triangles: usize = meshes.iter().map(|mesh| mesh.triangles).sum();
    let identity = vec![
        ("Meshes", meshes.len().to_string()),
        ("Vertices", vertices.to_string()),
        ("Triangles", triangles.to_string()),
        ("Materials", names.len().to_string()),
        (
            "Bounds",
            format!(
                "{:.2} x {:.2} x {:.2}",
                high.x - low.x,
                high.y - low.y,
                high.z - low.z
            ),
        ),
        (
            "Buffers",
            Bytes(vertices * size_of::<Vertex>() + triangles * 6).to_string(),
        ),
    ];

    log::info!(
        "assets/mdl: {path} {} meshes, {vertices} vertices, {} materials, {} unreadable",
        meshes.len(),
        names.len(),
        unreadable.len()
    );

    Ok(Level {
        identity,
        meshes,
        materials: names,
        unreadable,
        home,
        radius,
        gpu: gpu::Model::new(pending),
    })
}

/// Interleaves the attributes a mesh declares into the one buffer the shader reads. A mesh missing
/// a normal, tangent, UV or color gets a default rather than being dropped.
fn build(
    attributes: &[ironworks::file::mdl::VertexAttribute],
    indices: Vec<u16>,
) -> Result<(Vec<Vertex>, Vec<u16>), String> {
    let mut positions = None;
    let mut normals = None;
    let mut tangents = None;
    let mut uvs = None;
    let mut colors = None;
    for attribute in attributes {
        let slot = match attribute.kind {
            VertexAttributeKind::Position => &mut positions,
            VertexAttributeKind::Normal => &mut normals,
            VertexAttributeKind::Tangent1 => &mut tangents,
            VertexAttributeKind::Uv => &mut uvs,
            VertexAttributeKind::Color => &mut colors,
            _ => continue,
        };
        // The lowest usage index rather than the first declared: a second UV or color set belongs
        // to shading this viewer does not do, and nothing promises the sets arrive in order.
        if slot.is_none_or(|(held, _, _)| attribute.usage_index < held) {
            *slot = Some((attribute.usage_index, &attribute.values, attribute.format));
        }
    }
    let positions = positions.map(|(_, values, _)| values);
    let normals = normals.map(|(_, values, _)| values);
    // Only a byte tangent arrives scaled to 0..1; a half or float one is already signed.
    let signed = tangents.is_some_and(|(_, _, format)| format != VertexFormat::ByteFloat4);
    let tangents = tangents.map(|(_, values, _)| values);
    let uvs = uvs.map(|(_, values, _)| values);
    let colors = colors.map(|(_, values, _)| values);

    let Some(positions) = positions else {
        return Err("mesh declares no vertex positions".into());
    };
    let count = match positions {
        VertexValues::Vector3(values) => values.len(),
        VertexValues::Vector4(values) => values.len(),
        _ => return Err("vertex positions are not a vector".into()),
    };
    if let Some(index) = indices.iter().find(|index| usize::from(**index) >= count) {
        return Err(format!(
            "index {index} names none of the mesh's {count} vertices"
        ));
    }

    let vertices = (0..count)
        .map(|at| Vertex {
            position: xyz(positions, at).unwrap_or_default(),
            normal: normals
                .and_then(|held| xyz(held, at))
                .unwrap_or([0.0, 1.0, 0.0]),
            // A mesh with no tangent gets a zero one, which the shader reads as "no tangent"
            // rather than lighting a normal map through a frame nothing measured.
            tangent: tangents.and_then(|held| xyzw(held, at)).map_or(
                [0.0; 4],
                |value| match signed {
                    true => value,
                    false => value.map(|channel| channel * 2.0 - 1.0),
                },
            ),
            uv: uvs.and_then(|held| uv(held, at)).unwrap_or_default(),
            color: colors
                .and_then(|held| xyzw(held, at))
                .map_or([255; 4], |value| {
                    value.map(|channel| (channel.clamp(0.0, 1.0) * 255.0) as u8)
                }),
        })
        .collect();
    Ok((vertices, indices))
}

fn xyz(values: &VertexValues, at: usize) -> Option<[f32; 3]> {
    match values {
        VertexValues::Vector3(held) => held.get(at).copied(),
        VertexValues::Vector4(held) => held.get(at).map(|value| [value[0], value[1], value[2]]),
        _ => None,
    }
}

fn xyzw(values: &VertexValues, at: usize) -> Option<[f32; 4]> {
    match values {
        VertexValues::Vector4(held) => held.get(at).copied(),
        _ => None,
    }
}

/// A half4 UV element carries two sets packed as `xy` and `zw`, so the first two components are the
/// first set whichever shape the element took.
fn uv(values: &VertexValues, at: usize) -> Option<[f32; 2]> {
    match values {
        VertexValues::Vector2(held) => held.get(at).copied(),
        VertexValues::Vector3(held) => held.get(at).map(|value| [value[0], value[1]]),
        VertexValues::Vector4(held) => held.get(at).map(|value| [value[0], value[1]]),
        _ => None,
    }
}

/// What the debug row offers, in the order it offers it.
const VIEWS: [(gpu::Debug, &str); 9] = [
    (gpu::Debug::Normals, "Normals"),
    (gpu::Debug::Geometry, "Geometric"),
    (gpu::Debug::Tangents, "Tangents"),
    (gpu::Debug::Bitangents, "Bitangents"),
    (gpu::Debug::Handedness, "Handedness"),
    (gpu::Debug::Uv, "UVs"),
    (gpu::Debug::Color, "Vertex color"),
    (gpu::Debug::Alpha, "Vertex alpha"),
    (gpu::Debug::Meshes, "Meshes"),
];

pub fn ui(ui: &mut egui::Ui, model: &Rendered, backend: &Backend) {
    ui.horizontal_wrapped(|ui| {
        let debug = model.debug.get();
        for (mode, label) in VIEWS {
            if ui.selectable_label(debug == mode, label).clicked() {
                model.debug.set(match debug == mode {
                    true => gpu::Debug::None,
                    false => mode,
                });
            }
        }
        let level = model.level.borrow();
        if ui.button("Reset view").clicked() {
            model.camera.set(level.home);
        }
        if !level.unreadable.is_empty() {
            ui.label(
                RichText::new(format!("⚠ {} unreadable meshes", level.unreadable.len()))
                    .color(Color32::LIGHT_RED),
            )
            .on_hover_text(
                level
                    .unreadable
                    .iter()
                    .map(|(index, why)| format!("mesh {index}: {why}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
    });

    if let Some(why) = model.level.borrow().gpu.lock().unwrap().failure() {
        ui.centered_and_justified(|ui| {
            ui.colored_label(Color32::RED, format!("Could not build the shader: {why}"));
        });
        return;
    }

    model.poll(ui, backend);
    model.viewport(ui);
}

impl Rendered {
    /// Asks for whatever the model still needs, and hands what arrived to egui. Runs every frame;
    /// a slot that is already resolved costs a lookup.
    fn poll(&self, ui: &egui::Ui, backend: &Backend) {
        let level = self.level.borrow();
        let mut slots = self.slots.borrow_mut();
        for (index, slot) in slots.iter_mut().enumerate() {
            let path = &level.materials[index];
            match slot {
                None => {
                    let files = backend.files().clone();
                    let wanted = path.clone();
                    *slot = Some(Slot::Fetching(TrackedPromise::spawn_local(async move {
                        files.read(&wanted).await
                    })));
                }
                Some(Slot::Fetching(promise)) => {
                    let Some(result) = promise.try_get() else {
                        continue;
                    };
                    *slot = Some(match result {
                        Ok(bytes) => match Material::parse(bytes) {
                            Ok(material) => Slot::Ready(Box::new(material)),
                            Err(why) => Slot::Failed(why.to_string()),
                        },
                        Err(why) => {
                            log::error!("assets/mdl: {path}: {why}");
                            Slot::Failed(why.to_string())
                        }
                    });
                    if let Some(Slot::Ready(material)) = slot
                        && let Some(table) = material.table()
                    {
                        level.gpu.lock().unwrap().queue_table(index, table.to_vec());
                    }
                }
                Some(_) => {}
            }
        }

        let mut textures = self.textures.borrow_mut();
        for slot in slots.iter().flatten() {
            let Slot::Ready(material) = slot else {
                continue;
            };
            for path in material.textures() {
                if textures.contains_key(path) {
                    continue;
                }
                if self.resident.get() >= TEXTURE_BUDGET {
                    log::warn!("assets/mdl: {path}: past this model's texture budget");
                    textures.insert(path.clone(), Texture::Absent);
                    continue;
                }
                let files = backend.files().clone();
                let wanted = path.clone();
                textures.insert(
                    path.clone(),
                    Texture::Fetching(TrackedPromise::spawn_local(async move {
                        files.read_texture(&wanted, Some(TEXTURE_SIZE)).await
                    })),
                );
            }
        }
        for (path, texture) in textures.iter_mut() {
            let Texture::Fetching(promise) = texture else {
                continue;
            };
            let Some(result) = promise.try_get() else {
                continue;
            };
            *texture = match result {
                Ok(decoded) => {
                    let size = [
                        decoded.image.width() as usize,
                        decoded.image.height() as usize,
                    ];
                    self.resident
                        .set(self.resident.get() + size[0] * size[1] * 4);
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        size,
                        decoded.image.as_flat_samples().as_slice(),
                    );
                    // Model UVs tile, and a texture bound to a surface is minified far more often
                    // than it is magnified, so this is the one place the browser wants mipmaps and
                    // repeat rather than the crisp clamped sampling a texture preview wants.
                    Texture::Ready(ui.ctx().load_texture(
                        format!("mdl:{path}"),
                        image,
                        TextureOptions {
                            magnification: egui::TextureFilter::Linear,
                            minification: egui::TextureFilter::Linear,
                            wrap_mode: egui::TextureWrapMode::Repeat,
                            mipmap_mode: Some(egui::TextureFilter::Linear),
                        },
                    ))
                }
                Err(why) => {
                    log::error!("assets/mdl: {path}: {why}");
                    Texture::Absent
                }
            };
        }
    }

    /// The model itself: an orbit camera over a paint callback.
    fn viewport(&self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return;
        }

        let level = self.level.borrow();
        let mut camera = self.camera.get();
        if response.dragged_by(egui::PointerButton::Primary) {
            let delta = response.drag_delta();
            camera.yaw -= delta.x * 0.01;
            camera.pitch = (camera.pitch + delta.y * 0.01).clamp(-1.5, 1.5);
        }
        if response.dragged_by(egui::PointerButton::Secondary) {
            let delta = response.drag_delta();
            let (sin_yaw, cos_yaw) = camera.yaw.sin_cos();
            let right = Vec3::new(cos_yaw, 0.0, -sin_yaw);
            let up = Vec3::Y;
            let scale = camera.distance * 0.002;
            camera.target += (right * -delta.x + up * delta.y) * scale;
        }
        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll != 0.0 {
                camera.distance = (camera.distance * (1.0 - scroll * 0.002))
                    .clamp(level.home.distance * 0.02, level.home.distance * 20.0);
            }
        }
        self.camera.set(camera);

        let eye = camera.eye();
        let view = Mat4::look_at_rh(eye, camera.target, Vec3::Y);
        // Cut to the model's own bounding sphere. A fixed ratio leaves a large piece with almost no
        // depth precision where it is actually drawn.
        let span = (eye - level.home.target).length();
        let near = (span - level.radius).max(level.radius * 0.005);
        let projection =
            Mat4::perspective_rh_gl(FOV, rect.width() / rect.height(), near, span + level.radius);

        // Fill and rim follow the camera; a fill weighted toward the eye is the whole of what keeps
        // a surface turned away from the key from reading as a silhouette. Both are built from the
        // camera's axes rather than from a fragment's view vector, which would give every pixel a
        // rig of its own and sweep it across the surface as the camera moves.
        let axes = Mat3::from_mat4(view).transpose();
        let (right, up, back) = (axes.x_axis, axes.y_axis, axes.z_axis);
        let fill = back - right * 0.5 - up * 0.2;
        let rim = -back * 0.55 + up * 0.6 - right * 0.55;
        let mut lights = [0.0; 9];
        for (slot, light) in lights.chunks_exact_mut(3).zip([KEY, fill, rim]) {
            slot.copy_from_slice(&light.normalize().to_array());
        }

        let slots = self.slots.borrow();
        let textures = self.textures.borrow();
        let bind = |path: Option<&String>| match path.and_then(|path| textures.get(path)) {
            Some(Texture::Ready(handle)) => Some(handle.id()),
            _ => None,
        };
        let surfaces = level
            .meshes
            .iter()
            .map(|mesh| {
                let Some(Some(Slot::Ready(material))) = slots.get(mesh.material) else {
                    return gpu::Surface {
                        material: mesh.material,
                        ..Default::default()
                    };
                };
                gpu::Surface {
                    material: mesh.material,
                    family: material.family(),
                    normal: bind(material.texture(Role::Normal)),
                    index: bind(material.texture(Role::Index)),
                    mask: bind(material.texture(Role::Mask)),
                    diffuse: bind(material.texture(Role::Diffuse)),
                    alpha_threshold: material.alpha_threshold(),
                    diffuse_color: material.diffuse(),
                    emissive_color: material.emissive(),
                    normal_scale: material.normal_scale(),
                    cull: material.cull(),
                }
            })
            .collect();

        let frame = gpu::Frame {
            view: view.to_cols_array(),
            projection: projection.to_cols_array(),
            eye: eye.to_array(),
            lights,
            surfaces,
            debug: self.debug.get(),
        };

        // The context is taken from the painter rather than captured: `glow::Context` is neither
        // `Send` nor `Sync` on wasm, and a callback has to be both.
        let model = level.gpu.clone();
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                model.lock().unwrap().draw(painter.gl(), painter, &frame);
            })),
        });
    }

    /// Draws another detail level of the same file. The materials and textures already fetched are
    /// kept, matched to the new geometry by path, so nothing is asked for twice and nothing pops.
    fn switch(&self, lod: u8) {
        let level = match read_level(&self.path, &self.bytes, lod) {
            Ok(level) => level,
            Err(why) => {
                log::error!("assets/mdl: {}: detail level {lod}: {why}", self.path);
                return;
            }
        };

        let mut slots = self.slots.borrow_mut();
        let mut held: BTreeMap<String, Slot> = self
            .level
            .borrow_mut()
            .materials
            .drain(..)
            .zip(slots.drain(..))
            .filter_map(|(path, slot)| slot.map(|slot| (path, slot)))
            .collect();
        *slots = level
            .materials
            .iter()
            .map(|path| held.remove(path))
            .collect();
        // The new level's context has no color tables of its own, and a material kept from the old
        // one never transitions again to hand one over.
        for (index, slot) in slots.iter().enumerate() {
            if let Some(Slot::Ready(material)) = slot
                && let Some(table) = material.table()
            {
                level.gpu.lock().unwrap().queue_table(index, table.to_vec());
            }
        }

        self.lod.set(lod);
        *self.level.borrow_mut() = level;
    }

    pub fn details_ui(&self, ui: &mut egui::Ui, follow: &mut Option<String>) {
        let mut picked = None;
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let level = self.level.borrow();
            facts(ui, "mdl_identity", &level.identity);
            ui.add_space(8.0);
            section(ui, "Detail");
            let lod = self.lod.get();
            ui.horizontal(|ui| {
                for (level, label) in [(0, "High"), (1, "Medium"), (2, "Low")] {
                    if ui.selectable_label(lod == level, label).clicked() && lod != level {
                        picked = Some(level);
                    }
                }
            });
            ui.add_space(8.0);
            section(ui, "Materials");
            let slots = self.slots.borrow();
            for (index, path) in level.materials.iter().enumerate() {
                if link(ui, crate::utils::file_name(path), path) {
                    *follow = Some(path.clone());
                }
                match slots.get(index).and_then(Option::as_ref) {
                    Some(Slot::Ready(material)) => {
                        ui.label(RichText::new(material.summary()).weak());
                    }
                    Some(Slot::Failed(why)) => {
                        ui.label(RichText::new(why).color(Color32::LIGHT_RED));
                    }
                    _ => {
                        ui.label(RichText::new("loading").weak());
                    }
                }
                ui.add_space(4.0);
            }
        });
        if let Some(lod) = picked {
            self.switch(lod);
        }
    }
}
