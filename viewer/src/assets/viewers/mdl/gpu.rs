//! The GL side of the model viewer.
//!
//! Everything here runs inside an [`egui_glow`] paint callback, which is the only place a
//! `glow::Context` is reachable: the context is neither `Send` nor `Sync` on wasm, so it cannot be
//! captured, and eframe's copy of it is not threaded down to a viewer. Uploads therefore happen on
//! the first frame that draws rather than when the file is decoded, and freeing happens in a
//! graveyard the next callback drains.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

use egui::TextureId;
use glow::HasContext;

use super::Vertex;
use super::material::Family;

/// Attribute locations, in the order [`Vertex`] stores them.
const ATTRIBUTES: [(u32, i32, i32); 4] = [(0, 3, 0), (1, 3, 12), (2, 4, 24), (3, 2, 40)];
const COLOR: u32 = 4;
const COLOR_OFFSET: i32 = 48;

/// Texture units, in the order the shader's samplers declare them.
const NORMAL_UNIT: u32 = 0;
const INDEX_UNIT: u32 = 1;
const MASK_UNIT: u32 = 2;
const DIFFUSE_UNIT: u32 = 3;
const TABLE_UNIT: u32 = 4;

/// Texels per color-table row. This viewer's own packing, not the game's.
pub const TABLE_COLUMNS: i32 = 4;

const VERTEX_SOURCE: &str = include_str!("model.vert");
const FRAGMENT_SOURCE: &str = include_str!("model.frag");

/// A mesh's geometry, once it is on the card.
struct Buffers {
    layout: glow::VertexArray,
    vertices: glow::Buffer,
    indices: glow::Buffer,
}

/// GL objects with nothing left to draw them, waiting for a context to delete them under. A viewer
/// is dropped between frames, where there is no context, so its objects outlive it by one callback.
static GRAVEYARD: OnceLock<Mutex<Vec<Dead>>> = OnceLock::new();

pub enum Dead {
    Layout(glow::VertexArray),
    Buffer(glow::Buffer),
    Texture(glow::Texture),
    Program(glow::Program),
}

pub fn graveyard() -> &'static Mutex<Vec<Dead>> {
    GRAVEYARD.get_or_init(Default::default)
}

/// Deletes what an earlier viewer left behind. Called at the top of a draw, because that is the
/// only moment a context exists.
pub fn bury(gl: &glow::Context) {
    for dead in graveyard().lock().unwrap().drain(..) {
        unsafe {
            match dead {
                Dead::Layout(layout) => gl.delete_vertex_array(layout),
                Dead::Buffer(buffer) => gl.delete_buffer(buffer),
                Dead::Texture(texture) => gl.delete_texture(texture),
                Dead::Program(program) => gl.delete_program(program),
            }
        }
    }
}

/// What one draw call needs beyond its geometry: the material it uses, and the egui textures that
/// material resolved to.
pub struct Surface {
    pub material: usize,
    /// Which of the mesh's indices to draw, so a hidden part costs no triangles.
    pub runs: Vec<Range<i32>>,
    pub family: Family,
    pub normal: Option<TextureId>,
    pub index: Option<TextureId>,
    pub mask: Option<TextureId>,
    pub diffuse: Option<TextureId>,
    pub alpha_threshold: f32,
    pub diffuse_color: [f32; 3],
    pub emissive_color: [f32; 3],
    pub normal_scale: f32,
    pub cull: bool,
}

/// What a mesh draws as while its material is still being fetched: bare geometry, nothing tinted
/// away and nothing clipped.
impl Default for Surface {
    fn default() -> Self {
        Self {
            material: 0,
            runs: Vec::new(),
            family: Family::Background,
            normal: None,
            index: None,
            mask: None,
            diffuse: None,
            alpha_threshold: 0.0,
            diffuse_color: [1.0; 3],
            emissive_color: [0.0; 3],
            normal_scale: 1.0,
            cull: false,
        }
    }
}

/// What the shader draws instead of a shaded surface. Discriminants are the values `model.frag`
/// compares `u_debug` against.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Debug {
    None = 0,
    Normals = 1,
    Uv = 2,
    Geometry = 3,
    Tangents = 4,
    Bitangents = 5,
    Handedness = 6,
    Color = 7,
    Alpha = 8,
    Meshes = 9,
}

/// One frame of camera and material bindings, rebuilt every time the widget draws.
pub struct Frame {
    pub view: [f32; 16],
    pub projection: [f32; 16],
    pub eye: [f32; 3],
    /// Key, fill and rim directions, in world space. Built once a frame from the camera, so a
    /// surface is lit by one set of lights rather than by a set of its own.
    pub lights: [f32; 9],
    pub surfaces: Vec<Surface>,
    pub debug: Debug,
}

/// Geometry waiting for a context to upload it under.
#[derive(Default)]
pub struct Pending {
    pub meshes: Vec<(Vec<Vertex>, Vec<u16>)>,
}

/// Everything the callback owns, shared with the viewer that built it.
pub struct Model {
    pending: Option<Pending>,
    program: Option<glow::Program>,
    meshes: Vec<Buffers>,
    /// Color tables arrive with their materials, which is long after the geometry, so they queue
    /// rather than travelling with it.
    queued: Vec<(usize, Vec<f32>)>,
    /// Meshes whose indices a shape key rewrote, waiting for a context to upload them under.
    rewritten: Vec<(usize, Vec<u16>)>,
    tables: BTreeMap<usize, (glow::Texture, f32)>,
    /// Why the shader would not build, kept so the viewer can say so rather than draw nothing.
    failure: Option<String>,
}

impl Model {
    pub fn new(pending: Pending) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            pending: Some(pending),
            program: None,
            meshes: Vec::new(),
            queued: Vec::new(),
            rewritten: Vec::new(),
            tables: BTreeMap::new(),
            failure: None,
        }))
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Hands a material's color table over for the next draw to upload.
    pub fn queue_table(&mut self, material: usize, values: Vec<f32>) {
        self.queued.push((material, values));
    }

    /// Hands a mesh's indices over for the next draw to upload, replacing the ones it holds.
    pub fn queue_indices(&mut self, mesh: usize, indices: Vec<u16>) {
        self.rewritten.push((mesh, indices));
    }

    pub fn draw(&mut self, gl: &glow::Context, painter: &egui_glow::Painter, frame: &Frame) {
        bury(gl);
        if self.failure.is_some() {
            return;
        }
        if let Some(pending) = self.pending.take()
            && let Err(why) = self.upload(gl, pending)
        {
            self.failure = Some(why);
            return;
        }
        for (mesh, indices) in std::mem::take(&mut self.rewritten) {
            let Some(buffers) = self.meshes.get(mesh) else {
                continue;
            };
            // Through the mesh's own vertex array, since binding an element buffer rewrites
            // whichever array is current, and egui leaves its own bound around a callback.
            unsafe {
                gl.bind_vertex_array(Some(buffers.layout));
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(buffers.indices));
                gl.buffer_data_u8_slice(
                    glow::ELEMENT_ARRAY_BUFFER,
                    bytemuck::cast_slice(&indices),
                    glow::STATIC_DRAW,
                );
                gl.bind_vertex_array(None);
            }
        }
        for (material, values) in std::mem::take(&mut self.queued) {
            let rows = values.len() as i32 / (TABLE_COLUMNS * 4);
            match upload_table(gl, &values, rows) {
                Ok(texture) => {
                    self.tables.insert(material, (texture, rows as f32));
                }
                Err(why) => log::error!("assets/mdl: color table: {why}"),
            }
        }
        let Some(program) = self.program else {
            return;
        };
        // A zip would truncate instead, and a mesh drawn under another mesh's material shows as a
        // texturing bug rather than as the bookkeeping error it is.
        if self.meshes.len() != frame.surfaces.len() {
            self.failure = Some(format!(
                "{} meshes against {} surfaces",
                self.meshes.len(),
                frame.surfaces.len()
            ));
            return;
        }

        unsafe {
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.depth_mask(true);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::BLEND);
            gl.use_program(Some(program));

            let view = gl.get_uniform_location(program, "u_view");
            gl.uniform_matrix_4_f32_slice(view.as_ref(), false, &frame.view);
            let projection = gl.get_uniform_location(program, "u_projection");
            gl.uniform_matrix_4_f32_slice(projection.as_ref(), false, &frame.projection);
            let eye = gl.get_uniform_location(program, "u_eye");
            gl.uniform_3_f32_slice(eye.as_ref(), &frame.eye);
            let lights = gl.get_uniform_location(program, "u_lights[0]");
            gl.uniform_3_f32_slice(lights.as_ref(), &frame.lights);
            for (name, unit) in [
                ("u_normal_map", NORMAL_UNIT),
                ("u_index_map", INDEX_UNIT),
                ("u_mask_map", MASK_UNIT),
                ("u_diffuse_map", DIFFUSE_UNIT),
                ("u_table", TABLE_UNIT),
            ] {
                let slot = gl.get_uniform_location(program, name);
                gl.uniform_1_i32(slot.as_ref(), unit as i32);
            }
            let debug = gl.get_uniform_location(program, "u_debug");
            gl.uniform_1_i32(debug.as_ref(), frame.debug as i32);
            let have = gl.get_uniform_location(program, "u_have");
            let family = gl.get_uniform_location(program, "u_family");
            let mesh = gl.get_uniform_location(program, "u_mesh");
            let threshold = gl.get_uniform_location(program, "u_alpha_threshold");
            let rows = gl.get_uniform_location(program, "u_table_rows");
            let diffuse = gl.get_uniform_location(program, "u_diffuse_color");
            let emissive = gl.get_uniform_location(program, "u_emissive_color");
            let scale = gl.get_uniform_location(program, "u_normal_scale");

            for (at, (buffers, surface)) in self.meshes.iter().zip(&frame.surfaces).enumerate() {
                if surface.runs.is_empty() {
                    continue;
                }
                match surface.cull {
                    true => {
                        gl.enable(glow::CULL_FACE);
                        gl.cull_face(glow::BACK);
                        gl.front_face(glow::CCW);
                    }
                    false => gl.disable(glow::CULL_FACE),
                }

                let table = self.tables.get(&surface.material).copied();
                let mut bound = 0;
                for (unit, id) in [
                    (NORMAL_UNIT, surface.normal),
                    (INDEX_UNIT, surface.index),
                    (MASK_UNIT, surface.mask),
                    (DIFFUSE_UNIT, surface.diffuse),
                ] {
                    let texture = id.and_then(|id| painter.texture(id));
                    gl.active_texture(glow::TEXTURE0 + unit);
                    gl.bind_texture(glow::TEXTURE_2D, texture);
                    bound |= i32::from(texture.is_some()) << unit;
                }
                gl.active_texture(glow::TEXTURE0 + TABLE_UNIT);
                gl.bind_texture(glow::TEXTURE_2D, table.map(|(texture, _)| texture));
                bound |= i32::from(table.is_some()) << TABLE_UNIT;

                gl.uniform_1_i32(have.as_ref(), bound);
                gl.uniform_1_i32(family.as_ref(), surface.family as i32);
                gl.uniform_1_i32(mesh.as_ref(), at as i32);
                gl.uniform_1_f32(threshold.as_ref(), surface.alpha_threshold);
                gl.uniform_1_f32(rows.as_ref(), table.map_or(0.0, |(_, rows)| rows));
                gl.uniform_3_f32_slice(diffuse.as_ref(), &surface.diffuse_color);
                gl.uniform_3_f32_slice(emissive.as_ref(), &surface.emissive_color);
                gl.uniform_1_f32(scale.as_ref(), surface.normal_scale);

                gl.bind_vertex_array(Some(buffers.layout));
                for run in &surface.runs {
                    let offset = run.start * size_of::<u16>() as i32;
                    gl.draw_elements(
                        glow::TRIANGLES,
                        run.end - run.start,
                        glow::UNSIGNED_SHORT,
                        offset,
                    );
                }
            }

            gl.bind_vertex_array(None);
            gl.depth_mask(false);
        }
    }

    fn upload(&mut self, gl: &glow::Context, pending: Pending) -> Result<(), String> {
        // `antialias` on the canvas is a hint the implementation may ignore, and nothing short of a
        // live context says whether it did.
        let samples = unsafe { gl.get_parameter_i32(glow::SAMPLES) };
        let depth = unsafe { gl.get_parameter_i32(glow::DEPTH_BITS) };
        log::info!(
            "assets/mdl: {} meshes on {:?}, {samples} samples, {depth} depth bits",
            pending.meshes.len(),
            gl.version()
        );
        self.program = Some(build(gl)?);
        for (vertices, indices) in &pending.meshes {
            self.meshes.push(upload_mesh(gl, vertices, indices)?);
        }
        Ok(())
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        graveyard().lock().unwrap().extend(
            self.meshes
                .drain(..)
                .flat_map(|held| {
                    [
                        Dead::Layout(held.layout),
                        Dead::Buffer(held.vertices),
                        Dead::Buffer(held.indices),
                    ]
                })
                .chain(
                    std::mem::take(&mut self.tables)
                        .into_values()
                        .map(|(texture, _)| Dead::Texture(texture)),
                )
                .chain(self.program.take().map(Dead::Program)),
        );
    }
}

/// One mesh's buffers, with the attribute layout captured in a vertex array of its own.
///
/// The array is not an optimisation. egui leaves its own vertex array bound while a callback runs,
/// so setting attribute pointers without one would rewrite egui's layout to point at model
/// geometry, and every widget drawn afterwards would read vertices out of this mesh.
fn upload_mesh(
    gl: &glow::Context,
    vertices: &[Vertex],
    indices: &[u16],
) -> Result<Buffers, String> {
    unsafe {
        let layout = gl.create_vertex_array()?;
        gl.bind_vertex_array(Some(layout));

        let held = gl.create_buffer()?;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(held));
        gl.buffer_data_u8_slice(
            glow::ARRAY_BUFFER,
            bytemuck::cast_slice(vertices),
            glow::STATIC_DRAW,
        );

        let stride = size_of::<Vertex>() as i32;
        for (location, size, offset) in ATTRIBUTES {
            gl.enable_vertex_attrib_array(location);
            gl.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, stride, offset);
        }
        gl.enable_vertex_attrib_array(COLOR);
        gl.vertex_attrib_pointer_f32(COLOR, 4, glow::UNSIGNED_BYTE, true, stride, COLOR_OFFSET);

        let drawn = gl.create_buffer()?;
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(drawn));
        gl.buffer_data_u8_slice(
            glow::ELEMENT_ARRAY_BUFFER,
            bytemuck::cast_slice(indices),
            glow::STATIC_DRAW,
        );

        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        Ok(Buffers {
            layout,
            vertices: held,
            indices: drawn,
        })
    }
}

/// The color table, one RGBA texel per field group. Point sampled: the row pair is mixed in the
/// shader rather than by the sampler, so a row's own values stay exact.
fn upload_table(gl: &glow::Context, values: &[f32], rows: i32) -> Result<glow::Texture, String> {
    unsafe {
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA16F as i32,
            TABLE_COLUMNS,
            rows,
            0,
            glow::RGBA,
            glow::FLOAT,
            glow::PixelUnpackData::Slice(Some(bytemuck::cast_slice(values))),
        );
        for (name, value) in [
            (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
            (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
        }
        Ok(texture)
    }
}

fn build(gl: &glow::Context) -> Result<glow::Program, String> {
    unsafe {
        let program = gl.create_program()?;
        let mut built = Vec::new();
        for (stage, source) in [
            (glow::VERTEX_SHADER, VERTEX_SOURCE),
            (glow::FRAGMENT_SHADER, FRAGMENT_SOURCE),
        ] {
            let shader = gl.create_shader(stage)?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let why = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                for shader in built {
                    gl.delete_shader(shader);
                }
                gl.delete_program(program);
                return Err(why);
            }
            gl.attach_shader(program, shader);
            built.push(shader);
        }
        gl.link_program(program);
        for shader in built {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(program) {
            let why = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(why);
        }
        Ok(program)
    }
}
