//! The GL side of the PCB viewer.

use std::sync::{Arc, Mutex};

use glow::HasContext;

use super::super::mdl::gpu::{Dead, bury, graveyard};

const ATTRIBUTES: [(u32, i32, i32); 3] = [(0, 3, 0), (1, 3, 12), (2, 4, 24)];
const VERTEX_SOURCE: &str = include_str!("pcb.vert");
const FRAGMENT_SOURCE: &str = include_str!("pcb.frag");

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 4],
}

#[derive(Clone)]
pub struct Geometry {
    pub triangle_vertices: Vec<Vertex>,
    pub triangle_indices: Vec<u32>,
    pub wire_vertices: Vec<Vertex>,
    pub wire_indices: Vec<u32>,
    pub bounds: ([f32; 3], [f32; 3]),
}

impl Geometry {
    pub fn new() -> Self {
        Self {
            triangle_vertices: Vec::new(),
            triangle_indices: Vec::new(),
            wire_vertices: Vec::new(),
            wire_indices: Vec::new(),
            bounds: ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]),
        }
    }
}

struct Mesh {
    layout: glow::VertexArray,
    vertices: glow::Buffer,
    indices: glow::Buffer,
    count: i32,
}

pub struct Renderer {
    program: Option<glow::Program>,
    solid: Vec<Mesh>,
    wire: Vec<Mesh>,
    pending: Vec<Arc<Geometry>>,
    failure: Option<String>,
}

impl Renderer {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            program: None,
            solid: Vec::new(),
            wire: Vec::new(),
            pending: Vec::new(),
            failure: None,
        }))
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Hands over one sub-mesh's geometry, uploaded on the next draw alongside whatever already
    /// landed rather than replacing it.
    pub fn queue(&mut self, geometry: Arc<Geometry>) {
        self.pending.push(geometry);
    }

    pub fn draw(
        &mut self,
        gl: &glow::Context,
        painter: &egui_glow::Painter,
        view_projection: &[f32; 16],
        eye_pos: &[f32; 3],
        show_wire: bool,
    ) {
        bury(gl);
        if self.failure.is_some() {
            return;
        }
        for geometry in std::mem::take(&mut self.pending) {
            if let Err(why) = self.upload(gl, &geometry) {
                self.failure = Some(why);
                return;
            }
        }
        let Some(program) = self.program else {
            return;
        };

        unsafe {
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
            gl.disable(glow::BLEND);
            gl.enable(glow::CULL_FACE);
            gl.cull_face(glow::BACK);
            gl.use_program(Some(program));

            let view = gl.get_uniform_location(program, "u_view_projection");
            gl.uniform_matrix_4_f32_slice(view.as_ref(), false, view_projection);
            let eye = gl.get_uniform_location(program, "u_eye");
            gl.uniform_3_f32_slice(eye.as_ref(), eye_pos);

            for (meshes, mode) in [
                (Some(&self.solid), glow::TRIANGLES),
                (Some(&self.wire).filter(|_| show_wire), glow::LINES),
            ] {
                for mesh in meshes.into_iter().flatten() {
                    gl.bind_vertex_array(Some(mesh.layout));
                    gl.draw_elements(mode, mesh.count, glow::UNSIGNED_INT, 0);
                }
            }

            gl.bind_vertex_array(None);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
        }
        painter.gl();
    }

    fn upload(&mut self, gl: &glow::Context, geometry: &Geometry) -> Result<(), String> {
        log::info!(
            "assets/pcb: {} vertices, {} triangles on {:?}",
            geometry.triangle_vertices.len(),
            geometry.triangle_indices.len() / 3,
            gl.version()
        );
        if self.program.is_none() {
            self.program = Some(build(gl)?);
        }
        if !geometry.triangle_vertices.is_empty() {
            self.solid.push(upload_mesh(
                gl,
                &geometry.triangle_vertices,
                &geometry.triangle_indices,
            )?);
        }
        if !geometry.wire_vertices.is_empty() {
            self.wire.push(upload_mesh(
                gl,
                &geometry.wire_vertices,
                &geometry.wire_indices,
            )?);
        }
        Ok(())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let mut dead = graveyard().lock().unwrap();
        dead.extend(self.program.take().map(Dead::Program));
        for mesh in self.solid.drain(..).chain(self.wire.drain(..)) {
            dead.push(Dead::Layout(mesh.layout));
            dead.push(Dead::Buffer(mesh.vertices));
            dead.push(Dead::Buffer(mesh.indices));
        }
    }
}

fn cast<T>(values: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}

fn build(gl: &glow::Context) -> Result<glow::Program, String> {
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, source) in [
            (glow::VERTEX_SHADER, VERTEX_SOURCE),
            (glow::FRAGMENT_SHADER, FRAGMENT_SOURCE),
        ] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                return Err(gl.get_shader_info_log(shader));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        match gl.get_program_link_status(program) {
            true => Ok(program),
            false => Err(gl.get_program_info_log(program)),
        }
    }
}

fn upload_mesh(gl: &glow::Context, vertices: &[Vertex], indices: &[u32]) -> Result<Mesh, String> {
    unsafe {
        let layout = gl.create_vertex_array()?;
        gl.bind_vertex_array(Some(layout));

        let held = gl.create_buffer()?;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(held));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, cast(vertices), glow::STATIC_DRAW);
        let stride = size_of::<Vertex>() as i32;
        for (location, size, offset) in ATTRIBUTES {
            gl.enable_vertex_attrib_array(location);
            gl.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, stride, offset);
        }

        let drawn = gl.create_buffer()?;
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(drawn));
        gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, cast(indices), glow::STATIC_DRAW);

        gl.bind_vertex_array(None);
        Ok(Mesh {
            layout,
            vertices: held,
            indices: drawn,
            count: indices.len() as i32,
        })
    }
}
