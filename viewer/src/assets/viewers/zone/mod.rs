//! The files a zone's scene names beside its layers: what the sky and the lights do to what was
//! placed, and how the day runs through the environment it is lit, shaded and heard in.

use egui::{Color32, Sense, Vec2};

pub mod amb;
pub mod envs;
pub mod instanced;

/// Side of the square a color is drawn in.
const CHIP: f32 = 10.0;

fn axes(values: [f32; 3]) -> String {
    format!("{:.3}, {:.3}, {:.3}", values[0], values[1], values[2])
}

/// A time the file states as seconds since midnight.
fn clock(seconds: f32) -> String {
    let time = seconds.max(0.0) as u32;
    format!("{:02}:{:02}:{:02}", time / 3600, time / 60 % 60, time % 60)
}

/// One color, drawn beside the numbers it came from.
fn chip(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(CHIP), Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
}
