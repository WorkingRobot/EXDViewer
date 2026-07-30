//! The parameter buffer a material fills, which the package lays out but does not describe.

use egui::RichText;
use ironworks::file::shpk;
use shaders::names;

use super::super::shader::{Owner, Packed, named};
use super::super::{hashed, headers};

/// Components in a vec4 register, which is the unit a constant buffer is addressed in.
pub const COMPONENTS: usize = 4;

pub struct ParamRow {
    pub name: String,
    pub id: u32,
    pub offset: u16,
    pub size: u16,
}

/// One float of the parameter buffer, and which parameter owns it.
#[derive(Clone, Copy)]
pub struct Component {
    param: usize,
    /// False where the component continues a parameter that began in an earlier one.
    start: bool,
}

/// The parameters a material fills.
pub fn rows(package: &shpk::ShaderPackage) -> Vec<ParamRow> {
    package
        .material_params()
        .iter()
        .map(|param| ParamRow {
            name: named(param.id()),
            id: param.id(),
            offset: param.byte_offset(),
            size: param.byte_size(),
        })
        .collect()
}

/// The parameter buffer as the registers a shader addresses it by.
pub fn registers(package: &shpk::ShaderPackage) -> Vec<[Option<Component>; COMPONENTS]> {
    let floats = usize::try_from(package.param_buffer_size()).unwrap_or(0) / 4;
    let mut registers = vec![[None; COMPONENTS]; floats.div_ceil(COMPONENTS)];
    for (index, param) in package.material_params().iter().enumerate() {
        let first = usize::from(param.byte_offset()) / 4;
        for step in 0..usize::from(param.byte_size()) / 4 {
            let at = first + step;
            if let Some(register) = registers.get_mut(at / COMPONENTS) {
                register[at % COMPONENTS] = Some(Component {
                    param: index,
                    start: step == 0,
                });
            }
        }
    }
    registers
}

/// The parameter buffer as a reading needs it.
///
/// The reflection leaves this one buffer as a bare array, so what it holds comes from the package's
/// own parameter table instead. Several parameters share a register, so each carries the components it
/// occupies in the one it starts in.
pub fn packed(
    buffer: u32,
    registers: &[[Option<Component>; COMPONENTS]],
    params: &[ParamRow],
) -> Packed {
    let owners = registers
        .iter()
        .map(|register| {
            let mut here: Vec<(usize, u8)> = Vec::new();
            for (component, cell) in register.iter().enumerate() {
                let Some(cell) = cell else { continue };
                match here.iter_mut().find(|(param, _)| *param == cell.param) {
                    Some((_, mask)) => *mask |= 1 << component,
                    None => here.push((cell.param, 1 << component)),
                }
            }
            here.into_iter()
                .map(|(param, mask)| {
                    let param = &params[param];
                    Owner {
                        name: param.name.clone(),
                        mask,
                        declared: names::resolve(param.id).is_some().then_some(param.size),
                    }
                })
                .collect()
        })
        .collect();
    Packed { buffer, owners }
}

/// The parameter buffer as the shader addresses it: a row per register, a column per component.
/// A component continuing a parameter that began earlier is dimmed, so a vec3 reads as one block
/// rather than three repetitions of a name.
pub fn ui(
    ui: &mut egui::Ui,
    registers: &[[Option<Component>; COMPONENTS]],
    params: &[ParamRow],
    defaults: &[f32],
) {
    egui::Grid::new("shpk_params")
        .num_columns(COMPONENTS + 1)
        .striped(true)
        .show(ui, |ui| {
            headers(ui, &["", "x", "y", "z", "w"]);
            for (index, register) in registers.iter().enumerate() {
                ui.label(RichText::new(format!("c{index}")).monospace().weak());
                for (component, cell) in register.iter().enumerate() {
                    let Some(cell) = cell else {
                        ui.label(RichText::new("·").weak());
                        continue;
                    };
                    let param = &params[cell.param];
                    ui.vertical(|ui| {
                        hashed(
                            ui,
                            &format!("Parameter at +{} B, {} B", param.offset, param.size),
                            &param.name,
                            param.id,
                            !cell.start,
                        );
                        if let Some(value) = defaults.get(index * COMPONENTS + component) {
                            ui.label(RichText::new(value.to_string()).weak().small());
                        }
                    });
                }
                ui.end_row();
            }
        });
}
