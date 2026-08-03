//! The GL side of the placement view: two unit shapes, drawn once each with a transform and a
//! color per instance.
//!
//! Objects are made on the first frame that draws them and dead ones go to the model viewer's
//! graveyard, since a viewer is dropped between frames and there is no context to free them in.

use std::sync::{Arc, Mutex};

use glow::HasContext;

use super::super::mdl::gpu::{Dead, bury, graveyard};

/// Position and normal, both three floats.
const ATTRIBUTES: [(u32, i32, i32); 2] = [(0, 3, 0), (1, 3, 12)];

/// Center, scale, rotation and color, one set per instance.
const INSTANCE: [(u32, i32, i32); 4] = [(2, 3, 0), (3, 3, 12), (4, 4, 24), (5, 4, 40)];

const VERTEX_SOURCE: &str = include_str!("placed.vert");
const FRAGMENT_SOURCE: &str = include_str!("placed.frag");

/// What one thing is drawn as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shape {
    /// A solid box, lit so its faces read apart.
    Box,
    /// The twelve edges of one, for the volumes that are bounds rather than objects.
    Wire,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Instance {
    pub center: [f32; 3],
    pub scale: [f32; 3],
    pub turn: [f32; 4],
    pub color: [f32; 4],
}

pub struct Batch {
    pub shape: Shape,
    pub instances: Vec<Instance>,
}

pub struct Frame {
    pub view_projection: [f32; 16],
    pub batches: Vec<Batch>,
}

/// One shape on the card.
struct Mesh {
    layout: glow::VertexArray,
    vertices: glow::Buffer,
    indices: glow::Buffer,
    count: i32,
}

pub struct Placements {
    program: Option<glow::Program>,
    solid: Option<Mesh>,
    wire: Option<Mesh>,
    instances: Option<glow::Buffer>,
    failure: Option<String>,
}

impl Placements {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            program: None,
            solid: None,
            wire: None,
            instances: None,
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
        if let Err(error) = self.build(gl) {
            self.failure = Some(error);
            return;
        }

        let (Some(program), Some(instances)) = (self.program, self.instances) else {
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
            gl.uniform_matrix_4_f32_slice(view.as_ref(), false, &frame.view_projection);
            let lit = gl.get_uniform_location(program, "u_lit");

            // Every batch reads out of one buffer, each from its own offset.
            let all: Vec<Instance> = frame
                .batches
                .iter()
                .flat_map(|batch| batch.instances.iter().copied())
                .collect();
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(instances));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, cast(&all), glow::DYNAMIC_DRAW);

            let mut at = 0;
            for batch in &frame.batches {
                let (mesh, mode, light) = match batch.shape {
                    Shape::Box => (self.solid.as_ref(), glow::TRIANGLES, 1.0),
                    Shape::Wire => (self.wire.as_ref(), glow::LINES, 0.0),
                };
                let Some(mesh) = mesh else { continue };
                gl.uniform_1_f32(lit.as_ref(), light);
                gl.bind_vertex_array(Some(mesh.layout));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(instances));
                let stride = size_of::<Instance>() as i32;
                let base = at * stride;
                for (location, size, offset) in INSTANCE {
                    gl.enable_vertex_attrib_array(location);
                    gl.vertex_attrib_pointer_f32(
                        location,
                        size,
                        glow::FLOAT,
                        false,
                        stride,
                        base + offset,
                    );
                    gl.vertex_attrib_divisor(location, 1);
                }
                gl.draw_elements_instanced(
                    mode,
                    mesh.count,
                    glow::UNSIGNED_SHORT,
                    0,
                    batch.instances.len() as i32,
                );
                at += batch.instances.len() as i32;
            }

            gl.bind_vertex_array(None);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
        }
        painter.gl();
    }

    fn build(&mut self, gl: &glow::Context) -> Result<(), String> {
        if self.program.is_none() {
            self.program = Some(program(gl)?);
        }
        if self.solid.is_none() {
            let (vertices, indices) = solid();
            self.solid = Some(mesh(gl, &vertices, &indices)?);
        }
        if self.wire.is_none() {
            let (vertices, indices) = wire();
            self.wire = Some(mesh(gl, &vertices, &indices)?);
        }
        if self.instances.is_none() {
            self.instances = Some(unsafe { gl.create_buffer() }?);
        }
        Ok(())
    }
}

impl Drop for Placements {
    fn drop(&mut self) {
        let mut dead = graveyard().lock().unwrap();
        dead.extend(self.program.take().map(Dead::Program));
        dead.extend(self.instances.take().map(Dead::Buffer));
        for mesh in [self.solid.take(), self.wire.take()].into_iter().flatten() {
            dead.push(Dead::Layout(mesh.layout));
            dead.push(Dead::Buffer(mesh.vertices));
            dead.push(Dead::Buffer(mesh.indices));
        }
    }
}

fn cast<T>(values: &[T]) -> &[u8] {
    // Both are plain data and the slice is only ever handed straight to the driver.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}

fn program(gl: &glow::Context) -> Result<glow::Program, String> {
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

/// A mesh with a layout of its own, since egui leaves its own array bound around a callback.
fn mesh(gl: &glow::Context, vertices: &[f32], indices: &[u16]) -> Result<Mesh, String> {
    unsafe {
        let layout = gl.create_vertex_array()?;
        gl.bind_vertex_array(Some(layout));

        let buffer = gl.create_buffer()?;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, cast(vertices), glow::STATIC_DRAW);
        let stride = 6 * size_of::<f32>() as i32;
        for (location, size, offset) in ATTRIBUTES {
            gl.enable_vertex_attrib_array(location);
            gl.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, stride, offset);
        }

        let elements = gl.create_buffer()?;
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(elements));
        gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, cast(indices), glow::STATIC_DRAW);

        gl.bind_vertex_array(None);
        Ok(Mesh {
            layout,
            vertices: buffer,
            indices: elements,
            count: indices.len() as i32,
        })
    }
}

/// The corners of a unit box, in the order the faces below index them.
const CORNERS: [[f32; 3]; 8] = [
    [-1.0, -1.0, -1.0],
    [1.0, -1.0, -1.0],
    [1.0, 1.0, -1.0],
    [-1.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [1.0, -1.0, 1.0],
    [1.0, 1.0, 1.0],
    [-1.0, 1.0, 1.0],
];

/// Each face as its four corners and the way it points, wound so the outside is front facing.
const FACES: [([usize; 4], [f32; 3]); 6] = [
    ([4, 5, 6, 7], [0.0, 0.0, 1.0]),
    ([1, 0, 3, 2], [0.0, 0.0, -1.0]),
    ([1, 2, 6, 5], [1.0, 0.0, 0.0]),
    ([0, 4, 7, 3], [-1.0, 0.0, 0.0]),
    ([3, 7, 6, 2], [0.0, 1.0, 0.0]),
    ([0, 1, 5, 4], [0.0, -1.0, 0.0]),
];

fn solid() -> (Vec<f32>, Vec<u16>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for (corners, normal) in FACES {
        let first = (vertices.len() / 6) as u16;
        for corner in corners {
            vertices.extend(CORNERS[corner]);
            vertices.extend(normal);
        }
        indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    }
    (vertices, indices)
}

fn wire() -> (Vec<f32>, Vec<u16>) {
    let mut vertices = Vec::new();
    for corner in CORNERS {
        vertices.extend(corner);
        vertices.extend([0.0, 1.0, 0.0]);
    }
    let mut indices = Vec::new();
    for at in 0..4u16 {
        let next = (at + 1) % 4;
        indices.extend([at, next, at + 4, next + 4, at, at + 4]);
    }
    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::{CORNERS, solid, wire};

    /// Six faces of four corners, each carrying its own normal, wound so the outside faces out.
    #[test]
    fn the_solid_box_is_closed() {
        let (vertices, indices) = solid();
        assert_eq!(vertices.len(), 6 * 4 * 6);
        assert_eq!(indices.len(), 6 * 6);
        assert!(indices.iter().all(|&index| usize::from(index) < 24));
    }

    /// Twelve edges: a ring at each end and the four uprights between them, each corner meeting
    /// three of them.
    #[test]
    fn the_wire_box_has_every_edge_once() {
        let (vertices, indices) = wire();
        assert_eq!(vertices.len(), CORNERS.len() * 6);

        let mut edges: Vec<(u16, u16)> = indices
            .chunks(2)
            .map(|pair| (pair[0].min(pair[1]), pair[0].max(pair[1])))
            .collect();
        assert_eq!(edges.len(), 12);
        edges.sort_unstable();
        edges.dedup();
        assert_eq!(edges.len(), 12, "an edge is drawn twice");

        for corner in 0..8u16 {
            let met = edges
                .iter()
                .filter(|(from, to)| *from == corner || *to == corner)
                .count();
            assert_eq!(met, 3, "corner {corner} meets {met} edges");
        }
        // Every edge runs along one axis, which is what makes it a box rather than a diagonal.
        for (from, to) in edges {
            let differ = (0..3)
                .filter(|axis| CORNERS[usize::from(from)][*axis] != CORNERS[usize::from(to)][*axis])
                .count();
            assert_eq!(differ, 1);
        }
    }
}
