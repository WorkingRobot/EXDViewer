//! The GL side of the effect viewer.
//!
//! Shaped like the model viewer's: everything runs inside an [`egui_glow`] paint callback, uploads
//! happen on the first frame that draws them, and dead objects go to the same graveyard. What
//! differs is that the geometry is one unit quad and whatever models the effect carries, each drawn
//! once per live particle out of a single instance buffer the whole frame is written into.

use std::sync::{Arc, Mutex};

use egui::TextureId;
use glow::HasContext;

use super::super::mdl::gpu::{Dead, bury, graveyard};
use super::sim::{Blend, Mesh, Shape, Vertex};

/// Attribute locations, in the order [`Vertex`] stores them.
const ATTRIBUTES: [(u32, i32, i32); 2] = [(0, 3, 0), (1, 2, 12)];
const COLOR: u32 = 2;
const COLOR_OFFSET: i32 = 20;

/// Where the per-instance attributes start, and what each takes.
const INSTANCE: [(u32, i32, i32); 4] = [(3, 3, 0), (4, 3, 12), (5, 4, 24), (6, 4, 40)];

const VERTEX_SOURCE: &str = include_str!("particle.vert");
const FRAGMENT_SOURCE: &str = include_str!("particle.frag");

/// One instance of one shape, as the shader reads it.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub center: [f32; 3],
    pub scale: [f32; 3],
    pub turn: [f32; 4],
    pub color: [f32; 4],
}

/// Everything drawn under one shape, texture and blend at once.
pub struct Batch {
    pub shape: Shape,
    pub texture: Option<TextureId>,
    pub blend: Blend,
    pub instances: Vec<Instance>,
}

/// One frame of camera and batches, rebuilt every time the widget draws.
pub struct Frame {
    pub view_projection: [f32; 16],
    /// The camera's own axes, which a sprite is set into.
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub batches: Vec<Batch>,
}

struct Buffers {
    layout: glow::VertexArray,
    vertices: glow::Buffer,
    indices: glow::Buffer,
    count: i32,
}

/// The quad every sprite draws, with its texture's origin at its top left corner.
fn quad() -> Mesh {
    let corners = [
        ([-0.5, -0.5], [0.0, 1.0]),
        ([0.5, -0.5], [1.0, 1.0]),
        ([0.5, 0.5], [1.0, 0.0]),
        ([-0.5, 0.5], [0.0, 0.0]),
    ];
    Mesh {
        vertices: corners
            .map(|(position, uv)| Vertex {
                position: [position[0], position[1], 0.0],
                uv,
                color: [255; 4],
            })
            .into(),
        indices: vec![0, 1, 2, 0, 2, 3],
    }
}

pub struct Particles {
    pending: Option<Vec<Mesh>>,
    program: Option<glow::Program>,
    /// The quad, then one entry per model the effect carries.
    shapes: Vec<Buffers>,
    instances: Option<glow::Buffer>,
    /// Instances the buffer has room for, so a frame rewrites it rather than reallocating one.
    capacity: usize,
    /// Why the shader would not build, kept so the viewer can say so rather than draw nothing.
    failure: Option<String>,
}

impl Particles {
    pub fn new(models: Vec<Mesh>) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            pending: Some(std::iter::once(quad()).chain(models).collect()),
            program: None,
            shapes: Vec::new(),
            instances: None,
            capacity: 0,
            failure: None,
        }))
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
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
        let Some(program) = self.program else {
            return;
        };

        let instances: Vec<Instance> = frame
            .batches
            .iter()
            .flat_map(|batch| batch.instances.iter().copied())
            .collect();
        if instances.is_empty() {
            return;
        }

        unsafe {
            let buffer = match self.instances {
                Some(buffer) => buffer,
                None => match gl.create_buffer() {
                    Ok(buffer) => *self.instances.insert(buffer),
                    Err(why) => {
                        self.failure = Some(why);
                        return;
                    }
                },
            };
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            let bytes: &[u8] = bytemuck::cast_slice(&instances);
            match instances.len() <= self.capacity {
                true => gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytes),
                false => {
                    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
                    self.capacity = instances.len();
                }
            }

            // Nothing here is opaque, so the order the batches arrive in is the whole of what sorts
            // them; a depth buffer this does not write to could only reject what it should draw.
            gl.disable(glow::DEPTH_TEST);
            gl.depth_mask(false);
            gl.disable(glow::CULL_FACE);
            gl.use_program(Some(program));

            let camera = gl.get_uniform_location(program, "u_view_projection");
            gl.uniform_matrix_4_f32_slice(camera.as_ref(), false, &frame.view_projection);
            let right = gl.get_uniform_location(program, "u_right");
            gl.uniform_3_f32_slice(right.as_ref(), &frame.right);
            let up = gl.get_uniform_location(program, "u_up");
            gl.uniform_3_f32_slice(up.as_ref(), &frame.up);
            let map = gl.get_uniform_location(program, "u_map");
            gl.uniform_1_i32(map.as_ref(), 0);
            let billboard = gl.get_uniform_location(program, "u_billboard");
            let textured = gl.get_uniform_location(program, "u_textured");
            let mode = gl.get_uniform_location(program, "u_mode");

            let mut at = 0;
            for batch in &frame.batches {
                let shape = match batch.shape {
                    Shape::Sprite => 0,
                    Shape::Model(model) => model + 1,
                };
                let Some(buffers) = self.shapes.get(shape) else {
                    at += batch.instances.len();
                    continue;
                };

                gl.bind_vertex_array(Some(buffers.layout));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
                let stride = size_of::<Instance>() as i32;
                for (location, size, offset) in INSTANCE {
                    gl.vertex_attrib_pointer_f32(
                        location,
                        size,
                        glow::FLOAT,
                        false,
                        stride,
                        at as i32 * stride + offset,
                    );
                }

                let texture = batch.texture.and_then(|id| painter.texture(id));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, texture);
                gl.uniform_1_i32(textured.as_ref(), i32::from(texture.is_some()));
                gl.uniform_1_i32(billboard.as_ref(), i32::from(shape == 0));
                gl.uniform_1_i32(mode.as_ref(), batch.blend as i32);
                blend(gl, batch.blend);

                gl.draw_elements_instanced(
                    glow::TRIANGLES,
                    buffers.count,
                    glow::UNSIGNED_SHORT,
                    0,
                    batch.instances.len() as i32,
                );
                at += batch.instances.len();
            }

            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.blend_equation(glow::FUNC_ADD);
            gl.disable(glow::BLEND);
            gl.disable(glow::DEPTH_TEST);
            gl.depth_mask(false);
        }
    }

    fn upload(&mut self, gl: &glow::Context, meshes: Vec<Mesh>) -> Result<(), String> {
        log::info!("assets/avfx: {} shapes on {:?}", meshes.len(), gl.version());
        self.program = Some(build(gl)?);
        for mesh in &meshes {
            self.shapes.push(upload(gl, mesh)?);
        }
        Ok(())
    }
}

impl Drop for Particles {
    fn drop(&mut self) {
        graveyard().lock().unwrap().extend(
            self.shapes
                .drain(..)
                .flat_map(|held| {
                    [
                        Dead::Layout(held.layout),
                        Dead::Buffer(held.vertices),
                        Dead::Buffer(held.indices),
                    ]
                })
                .chain(self.instances.take().map(Dead::Buffer))
                .chain(self.program.take().map(Dead::Program)),
        );
    }
}

/// How a blend's source and destination are weighted. The shader hands over a source already
/// multiplied by its own opacity, so alpha is not a factor here.
fn blend(gl: &glow::Context, blend: Blend) {
    unsafe {
        gl.blend_equation(match blend {
            Blend::Subtract => glow::FUNC_REVERSE_SUBTRACT,
            _ => glow::FUNC_ADD,
        });
        match blend {
            Blend::Opaque => {
                gl.disable(glow::BLEND);
                return;
            }
            Blend::Alpha => gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA),
            Blend::Multiply => gl.blend_func(glow::DST_COLOR, glow::ZERO),
            Blend::Screen => gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_COLOR),
            Blend::Subtract | Blend::Add => gl.blend_func(glow::ONE, glow::ONE),
        }
        gl.enable(glow::BLEND);
    }
}

/// One shape's buffers, with its own vertex array: egui leaves its own bound while a callback runs,
/// so setting attribute pointers without one would rewrite egui's layout.
fn upload(gl: &glow::Context, mesh: &Mesh) -> Result<Buffers, String> {
    unsafe {
        let layout = gl.create_vertex_array()?;
        gl.bind_vertex_array(Some(layout));

        let vertices = gl.create_buffer()?;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertices));
        gl.buffer_data_u8_slice(
            glow::ARRAY_BUFFER,
            bytemuck::cast_slice(&mesh.vertices),
            glow::STATIC_DRAW,
        );

        let stride = size_of::<Vertex>() as i32;
        for (location, size, offset) in ATTRIBUTES {
            gl.enable_vertex_attrib_array(location);
            gl.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, stride, offset);
        }
        gl.enable_vertex_attrib_array(COLOR);
        gl.vertex_attrib_pointer_f32(COLOR, 4, glow::UNSIGNED_BYTE, true, stride, COLOR_OFFSET);
        for (location, ..) in INSTANCE {
            gl.enable_vertex_attrib_array(location);
            gl.vertex_attrib_divisor(location, 1);
        }

        let indices = gl.create_buffer()?;
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(indices));
        gl.buffer_data_u8_slice(
            glow::ELEMENT_ARRAY_BUFFER,
            bytemuck::cast_slice(&mesh.indices),
            glow::STATIC_DRAW,
        );

        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        Ok(Buffers {
            layout,
            vertices,
            indices,
            count: mesh.indices.len() as i32,
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
