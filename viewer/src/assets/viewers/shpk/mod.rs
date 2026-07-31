//! `.shpk` shader packages: the compiled shaders a material names, and the resources, parameters
//! and keys they are driven by.

mod keys;
mod list;
mod merged;
mod params;

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use egui::ScrollArea;
use hlsl::layout::Member;
use ironworks::file::shpk::{self, Stage};
use shaders::names;

use super::shader::{self, Naming, ResourceRow, Shader};
use super::{Preview, facts, section};
use crate::assets::Bytes;
use keys::Keys;
use params::{COMPONENTS, Component, ParamRow};

/// A shader package, decoded and ready to draw.
pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    /// Resources under the heading each group is drawn with.
    resources: Vec<(&'static str, Vec<ResourceRow>)>,
    params: Vec<ParamRow>,
    /// The parameter buffer as the registers a shader addresses it by.
    registers: Vec<[Option<Component>; COMPONENTS]>,
    /// The shader's defaults for that buffer, indexed by the same float.
    defaults: Vec<f32>,
    keys: Keys,
    /// Stage, how many shaders it holds, and how much bytecode they take.
    stages: Vec<(&'static str, usize, usize)>,
    shaders: Vec<Shader>,
    naming: Naming,
    /// Which stage is filtered to and which shader is picked, kept per file the way the icon sheet
    /// keeps its controller.
    state: egui::Id,
}

/// Constant buffer layouts, by the resource id that names the buffer.
///
/// The layouts live in the compiled bytecode's reflection rather than in the package tables, and
/// every shader that binds a buffer describes it identically. So rather than sweeping thousands of
/// blobs, this walks the shader list once and takes only those that bind a buffer nothing before
/// them did: enough to cover every declared buffer, in around ten blobs even for the largest
/// package.
fn layouts(package: &shpk::ShaderPackage, bytes: &[u8]) -> HashMap<u32, Vec<Member>> {
    let wanted: HashSet<u32> = package.constants().iter().map(|c| c.id()).collect();
    let mut seen = HashSet::new();
    let mut found = HashMap::new();

    for shader in package.shaders() {
        if seen.len() == wanted.len() {
            break;
        }
        let adds = shader
            .resources()
            .iter()
            .any(|resource| wanted.contains(&resource.id()) && seen.insert(resource.id()));
        if !adds {
            continue;
        }

        let start = package.blobs_offset() + usize::try_from(shader.blob_offset()).unwrap_or(0);
        let end = start.saturating_add(usize::try_from(shader.blob_size()).unwrap_or(0));
        if let Some(blob) = bytes.get(start..end) {
            shader::buffers(blob, &mut found);
        }
    }
    found
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    // Parsed off the caller's bytes rather than an owned copy
    let package = shpk::ShaderPackage::parse(bytes)?;
    let layouts = layouts(&package, bytes);

    let mut stages: Vec<(&'static str, usize, usize)> = Vec::new();
    let mut shaders = Vec::with_capacity(package.shaders().len());
    for shader in package.shaders() {
        let stage = match shader.stage() {
            Stage::Vertex => "Vertex",
            Stage::Pixel => "Pixel",
            Stage::Hull => "Hull",
            Stage::Domain => "Domain",
            Stage::Geometry => "Geometry",
        };
        let size = usize::try_from(shader.blob_size()).unwrap_or(0);
        let start = package.blobs_offset() + usize::try_from(shader.blob_offset()).unwrap_or(0);
        shaders.push(Shader {
            stage,
            blob: start..start.saturating_add(size),
            bindings: shader::bindings(shader.constants(), shader.samplers(), shader.textures()),
        });
        match stages.iter_mut().find(|(name, _, _)| *name == stage) {
            Some((_, count, bytes)) => {
                *count += 1;
                *bytes += size;
            }
            None => stages.push((stage, 1, size)),
        }
    }

    let resources = shader::resources(
        [
            package.constants(),
            package.samplers(),
            package.textures(),
            package.uavs(),
        ],
        |resource| package.name(resource),
        &layouts,
    );

    let params = params::rows(&package);
    let registers = params::registers(&package);

    let [subview_one, subview_two] = package.subview_defaults();
    let identity = vec![
        ("Version", format!("{:#06X}", package.version())),
        (
            "DirectX",
            match package.directx() {
                shpk::DirectX::Dx9 => "9".to_owned(),
                shpk::DirectX::Dx11 => "11".to_owned(),
                shpk::DirectX::Unknown(tag) => String::from_utf8_lossy(&tag).into_owned(),
            },
        ),
        ("Shaders", package.shaders().len().to_string()),
        ("Bytecode", Bytes(package.bytecode_size()).to_string()),
        (
            "Parameter buffer",
            format!(
                "{} registers ({} B)",
                registers.len(),
                package.param_buffer_size()
            ),
        ),
        ("Selector nodes", package.nodes().len().to_string()),
        ("Aliases", package.aliases().len().to_string()),
        ("Subview 1", shader::named(subview_one)),
        ("Subview 2", shader::named(subview_two)),
    ];

    let naming = Naming {
        resources: package
            .shaders()
            .iter()
            .flat_map(shpk::Shader::resources)
            .chain(package.constants())
            .chain(package.samplers())
            .chain(package.textures())
            .chain(package.uavs())
            .filter_map(|resource| Some((resource.id(), package.name(resource)?.to_owned())))
            .collect(),
        // The buffer a material fills, whose fields the reflection does not name; its registers are
        // read off the package's own parameter table instead.
        packed: Some(params::packed(
            names::hash(b"g_MaterialParameter"),
            &registers,
            &params,
        )),
        layouts,
    };

    Ok(Preview::Shpk(Box::new(Rendered {
        identity,
        resources,
        params,
        registers,
        defaults: package.param_defaults().to_vec(),
        keys: keys::read(&package),
        stages,
        shaders,
        naming,
        state: egui::Id::new(("shpk shader", path)),
    })))
}

pub fn ui(ui: &mut egui::Ui, package: &Rendered, bytes: &[u8]) {
    // No scroll area around this: the list and the code each carry their own, and an outer one
    // leaves the code unable to tell how much of the panel is left for it.
    list::ui(ui, package, bytes);
}

/// Everything about the package that is not a shader. It sits beside the code rather than above it,
/// where it would push the thing being read off the screen.
fn metadata_ui(ui: &mut egui::Ui, package: &Rendered) {
    if !package.registers.is_empty() {
        section(ui, "Material parameters");
        // Four columns of long names overflow a narrow panel, and only this table does.
        ScrollArea::horizontal()
            .id_salt("shpk_params_scroll")
            .show(ui, |ui| {
                // Or every name wraps to the width of a narrow panel instead of the table simply
                // being wider than one.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                params::ui(ui, &package.registers, &package.params, &package.defaults);
            });
        ui.add_space(8.0);
        ui.separator();
    }

    if package.resources.iter().any(|(_, rows)| !rows.is_empty()) {
        section(ui, "Resources");
        ScrollArea::horizontal()
            .id_salt("shpk_resources_scroll")
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                shader::resources_ui(ui, &package.resources);
            });
        ui.add_space(8.0);
        ui.separator();
    }

    if package.keys.any() {
        section(ui, "Keys");
        ScrollArea::horizontal()
            .id_salt("shpk_keys_scroll")
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                package.keys.ui(ui);
            });
    }
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            // Whichever shader the list has picked, so that clicking between two leaves this in
            // place and what differs is the rows that changed. A merged source is every shader of
            // the pass at once, so there is no one set of conditions it was compiled under.
            if let Some((_, _, picked)) = ui
                .data(|data| data.get_temp::<(usize, usize, usize)>(self.state))
                .filter(|_| !merged::reading(ui, self.state))
            {
                self.keys.defines_ui(ui, picked);
                ui.add_space(8.0);
                ui.separator();
            }
            facts(ui, "shpk_identity", &self.identity);
            ui.add_space(8.0);
            ui.separator();
            metadata_ui(ui, self);
        });
    }
}
