//! `.shcd` shader code: one compiled shader, as the game's post effect and compute passes load it.

use std::collections::HashMap;

use anyhow::Result;
use egui::ScrollArea;
use ironworks::file::shcd::{self, Stage};

use super::shader::{self, Naming, ResourceRow, Shader, code};
use super::{Preview, facts, section};
use crate::assets::Bytes;

/// A shader, decoded and ready to draw.
pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    /// Resources under the heading each group is drawn with.
    resources: Vec<(&'static str, Vec<ResourceRow>)>,
    shader: Shader,
    naming: Naming,
    /// Which reading is on show, kept per file the way the package viewer keeps its pick.
    state: egui::Id,
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let code = shcd::ShaderCode::parse(bytes)?;
    let stage = match code.stage() {
        Stage::Vertex => "Vertex",
        Stage::Pixel => "Pixel",
        Stage::Geometry => "Geometry",
        Stage::Compute => "Compute",
        Stage::Hull => "Hull",
        Stage::Domain => "Domain",
        Stage::Unknown(_) => "Unknown",
    };

    // Where a package counts a blob from where its bytecode begins, a .shcd states the offset
    // outright, so there is no base to add.
    let blob = code.blob_offset()..code.blob_offset().saturating_add(code.blob_size());
    let mut layouts = HashMap::new();
    if let Some(held) = bytes.get(blob.clone()) {
        shader::buffers(held, &mut layouts);
    }

    let resources = shader::resources(
        [
            code.constants(),
            code.samplers(),
            code.textures(),
            code.uavs(),
        ],
        |resource| code.name(resource),
        &layouts,
    );

    let identity = vec![
        ("Version", format!("{:#06X}", code.version())),
        (
            "Stage",
            match code.stage() {
                Stage::Unknown(tag) => format!("Unknown ({tag:#04X})"),
                _ => stage.to_owned(),
            },
        ),
        (
            "DirectX",
            match code.directx() {
                shcd::DirectX::Dx9 => "9".to_owned(),
                shcd::DirectX::Dx11 => "11".to_owned(),
                shcd::DirectX::Unknown(tag) => String::from_utf8_lossy(&tag).into_owned(),
            },
        ),
        ("Bytecode", Bytes(code.blob_size()).to_string()),
    ];

    Ok(Preview::Shcd(Box::new(Rendered {
        identity,
        resources,
        shader: Shader {
            stage,
            blob,
            bindings: shader::bindings(code.constants(), code.samplers(), code.textures()),
        },
        naming: Naming {
            resources: code
                .resources()
                .iter()
                .filter_map(|resource| Some((resource.id(), code.name(resource)?.to_owned())))
                .collect(),
            layouts,
            // Nothing here stands in for the reflection: a .shcd has no parameter table of its own.
            packed: None,
        },
        state: egui::Id::new(("shcd shader", path)),
    })))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered, bytes: &[u8]) {
    code::ui(
        ui,
        file.state,
        &format!("{} shader", file.shader.stage),
        &file.shader,
        &file.naming,
        bytes,
    );
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            facts(ui, "shcd_identity", &self.identity);
            if self.resources.iter().any(|(_, rows)| !rows.is_empty()) {
                ui.add_space(8.0);
                ui.separator();
                section(ui, "Resources");
                ScrollArea::horizontal()
                    .id_salt("shcd_resources_scroll")
                    .show(ui, |ui| {
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                        shader::resources_ui(ui, &self.resources);
                    });
            }
        });
    }
}
