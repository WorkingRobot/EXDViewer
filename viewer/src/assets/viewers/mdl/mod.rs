//! `.mdl` models, drawn.
//!
//! Geometry comes off the file when it is decoded; the materials it names are fetched afterwards and
//! land on meshes already on screen, so a model shows as untextured geometry first and dresses
//! itself as its textures arrive.
//!
//! The shading approximates the game's rather than reproducing it: a color table row is picked the
//! way the game picks one and drives a diffuse color, a specular color and a specular exponent, the
//! mask map scales all three, and everything is lit by three lights that follow the camera instead
//! of by the scene's. Skinning, dyes and decals are all absent, so a character stands in bind pose.
//!
//! Shape keys are applied by rewriting the indices they name, which is what the file states rather
//! than a blend, so a shape is either on or off.

pub(super) mod gpu;
pub(super) mod material;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
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

/// One part of a mesh, drawn with the rest of it but hideable on its own.
struct Part {
    range: Range<usize>,
    shown: bool,
    /// What the model's attribute table calls this part, which is the only name it carries. Empty
    /// where the part claims no attribute.
    attributes: String,
}

/// One mesh of the model, as far as the browser cares about it.
struct Mesh {
    material: usize,
    vertices: usize,
    triangles: usize,
    /// The runs of indices the file splits the mesh into, and whether each draws. A mesh the file
    /// does not split holds the one run covering all of them.
    parts: Vec<Part>,
    /// The mesh's indices as the file lists them, kept only where the model has a shape key that
    /// could rewrite them, since applying one is a rewrite of these rather than of what is on the
    /// card.
    base: Vec<u16>,
}

/// One shape key, and where it rewrites the geometry.
struct Shape {
    name: String,
    /// Which of the level's meshes the shape touches, and for each the indices it replaces.
    rewrites: Vec<(usize, Vec<(u16, u16)>)>,
}

/// Shape keys the file names as variants of one thing, which the browser offers as alternatives
/// rather than as switches that stack. A name carrying no variant stands in a group of its own.
struct Group {
    /// The file's own abbreviation, left as it writes it. Empty for a shape standing alone.
    category: String,
    /// Positions in [`Level::shapes`], each with the variant its name ends in.
    variants: Vec<(usize, String)>,
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
    /// Shape keys reaching this detail level, in the order the file declares them.
    shapes: Vec<Shape>,
    /// The same shapes, gathered by the category their names share.
    groups: Vec<Group>,
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
    /// Which detail levels the file draws anything at.
    drawn: [bool; 3],
    level: RefCell<Level>,
    /// Shape keys the user has switched on, by name: a detail level built later carries its own
    /// shapes, and the names are what survives the switch.
    shapes: RefCell<BTreeSet<String>>,
    slots: RefCell<Vec<Option<Slot>>>,
    textures: RefCell<BTreeMap<String, Texture>>,
    camera: Cell<Camera>,
    /// Decoded texture bytes handed to egui so far.
    resident: Cell<usize>,
    debug: Cell<gpu::Debug>,
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let container = ModelContainer::read(Cursor::new(bytes.to_vec()))?;
    let level = read_level(path, &container, 0)?;
    let camera = level.home;
    Ok(Preview::Model(Box::new(Rendered {
        path: path.to_owned(),
        bytes: bytes.to_vec(),
        lod: Cell::new(0),
        drawn: std::array::from_fn(|lod| {
            container
                .model(detail(lod as u8))
                .meshes()
                .iter()
                .any(|mesh| mesh.kinds().contains(&MeshKind::Standard))
        }),
        slots: RefCell::new((0..level.materials.len()).map(|_| None).collect()),
        shapes: Default::default(),
        level: RefCell::new(level),
        textures: Default::default(),
        camera: Cell::new(camera),
        resident: Cell::new(0),
        debug: Cell::new(gpu::Debug::None),
    })))
}

pub(super) fn detail(lod: u8) -> Lod {
    match lod {
        0 => Lod::High,
        1 => Lod::Medium,
        _ => Lod::Low,
    }
}

fn read_level(path: &str, container: &ModelContainer, lod: u8) -> Result<Level> {
    let model = container.model(detail(lod));

    let mut names: Vec<String> = Vec::new();
    let mut meshes = Vec::new();
    let mut unreadable = Vec::new();
    let mut pending = gpu::Pending::default();
    let mut low = Vec3::splat(f32::INFINITY);
    let mut high = Vec3::splat(f32::NEG_INFINITY);

    let attributes = model.attribute_names().unwrap_or_default();
    let declared = model.shapes();
    let mut rewrites: Vec<Vec<(usize, Vec<(u16, u16)>)>> =
        declared.iter().map(|_| Vec::new()).collect();

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
        let submeshes = mesh.submeshes();
        let parts = match submeshes.is_empty() {
            true => vec![Part {
                range: 0..indices.len(),
                shown: true,
                attributes: String::new(),
            }],
            false => submeshes
                .iter()
                .map(|part| Part {
                    range: part.start..part.start + part.count,
                    shown: true,
                    attributes: named(&attributes, part.attributes),
                })
                .collect(),
        };
        for (shape, touched) in declared.iter().zip(&mut rewrites) {
            let values = shape.rewrites(&mesh);
            if !values.is_empty() {
                touched.push((meshes.len(), values));
            }
        }
        meshes.push(Mesh {
            material,
            vertices: vertices.len(),
            triangles: indices.len() / 3,
            parts,
            base: match declared.is_empty() {
                true => Vec::new(),
                false => indices.clone(),
            },
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

    let shapes = declared
        .iter()
        .zip(rewrites)
        .filter(|(_, touched)| !touched.is_empty())
        .map(|(shape, touched)| Shape {
            name: shape.name().unwrap_or_default(),
            rewrites: touched,
        })
        .collect::<Vec<_>>();

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
        groups: group(&shapes),
        meshes,
        shapes,
        materials: names,
        unreadable,
        home,
        radius,
        gpu: gpu::Model::new(pending),
    })
}

/// Interleaves the attributes a mesh declares into the one buffer the shader reads. A mesh missing
/// a normal, tangent, UV or color gets a default rather than being dropped.
pub(super) fn build(
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

/// Shapes gathered by category, in the order the file declares them. A name is read as
/// `shp_<category>_<variant>`; most carry no variant, and each of those stands alone.
fn group(shapes: &[Shape]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for (at, shape) in shapes.iter().enumerate() {
        let (category, variant) = match shape
            .name
            .strip_prefix("shp_")
            .and_then(|rest| rest.rsplit_once('_'))
        {
            Some((category, variant)) => (category.to_owned(), variant.to_owned()),
            None => (String::new(), shape.name.clone()),
        };
        match groups
            .iter_mut()
            .find(|group| !group.category.is_empty() && group.category == category)
        {
            Some(group) => group.variants.push((at, variant)),
            None => groups.push(Group {
                category,
                variants: vec![(at, variant)],
            }),
        }
    }
    groups
}

/// What the model's attribute table calls the bits a part sets. The mask is 32 bits wide however
/// many names the table holds.
fn named(attributes: &[String], mask: u32) -> String {
    attributes
        .iter()
        .take(32)
        .enumerate()
        .filter(|(bit, _)| mask & (1 << bit) != 0)
        .map(|(_, name)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The parts still showing, as the fewest runs that cover them. A file lists a mesh's parts in
/// index order, so two neighbours that both draw are one call rather than two.
fn shown(parts: &[Part]) -> Vec<Range<i32>> {
    let mut runs: Vec<Range<i32>> = Vec::new();
    for part in parts.iter().filter(|part| part.shown) {
        let run = part.range.start as i32..part.range.end as i32;
        match runs.last_mut() {
            Some(last) if last.end == run.start => last.end = run.end,
            _ => runs.push(run),
        }
    }
    runs
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
                    // Taken as premultiplied, which is the one path that copies the bytes through
                    // untouched. These are looked up channel by channel rather than composited, and
                    // a normal or mask map carrying anything but opacity in its alpha has its other
                    // three channels scaled away by the unmultiplied path.
                    let image = egui::ColorImage::from_rgba_premultiplied(
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
        let pan = |camera: &mut Camera, delta: egui::Vec2| {
            let (sin_yaw, cos_yaw) = camera.yaw.sin_cos();
            let right = Vec3::new(cos_yaw, 0.0, -sin_yaw);
            let scale = camera.distance * 0.002;
            camera.target += (right * -delta.x + Vec3::Y * delta.y) * scale;
        };
        let zoom = |camera: &mut Camera, scale: f32| {
            camera.distance = (camera.distance * scale)
                .clamp(level.home.distance * 0.02, level.home.distance * 20.0);
        };

        // A second finger takes the gesture over: egui carries on reporting a primary drag through
        // one, so leaving the orbit armed would spin the model while it is being pinched.
        let touch = ui.input(|input| input.multi_touch());
        match touch.filter(|_| response.dragged()) {
            Some(touch) => {
                zoom(&mut camera, 1.0 / touch.zoom_delta);
                pan(&mut camera, touch.translation_delta);
            }
            None => {
                if response.dragged_by(egui::PointerButton::Primary) {
                    let delta = response.drag_delta();
                    camera.yaw -= delta.x * 0.01;
                    camera.pitch = (camera.pitch + delta.y * 0.01).clamp(-1.5, 1.5);
                }
                if response.dragged_by(egui::PointerButton::Secondary) {
                    pan(&mut camera, response.drag_delta());
                }
            }
        }
        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll != 0.0 {
                zoom(&mut camera, 1.0 - scroll * 0.002);
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
                let runs = shown(&mesh.parts);
                let Some(Some(Slot::Ready(material))) = slots.get(mesh.material) else {
                    return gpu::Surface {
                        material: mesh.material,
                        runs,
                        ..Default::default()
                    };
                };
                if !material.drawn() {
                    return gpu::Surface {
                        material: mesh.material,
                        ..Default::default()
                    };
                }
                gpu::Surface {
                    material: mesh.material,
                    runs,
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

    /// Rewrites every touched mesh's indices from the file's own, so switching a shape off restores
    /// what it replaced and two shapes over the same mesh both land.
    fn apply(&self) {
        let level = self.level.borrow();
        let enabled = self.shapes.borrow();
        let mut rewritten: BTreeMap<usize, Vec<u16>> = BTreeMap::new();
        for shape in level
            .shapes
            .iter()
            .filter(|shape| enabled.contains(&shape.name))
        {
            for (mesh, values) in &shape.rewrites {
                let indices = rewritten
                    .entry(*mesh)
                    .or_insert_with(|| level.meshes[*mesh].base.clone());
                for (offset, vertex) in values {
                    if let Some(held) = indices.get_mut(usize::from(*offset)) {
                        *held = *vertex;
                    }
                }
            }
        }
        // A mesh a shape has just stopped touching still holds that shape's indices, so every mesh
        // any shape reaches is uploaded rather than only the ones still rewritten.
        let mut gpu = level.gpu.lock().unwrap();
        for mesh in level
            .shapes
            .iter()
            .flat_map(|shape| &shape.rewrites)
            .map(|(mesh, _)| *mesh)
            .collect::<BTreeSet<_>>()
        {
            let indices = rewritten
                .remove(&mesh)
                .unwrap_or_else(|| level.meshes[mesh].base.clone());
            gpu.queue_indices(mesh, indices);
        }
    }

    /// Draws another detail level of the same file. The materials and textures already fetched are
    /// kept, matched to the new geometry by path, so nothing is asked for twice and nothing pops.
    fn switch(&self, lod: u8) {
        let read = ModelContainer::read(Cursor::new(self.bytes.clone()))
            .map_err(anyhow::Error::from)
            .and_then(|container| read_level(&self.path, &container, lod));
        let level = match read {
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
        self.apply();
    }

    pub fn details_ui(&self, ui: &mut egui::Ui, follow: &mut Option<String>) {
        let mut picked = None;
        let mut toggled = None;
        let mut picked_shape = None;
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let level = self.level.borrow();
            facts(ui, "mdl_identity", &level.identity);
            // A file drawing at one detail level has nothing to pick between.
            if self.drawn.iter().filter(|drawn| **drawn).count() > 1 {
                ui.add_space(8.0);
                section(ui, "Detail");
                let lod = self.lod.get();
                ui.horizontal(|ui| {
                    for (level, label) in [(0, "High"), (1, "Medium"), (2, "Low")] {
                        let picker = ui.add_enabled(
                            self.drawn[usize::from(level)],
                            egui::Button::selectable(lod == level, label),
                        );
                        if picker.clicked() && lod != level {
                            picked = Some(level);
                        }
                    }
                });
            }
            if !level.shapes.is_empty() {
                ui.add_space(8.0);
                section(ui, "Shapes");
                let enabled = self.shapes.borrow();
                let on = |at: usize| enabled.contains(&level.shapes[at].name);
                let hover = |at: usize| {
                    let shape = &level.shapes[at];
                    format!("{}\n{} meshes rewritten", shape.name, shape.rewrites.len())
                };
                // Clicking the variant already showing is what turns its category off, so a
                // category needs no entry of its own for having nothing applied.
                let chip = |ui: &mut egui::Ui, at: usize, label: &str| {
                    ui.selectable_label(on(at), label)
                        .on_hover_text(hover(at))
                        .clicked()
                };
                for (index, group) in level.groups.iter().enumerate() {
                    if group.category.is_empty() {
                        continue;
                    }
                    ui.label(RichText::new(&group.category).weak());
                    ui.horizontal_wrapped(|ui| {
                        for (at, variant) in &group.variants {
                            if chip(ui, *at, variant) {
                                picked_shape = Some((index, (!on(*at)).then_some(*at)));
                            }
                        }
                    });
                }
                // Whatever the file names without a variant, which is most of what a model
                // deforms. Each stands on its own, so they share one row rather than taking a
                // heading each.
                if level.groups.iter().any(|group| group.category.is_empty()) {
                    ui.horizontal_wrapped(|ui| {
                        for (index, group) in level.groups.iter().enumerate() {
                            if !group.category.is_empty() {
                                continue;
                            }
                            let (at, name) = &group.variants[0];
                            if chip(ui, *at, name) {
                                picked_shape = Some((index, (!on(*at)).then_some(*at)));
                            }
                        }
                    });
                }
            }

            ui.add_space(8.0);
            section(ui, "Meshes");
            for (index, mesh) in level.meshes.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    let drawn = mesh.parts.iter().any(|part| part.shown);
                    if ui
                        .selectable_label(drawn, RichText::new(format!("Mesh {index}")).weak())
                        .on_hover_text(format!(
                            "{}\n{} triangles",
                            crate::utils::file_name(&level.materials[mesh.material]),
                            mesh.triangles
                        ))
                        .clicked()
                    {
                        toggled = Some((index, None));
                    }
                    for (part, held) in mesh.parts.iter().enumerate() {
                        let label = match held.attributes.is_empty() {
                            true => part.to_string(),
                            false => held.attributes.clone(),
                        };
                        if ui.selectable_label(held.shown, label).clicked() {
                            toggled = Some((index, Some(part)));
                        }
                    }
                });
            }
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
        if let Some((mesh, part)) = toggled {
            let mut level = self.level.borrow_mut();
            let parts = &mut level.meshes[mesh].parts;
            match part {
                Some(part) => parts[part].shown = !parts[part].shown,
                None => {
                    let hide = parts.iter().any(|part| part.shown);
                    for part in parts.iter_mut() {
                        part.shown = !hide;
                    }
                }
            }
        }
        if let Some((group, variant)) = picked_shape {
            {
                let level = self.level.borrow();
                let mut enabled = self.shapes.borrow_mut();
                for (at, _) in &level.groups[group].variants {
                    enabled.remove(&level.shapes[*at].name);
                }
                if let Some(at) = variant {
                    enabled.insert(level.shapes[at].name.clone());
                }
            }
            self.apply();
        }
        if let Some(lod) = picked {
            self.switch(lod);
        }
    }
}
