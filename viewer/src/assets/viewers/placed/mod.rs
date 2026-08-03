//! What a file puts in space, flown around.
//!
//! A file that places things gives them here as a transform and a color apiece; the view frames
//! them all on arrival and is orbited, panned and zoomed from there. Nothing about it is the game's
//! renderer: a box is a box, and the color is whatever the viewer chose to tell things apart by.

mod gpu;

use std::cell::Cell;
use std::sync::{Arc, Mutex};

use egui::{Color32, RichText, Sense};
use glam::{Mat4, Vec3};

pub use gpu::{Batch, Instance, Shape};

/// Vertical field of view.
const FOV: f32 = 55.0_f32.to_radians();

/// How much further out than the things themselves the view opens at.
const MARGIN: f32 = 1.8;

#[derive(Clone, Copy)]
struct Camera {
    yaw: f32,
    pitch: f32,
    distance: f32,
    target: Vec3,
}

impl Camera {
    fn eye(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        self.target
            + Vec3::new(
                self.distance * cos_pitch * sin_yaw,
                self.distance * sin_pitch,
                self.distance * cos_pitch * cos_yaw,
            )
    }
}

/// Everything one file placed, ready to draw.
pub struct View {
    batches: Vec<Batch>,
    renderer: Arc<Mutex<gpu::Placements>>,
    camera: Cell<Camera>,
    home: Camera,
    /// Half the extent of what was placed, which sets the clip planes and the zoom range.
    reach: f32,
}

impl View {
    /// Frames `batches` on the box they all fall inside.
    pub fn new(batches: Vec<Batch>) -> Self {
        let mut low = Vec3::splat(f32::INFINITY);
        let mut high = Vec3::splat(f32::NEG_INFINITY);
        for instance in batches.iter().flat_map(|batch| &batch.instances) {
            let center = Vec3::from_array(instance.center);
            let scale = Vec3::from_array(instance.scale).abs();
            low = low.min(center - scale);
            high = high.max(center + scale);
        }
        let (target, reach) = match low.x <= high.x {
            true => (
                (low + high) * 0.5,
                ((high - low).length() * 0.5).max(f32::EPSILON),
            ),
            false => (Vec3::ZERO, 1.0),
        };

        let home = Camera {
            yaw: 0.7,
            pitch: 0.5,
            distance: reach * MARGIN,
            target,
        };
        Self {
            batches,
            renderer: gpu::Placements::new(),
            camera: Cell::new(home),
            home,
            reach,
        }
    }

    /// How many things the view holds, for the caller to say so.
    pub fn count(&self) -> usize {
        self.batches.iter().map(|batch| batch.instances.len()).sum()
    }

    pub fn ui(&self, ui: &mut egui::Ui) {
        if let Some(failure) = self.renderer.lock().unwrap().failure() {
            ui.centered_and_justified(|ui| {
                ui.colored_label(Color32::RED, format!("Could not draw this: {failure}"));
            });
            return;
        }
        if self.batches.iter().all(|batch| batch.instances.is_empty()) {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("This file places nothing").weak());
            });
            return;
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("Drag to orbit, right-drag to pan, scroll to zoom").weak());
            if ui.button("Reset view").clicked() {
                self.camera.set(self.home);
            }
        });
        ui.add_space(4.0);
        self.viewport(ui);
    }

    fn viewport(&self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return;
        }

        let mut camera = self.camera.get();
        let pan = |camera: &mut Camera, delta: egui::Vec2| {
            let (sin_yaw, cos_yaw) = camera.yaw.sin_cos();
            let right = Vec3::new(cos_yaw, 0.0, -sin_yaw);
            let scale = camera.distance * 0.002;
            camera.target += (right * -delta.x + Vec3::Y * delta.y) * scale;
        };
        let zoom = |camera: &mut Camera, scale: f32| {
            camera.distance = (camera.distance * scale)
                .clamp(self.home.distance * 0.005, self.home.distance * 20.0);
        };

        // A second finger takes the gesture over: egui carries on reporting a primary drag through
        // one, so leaving the orbit armed would spin the scene while it is being pinched.
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
        let span = (eye - self.home.target).length();
        let near = (span - self.reach).max(self.reach * 0.002);
        let projection =
            Mat4::perspective_rh_gl(FOV, rect.width() / rect.height(), near, span + self.reach);

        let frame = gpu::Frame {
            view_projection: (projection * view).to_cols_array(),
            batches: self
                .batches
                .iter()
                .map(|batch| Batch {
                    shape: batch.shape,
                    instances: batch.instances.clone(),
                })
                .collect(),
        };

        // The context is taken from the painter rather than captured: `glow::Context` is neither
        // `Send` nor `Sync` on wasm, and a callback has to be both.
        let renderer = self.renderer.clone();
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                renderer.lock().unwrap().draw(painter.gl(), painter, &frame);
            })),
        });
    }
}

/// A color from an index, so runs of things tell apart without the caller picking any.
pub fn tint(index: usize) -> [f32; 4] {
    let hue = (index as f32 * 0.618_034).fract();
    let wrap = |offset: f32| ((hue + offset).fract() * 6.0 - 3.0).abs().clamp(0.0, 1.0);
    [
        0.35 + 0.65 * wrap(0.0),
        0.35 + 0.65 * wrap(1.0 / 3.0),
        0.35 + 0.65 * wrap(2.0 / 3.0),
        1.0,
    ]
}
