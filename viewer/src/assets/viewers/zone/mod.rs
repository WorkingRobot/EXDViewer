//! The files a zone's scene names beside its layers: what the sky and the lights do to what was
//! placed, and how the day runs through the environment it is lit, shaded and heard in.

pub mod amb;
pub mod envs;
pub mod instanced;

use super::chip;

fn axes(values: [f32; 3]) -> String {
    format!("{:.3}, {:.3}, {:.3}", values[0], values[1], values[2])
}

/// A time the file states as seconds since midnight.
fn clock(seconds: f32) -> String {
    let time = seconds.max(0.0) as u32;
    format!("{:02}:{:02}:{:02}", time / 3600, time / 60 % 60, time % 60)
}
