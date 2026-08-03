//! The GL side of the scene view.
//!
//! Shaped like the model viewer's: everything runs inside an [`egui_glow`] paint callback, uploads
//! happen on the first frame that draws them, and dead objects go to the same graveyard. What
//! differs is that one model is drawn many times over, so each detail level carries a buffer of
//! instance matrices the draw walks with `draw_elements_instanced`.

use std::sync::{Arc, Mutex};

use egui::TextureId;
use glow::HasContext;

use super::super::super::mdl::Vertex;
use super::super::super::mdl::gpu::{Dead, bury, graveyard};

/// Attribute locations, in the order [`Vertex`] stores them. The tangent and color the model viewer
/// reads are left unbound; nothing here shades from them.
const ATTRIBUTES: [(u32, i32, i32); 3] = [(0, 3, 0), (1, 3, 12), (3, 2, 40)];

/// Where the instance matrix starts, taking a location per column.
const MODEL: u32 = 5;

const DIFFUSE_UNIT: u32 = 0;

/// Models uploaded in one callback. Decoding is already spread over frames; this bounds the GL work
/// a single frame can be handed on top of it.
const UPLOADS: usize = 4;

const VERTEX_SOURCE: &str = include_str!("scene.vert");
const FRAGMENT_SOURCE: &str = include_str!("scene.frag");

/// One mesh's geometry.
struct Mesh {
    layout: glow::VertexArray,
    vertices: glow::Buffer,
    indices: glow::Buffer,
    count: i32,
}

/// One detail level of one model: its meshes, and the matrices they are drawn at.
struct Level {
    meshes: Vec<Mesh>,
    instances: glow::Buffer,
    /// Matrices the buffer has room for, which is every instance of the model rather than the ones
    /// currently visible, so moving the camera rewrites a buffer instead of reallocating one.
    capacity: usize,
}

struct Model {
    levels: Vec<Level>,
}

/// One model's geometry, waiting for a context to upload it under.
pub struct Pending {
    pub model: usize,
    /// Per detail level, each mesh as its vertices and indices.
    pub levels: Vec<Vec<(Vec<Vertex>, Vec<u16>)>>,
    /// Instances the model is placed at, which is what its buffers are sized for.
    pub instances: usize,
}

/// What one mesh needs beyond its geometry.
pub struct Surface {
    pub diffuse: Option<TextureId>,
    pub diffuse_color: [f32; 3],
    pub emissive_color: [f32; 3],
    pub alpha_threshold: f32,
    pub cull: bool,
    /// Set where the material said the surface is not drawn at all.
    pub hidden: bool,
}

/// One model at one detail level, and the run of its instance buffer to draw.
pub struct Batch {
    pub model: usize,
    pub level: usize,
    pub instances: i32,
    /// One per mesh of the level, in the order they were uploaded.
    pub surfaces: Vec<Surface>,
}

pub struct Frame {
    pub view_projection: [f32; 16],
    /// Where the key light stands, in world space.
    pub key: [f32; 3],
    pub horizon: [f32; 3],
    /// Where the fade into the horizon starts and ends.
    pub fade: [f32; 2],
    pub batches: Vec<Batch>,
}

pub struct Renderer {
    program: Option<glow::Program>,
    models: Vec<Option<Model>>,
    pending: Vec<Pending>,
    /// Instance matrices waiting for a context, as (model, level, matrices).
    queued: Vec<(usize, usize, Vec<f32>)>,
    failure: Option<String>,
}

impl Renderer {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            program: None,
            models: Vec::new(),
            pending: Vec::new(),
            queued: Vec::new(),
            failure: None,
        }))
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Geometry the card has not been handed yet, so the scene knows to keep asking for frames.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    pub fn queue_model(&mut self, pending: Pending) {
        self.pending.push(pending);
    }

    pub fn queue_instances(&mut self, model: usize, level: usize, matrices: Vec<f32>) {
        self.queued.push((model, level, matrices));
    }

    pub fn draw(&mut self, gl: &glow::Context, painter: &egui_glow::Painter, frame: &Frame) {
        bury(gl);
        if self.failure.is_some() {
            return;
        }
        if self.program.is_none() {
            match build(gl) {
                Ok(program) => self.program = Some(program),
                Err(why) => {
                    self.failure = Some(why);
                    return;
                }
            }
        }
        for pending in self
            .pending
            .drain(..self.pending.len().min(UPLOADS))
            .collect::<Vec<_>>()
        {
            let at = pending.model;
            match upload(gl, pending) {
                Ok(model) => {
                    if self.models.len() <= at {
                        self.models.resize_with(at + 1, || None);
                    }
                    self.models[at] = Some(model);
                }
                Err(why) => log::error!("assets/layer: model {at}: {why}"),
            }
        }
        for (model, level, matrices) in std::mem::take(&mut self.queued) {
            let Some(Some(held)) = self.models.get_mut(model) else {
                // The scene counted this model as drawn the moment it decoded, which can be several
                // frames before the upload budget reaches it. Holding the matrices rather than
                // dropping them is what keeps it from drawing against an unwritten buffer.
                if self.pending.iter().any(|pending| pending.model == model) {
                    self.queued.push((model, level, matrices));
                }
                continue;
            };
            let Some(level) = held.levels.get_mut(level) else {
                continue;
            };
            unsafe {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(level.instances));
                let wanted = matrices.len() / 16;
                if wanted > level.capacity {
                    level.capacity = wanted;
                    gl.buffer_data_size(
                        glow::ARRAY_BUFFER,
                        (wanted * 16 * size_of::<f32>()) as i32,
                        glow::DYNAMIC_DRAW,
                    );
                }
                gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytemuck::cast_slice(&matrices));
                gl.bind_buffer(glow::ARRAY_BUFFER, None);
            }
        }

        let Some(program) = self.program else {
            return;
        };
        unsafe {
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.depth_mask(true);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::BLEND);
            gl.use_program(Some(program));

            let view_projection = gl.get_uniform_location(program, "u_view_projection");
            gl.uniform_matrix_4_f32_slice(view_projection.as_ref(), false, &frame.view_projection);
            let key = gl.get_uniform_location(program, "u_key");
            gl.uniform_3_f32_slice(key.as_ref(), &frame.key);
            let horizon = gl.get_uniform_location(program, "u_horizon");
            gl.uniform_3_f32_slice(horizon.as_ref(), &frame.horizon);
            let fade = gl.get_uniform_location(program, "u_fade");
            gl.uniform_2_f32_slice(fade.as_ref(), &frame.fade);
            let map = gl.get_uniform_location(program, "u_diffuse_map");
            gl.uniform_1_i32(map.as_ref(), DIFFUSE_UNIT as i32);

            let have = gl.get_uniform_location(program, "u_have_diffuse");
            let threshold = gl.get_uniform_location(program, "u_alpha_threshold");
            let diffuse = gl.get_uniform_location(program, "u_diffuse_color");
            let emissive = gl.get_uniform_location(program, "u_emissive_color");

            for batch in &frame.batches {
                if batch.instances <= 0 {
                    continue;
                }
                let Some(Some(model)) = self.models.get(batch.model) else {
                    continue;
                };
                let Some(level) = model.levels.get(batch.level) else {
                    continue;
                };
                for (mesh, surface) in level.meshes.iter().zip(&batch.surfaces) {
                    if surface.hidden {
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
                    let texture = surface.diffuse.and_then(|id| painter.texture(id));
                    gl.active_texture(glow::TEXTURE0 + DIFFUSE_UNIT);
                    gl.bind_texture(glow::TEXTURE_2D, texture);
                    gl.uniform_1_i32(have.as_ref(), i32::from(texture.is_some()));
                    gl.uniform_1_f32(threshold.as_ref(), surface.alpha_threshold);
                    gl.uniform_3_f32_slice(diffuse.as_ref(), &surface.diffuse_color);
                    gl.uniform_3_f32_slice(emissive.as_ref(), &surface.emissive_color);

                    gl.bind_vertex_array(Some(mesh.layout));
                    gl.draw_elements_instanced(
                        glow::TRIANGLES,
                        mesh.count,
                        glow::UNSIGNED_SHORT,
                        0,
                        batch.instances,
                    );
                }
            }

            gl.bind_vertex_array(None);
            gl.depth_mask(false);
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        graveyard().lock().unwrap().extend(
            self.models
                .drain(..)
                .flatten()
                .flat_map(|model| model.levels)
                .flat_map(|level| {
                    level
                        .meshes
                        .into_iter()
                        .flat_map(|mesh| {
                            [
                                Dead::Layout(mesh.layout),
                                Dead::Buffer(mesh.vertices),
                                Dead::Buffer(mesh.indices),
                            ]
                        })
                        .chain([Dead::Buffer(level.instances)])
                })
                .chain(self.program.take().map(Dead::Program)),
        );
    }
}

fn upload(gl: &glow::Context, pending: Pending) -> Result<Model, String> {
    let mut levels = Vec::new();
    for meshes in pending.levels {
        let instances = unsafe {
            let buffer = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            gl.buffer_data_size(
                glow::ARRAY_BUFFER,
                (pending.instances.max(1) * 16 * size_of::<f32>()) as i32,
                glow::DYNAMIC_DRAW,
            );
            buffer
        };
        let mut built = Vec::new();
        for (vertices, indices) in &meshes {
            built.push(upload_mesh(gl, vertices, indices, instances)?);
        }
        levels.push(Level {
            meshes: built,
            instances,
            capacity: pending.instances.max(1),
        });
    }
    unsafe { gl.bind_buffer(glow::ARRAY_BUFFER, None) };
    Ok(Model { levels })
}

/// One mesh's buffers, with its own vertex array. The instance buffer is bound into that array
/// rather than at draw time, so the divisors are set once.
fn upload_mesh(
    gl: &glow::Context,
    vertices: &[Vertex],
    indices: &[u16],
    instances: glow::Buffer,
) -> Result<Mesh, String> {
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

        gl.bind_buffer(glow::ARRAY_BUFFER, Some(instances));
        let row = 4 * size_of::<f32>() as i32;
        for column in 0..4 {
            let location = MODEL + column as u32;
            gl.enable_vertex_attrib_array(location);
            gl.vertex_attrib_pointer_f32(location, 4, glow::FLOAT, false, 4 * row, column * row);
            gl.vertex_attrib_divisor(location, 1);
        }

        let drawn = gl.create_buffer()?;
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(drawn));
        gl.buffer_data_u8_slice(
            glow::ELEMENT_ARRAY_BUFFER,
            bytemuck::cast_slice(indices),
            glow::STATIC_DRAW,
        );

        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        Ok(Mesh {
            layout,
            vertices: held,
            indices: drawn,
            count: indices.len() as i32,
        })
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
