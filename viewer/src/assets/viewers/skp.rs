//! `.skp` skeleton parameters: the bones an animation drives, how a chain of them turns towards a
//! look-at target, and how the body leans on a slope.

use anyhow::Result;
use egui::{RichText, ScrollArea, vec2};
use ironworks::file::{File, skp};
use std::io::Cursor;

use super::{Preview, facts, headers, heading, link, section};
use crate::utils::file_name;

/// One look-at parameter set, which a chain's bones select between.
struct Param {
    index: u8,
    limit_angles: [f32; 4],
    limit_angle: f32,
    forward_rotation: [f32; 3],
    eye_position: [f32; 3],
    gain: f32,
    flags: u32,
}

/// One bone of a look-at chain.
struct Element {
    priority: u8,
    param: u8,
    bone: String,
    parent: String,
}

/// A named chain of bones driven by look-at.
struct Group {
    name: String,
    elements: Vec<Element>,
}

/// How the body leans on a slope.
struct Slope {
    angles: [f32; 2],
    points: Vec<[f32; 3]>,
}

/// A parameter file, decoded and ready to draw.
pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    /// The skeleton these parameters are layered over, which sits beside the file under the same
    /// name.
    skeleton: String,
    /// Each layer's own flag word, and the bones it drives.
    layers: Vec<(u32, String)>,
    params: Vec<Param>,
    groups: Vec<Group>,
    slope: Option<Slope>,
}

fn axes(values: [f32; 3]) -> String {
    format!("{:.3}, {:.3}, {:.3}", values[0], values[1], values[2])
}

/// A name buffer as written, where the game leaves uninitialized bytes past the terminator.
fn named(name: skp::Name) -> String {
    name.as_str().unwrap_or("?").to_owned()
}

/// The sections the file declares, in flag order.
fn declared(sections: skp::Sections) -> String {
    let listed = [
        (sections.animation(), "animation"),
        (sections.look_at(), "look-at"),
        (sections.ccd(), "CCD"),
        (sections.feet(), "foot IK"),
        (sections.slope(), "slope"),
    ]
    .iter()
    .filter(|(declared, _)| *declared)
    .map(|(_, name)| *name)
    .collect::<Vec<_>>()
    .join(", ");
    match listed.is_empty() {
        true => "none".to_owned(),
        false => listed,
    }
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = skp::SkeletonParameters::read(Cursor::new(bytes.to_vec()))?;

    let layers = file
        .animation_layers()
        .iter()
        .map(|layer| {
            let bones = layer
                .bone_indices()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            (layer.layer(), bones)
        })
        .collect::<Vec<_>>();

    let params = file
        .look_at()
        .map(skp::LookAt::params)
        .into_iter()
        .flatten()
        .map(|param| Param {
            index: param.index(),
            limit_angles: param.limit_angles(),
            limit_angle: param.limit_angle(),
            forward_rotation: param.forward_rotation(),
            eye_position: param.eye_positions(),
            gain: param.gain(),
            flags: param.flags(),
        })
        .collect::<Vec<_>>();

    let groups = file
        .look_at()
        .map(skp::LookAt::groups)
        .into_iter()
        .flatten()
        .map(|group| Group {
            name: named(group.id()),
            elements: group
                .elements()
                .iter()
                .map(|element| Element {
                    priority: element.priority(),
                    param: element.param_index(),
                    bone: named(element.bone_name()),
                    parent: named(element.parent_bone_name()),
                })
                .collect(),
        })
        .collect::<Vec<_>>();

    let slope = file.slope().map(|slope| Slope {
        angles: slope.angles(),
        points: slope.points().clone(),
    });

    let mut identity = vec![
        (
            "Version",
            file.version()
                .number()
                .map_or_else(|| format!("{:?}", file.version()), |tag| tag.to_string()),
        ),
        ("Sections", declared(file.sections())),
        ("Animation layers", layers.len().to_string()),
    ];
    if file.sections().look_at() {
        identity.push(("Look-at parameters", params.len().to_string()));
        identity.push(("Look-at chains", groups.len().to_string()));
    }
    // Neither section has a known payload, so the offset is the whole of what the file says.
    if file.sections().ccd() {
        identity.push(("CCD offset", format!("{:#x}", file.ccd_offset())));
    }
    if file.sections().feet() {
        identity.push(("Foot IK offset", format!("{:#x}", file.foot_offset())));
    }
    if let Some(slope) = &slope {
        identity.push(("Slope points", slope.points.len().to_string()));
    }

    log::info!(
        "assets/skp: {path} sections {}, {} layers, {} look-at chains",
        declared(file.sections()),
        layers.len(),
        groups.len()
    );

    Ok(Preview::Skp(Box::new(Rendered {
        identity,
        skeleton: format!("{}.sklb", path.trim_end_matches(".skp")),
        layers,
        params,
        groups,
        slope,
    })))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) -> Option<String> {
    let mut follow = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Skeleton").weak());
        if link(ui, file_name(&file.skeleton), &file.skeleton) {
            follow = Some(file.skeleton.clone());
        }
    });
    ui.add_space(4.0);

    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        if !file.layers.is_empty() {
            section(ui, "Animation layers");
            for (layer, bones) in &file.layers {
                heading(ui, &format!("Layer {layer:#010x}"));
                ui.label(RichText::new(bones).monospace());
            }
        }

        if !file.params.is_empty() {
            ui.add_space(8.0);
            ui.separator();
            section(ui, "Look-at parameters");
            ScrollArea::horizontal()
                .id_salt("skp_params_scroll")
                .show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    egui::Grid::new("skp_params")
                        .num_columns(7)
                        .striped(true)
                        .show(ui, |ui| {
                            headers(
                                ui,
                                &[
                                    "#",
                                    "Limit angles",
                                    "Limit angle",
                                    "Forward rotation",
                                    "Eye position",
                                    "Gain",
                                    "Flags",
                                ],
                            );
                            for param in &file.params {
                                for cell in [
                                    param.index.to_string(),
                                    param
                                        .limit_angles
                                        .iter()
                                        .map(|angle| format!("{angle:.3}"))
                                        .collect::<Vec<_>>()
                                        .join(", "),
                                    format!("{:.3}", param.limit_angle),
                                    axes(param.forward_rotation),
                                    axes(param.eye_position),
                                    format!("{:.3}", param.gain),
                                    format!("{:#010x}", param.flags),
                                ] {
                                    ui.label(RichText::new(cell).monospace());
                                }
                                ui.allocate_space(vec2(ui.available_width(), 0.0));
                                ui.end_row();
                            }
                        });
                });
        }

        if !file.groups.is_empty() {
            ui.add_space(8.0);
            ui.separator();
            section(ui, "Look-at chains");
            for group in &file.groups {
                heading(ui, &group.name);
                egui::Grid::new(("skp_chain", &group.name))
                    .num_columns(4)
                    .striped(true)
                    .show(ui, |ui| {
                        headers(ui, &["Priority", "Parameter", "Bone", "Parent"]);
                        for element in &group.elements {
                            for cell in [
                                element.priority.to_string(),
                                element.param.to_string(),
                                element.bone.clone(),
                                element.parent.clone(),
                            ] {
                                ui.label(RichText::new(cell).monospace());
                            }
                            ui.allocate_space(vec2(ui.available_width(), 0.0));
                            ui.end_row();
                        }
                    });
            }
        }

        if let Some(slope) = &file.slope {
            ui.add_space(8.0);
            ui.separator();
            section(ui, "Slope");
            ui.label(
                RichText::new(format!(
                    "Angles {:.3}, {:.3} rad",
                    slope.angles[0], slope.angles[1]
                ))
                .monospace(),
            );
            heading(ui, "Points");
            for point in &slope.points {
                ui.label(RichText::new(axes(*point)).monospace());
            }
        }
    });
    follow
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| facts(ui, "skp_identity", &self.identity));
    }
}
