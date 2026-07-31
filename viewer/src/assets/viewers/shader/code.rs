//! A shader as code: the HLSL it reads back as, or the assembly it was read from.

use std::sync::Arc;

use dxbc::chunks::ChunkData;
use egui::{RichText, ScrollArea};
use hlsl::layout::Member;

use super::{Binding, Naming, Register, Shader, program};
use crate::assets::viewers::heading;
use crate::settings::CODE_SYNTAX_THEME;
use crate::utils::highlight;

/// Space kept between the code and the edge of the surface it is drawn on.
const MARGIN: i8 = 6;

/// One shader, read either way.
///
/// Only this shader is read, and only its own blob is touched: the worst case in the shipped files is
/// a shader of some eight milliseconds, against forty-five seconds to do all twenty-six thousand of a
/// package at once.
pub fn ui(
    ui: &mut egui::Ui,
    state: egui::Id,
    title: &str,
    shader: &Shader,
    naming: &Naming,
    bytes: &[u8],
) {
    let reading = state.with("source");
    let mut source = ui
        .data(|data| data.get_temp::<bool>(reading))
        .unwrap_or(true);
    ui.horizontal(|ui| {
        heading(ui, title);
        if ui.selectable_label(source, "HLSL").clicked() {
            source = true;
        }
        if ui.selectable_label(!source, "Assembly").clicked() {
            source = false;
        }
    });
    ui.data_mut(|data| data.insert_temp(reading, source));

    // Held against the pick rather than redone each frame: the text runs to thirteen hundred lines,
    // and nothing about it changes until another shader or another reading is chosen.
    let slot = state.with("reading");
    let cached = ui.data(|data| data.get_temp::<((usize, bool), Arc<(Vec<String>, usize)>)>(slot));
    let at = (shader.blob.start, source);
    let lines = match cached {
        Some((held, lines)) if held == at => Some(lines),
        _ => {
            let fresh = bytes
                .get(shader.blob.clone())
                .and_then(|blob| Some((program(blob)?, blob)))
                .map(|(program, blob)| {
                    Arc::new(match source {
                        true => {
                            let read = hlsl::decompile(&program, &names(naming, shader, blob));
                            (read.lines, read.body)
                        }
                        // The assembly names nothing itself, so what a line touches goes in a
                        // comment beside it. It declares as it goes, so there is nothing to fold.
                        false => (
                            annotate(naming, shader, &dxbc::shex::format_program(&program)),
                            0,
                        ),
                    })
                });
            if let Some(lines) = &fresh {
                ui.data_mut(|data| data.insert_temp(slot, (at, Arc::clone(lines))));
            }
            fresh
        }
    };

    let Some(held) = lines else {
        ui.label(RichText::new("No shader program in this blob.").weak());
        return;
    };
    let (lines, body) = (&held.0, held.1);

    let folded = state.with("declarations");
    let mut hide = ui
        .data(|data| data.get_temp::<bool>(folded))
        .unwrap_or(false);
    let from = match hide {
        true => body,
        false => 0,
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{} lines", lines.len() - from))
                .weak()
                .small(),
        );
        if ui.small_button("Copy").clicked() {
            // What is on screen, so a fold takes the declarations out of the clipboard too.
            ui.ctx().copy_text(lines[from..].join("\n"));
        }
        if body > 0 {
            ui.checkbox(&mut hide, RichText::new("Hide declarations").small());
        }
        if source {
            ui.label(
                RichText::new("compiles, but not guaranteed to be perfect")
                    .weak()
                    .small(),
            );
        }
    });
    ui.data_mut(|data| data.insert_temp(folded, hide));
    listing(
        ui,
        "shader_code",
        lines,
        from,
        match source {
            true => "HLSL",
            false => "DXBC",
        },
    );
}

/// The code itself, numbered from `from` and drawn on the theme's own surface.
pub fn listing(ui: &mut egui::Ui, salt: &str, lines: &[String], from: usize, language: &str) {
    let lines = &lines[from.min(lines.len())..];

    let theme = CODE_SYNTAX_THEME.get(ui.ctx());
    // A theme's colors are chosen against its own background, so the code is drawn on that rather
    // than on the panel; the gutter takes the theme's text color for the same reason. Without one
    // declared, this is the surface the schema editor's own code sits on.
    let (fill, ink) = theme
        .surface()
        .unwrap_or_else(|| (ui.visuals().text_edit_bg_color(), ui.visuals().text_color()));
    let height = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::Frame::new()
        .fill(fill)
        .inner_margin(MARGIN)
        .corner_radius(ui.visuals().menu_corner_radius)
        .show(ui, |ui| {
            // Only the lines on screen are laid out, which is what keeps a shader of thirteen hundred
            // of them from building a job for every one each frame. The grammars carry no state from
            // one line to the next, so a line highlighted alone reads as it would in a whole-file
            // pass.
            ScrollArea::both()
                .id_salt(salt)
                // What is left of the panel, which is the point of moving everything else out of it.
                .max_height(ui.available_height().max(height * 12.0))
                .auto_shrink([false, true])
                .show_rows(ui, height, lines.len(), |ui, rows| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    for (offset, line) in lines[rows.clone()].iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            // Selection runs across labels, so a gutter that takes part in it lands
                            // in whatever is dragged out. Only the code answers to the mouse.
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("{:>5}  ", from + rows.start + offset))
                                        .monospace()
                                        .color(ink.gamma_multiply(0.5)),
                                )
                                .selectable(false),
                            );
                            let job = highlight(ui.ctx(), ui.style(), &theme, line, language);
                            ui.add(egui::Label::new(job).selectable(true));
                        });
                    }
                });
        });
}

/// What this shader's registers are called, so a reading names them rather than their slots.
fn names(naming: &Naming, shader: &Shader, blob: &[u8]) -> hlsl::Names {
    let mut names = hlsl::Names::default();
    for binding in &shader.bindings {
        let Some(name) = naming.resources.get(&binding.id) else {
            continue;
        };
        match binding.register {
            Register::Texture => {
                names.textures.insert(binding.slot, name.clone());
            }
            Register::Sampler => {
                names.samplers.insert(binding.slot, name.clone());
            }
            Register::Constant => {
                let fields = match packed(naming, binding) {
                    Some(packed) => packed
                        .owners
                        .iter()
                        .enumerate()
                        .flat_map(|(register, here)| {
                            here.iter().filter_map(move |owner| {
                                Some(hlsl::Field::packed(
                                    owner.name.clone(),
                                    owner.declared?,
                                    register as u32,
                                    owner.mask,
                                ))
                            })
                        })
                        .collect(),
                    None => naming
                        .layouts
                        .get(&binding.id)
                        .map(|members| members.iter().map(described).collect())
                        .unwrap_or_default(),
                };
                names
                    .constants
                    .insert(binding.slot, hlsl::Buffer::new(name.clone(), fields));
            }
        }
    }

    // The signatures name the interpolators, which the file's tables say nothing about.
    for chunk in dxbc::scan_dxbc(blob)
        .iter()
        .flat_map(|container| &container.chunks)
    {
        let (into, signature) = match chunk.parse() {
            ChunkData::InputSignature(signature) => (&mut names.inputs, signature),
            ChunkData::OutputSignature(signature) => (&mut names.outputs, signature),
            _ => continue,
        };
        for element in &signature.elements {
            into.entry(element.register).or_insert_with(|| {
                hlsl::Semantic::new(
                    &element.semantic_name,
                    element.semantic_index,
                    element.component_type,
                    element.mask,
                )
            });
        }
    }
    names
}

/// A field as the bytecode's reflection described it.
fn described(member: &Member) -> hlsl::Field {
    hlsl::Field::described(
        member.name.clone(),
        member.kind.clone(),
        member.offset,
        member.size,
    )
}

/// The file's own account of a buffer, where this binding is the one it describes.
fn packed<'a>(naming: &'a Naming, binding: &Binding) -> Option<&'a super::Packed> {
    naming
        .packed
        .as_ref()
        .filter(|packed| packed.buffer == binding.id)
}

/// A register reference in a line of disassembly: `cb0[6]`, `t3`, `s1`.
fn reference(line: &str, at: usize) -> Option<(Register, u16, Option<u16>, usize)> {
    let rest = &line[at..];
    let (register, tag) = [
        (Register::Constant, "cb"),
        (Register::Texture, "t"),
        (Register::Sampler, "s"),
    ]
    .into_iter()
    .find(|(_, tag)| rest.starts_with(tag))?;

    let digits = rest[tag.len()..]
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len() - tag.len());
    if digits == 0 {
        return None;
    }
    let slot = rest[tag.len()..tag.len() + digits].parse().ok()?;
    let mut end = tag.len() + digits;

    // A constant buffer reference carries the register it reads, which is what names the field.
    let index = match rest[end..].strip_prefix('[') {
        Some(inner) => {
            let close = inner.find(']')?;
            end += close + 2;
            inner[..close].parse().ok()
        }
        None => None,
    };
    Some((register, slot, index, at + end))
}

/// What a shader's own bindings say a register reference is.
///
/// `cb0[6]` is the buffer this shader put at slot zero, read at its sixth vec4. For most buffers the
/// bytecode's reflection names the field sitting there; for the one it leaves as a bare array,
/// whatever the file says occupies that register is named instead.
fn explain(
    naming: &Naming,
    shader: &Shader,
    at: (Register, u16, Option<u16>),
    swizzle: &str,
) -> Option<String> {
    let (register, slot, index) = at;
    let binding = shader
        .bindings
        .iter()
        .find(|binding| binding.register == register && binding.slot == slot)?;
    let name = naming.resources.get(&binding.id)?;

    let Some(index) = index else {
        return Some(name.clone());
    };

    if let Some(packed) = packed(naming, binding) {
        // Several parameters share a register, so the swizzle decides which of them a line actually
        // reads. Without one, every component is in play.
        let read = match swizzle.is_empty() {
            true => 0xF,
            false => swizzle
                .chars()
                .filter_map(|component| "xyzw".find(component))
                .fold(0, |mask, at| mask | 1 << at),
        };
        let owners: Vec<&str> = packed
            .owners
            .get(usize::from(index))?
            .iter()
            .filter(|owner| owner.mask & read != 0)
            .map(|owner| owner.name.as_str())
            .collect();
        return match owners.is_empty() {
            true => Some(format!("{name}[{index}]")),
            false => Some(format!("{name}: {}", owners.join(", "))),
        };
    }

    // Fields are laid out in order, so the one covering a register is the last starting at or
    // before it.
    let field = naming
        .layouts
        .get(&binding.id)?
        .iter()
        .rfind(|member| member.offset / 16 <= u32::from(index))?;
    Some(format!("{name}.{}", field.name))
}

/// The disassembly with a trailing comment naming what each line touches.
fn annotate(naming: &Naming, shader: &Shader, text: &str) -> Vec<String> {
    text.lines()
        .map(|line| {
            let mut seen: Vec<String> = Vec::new();
            let mut at = 0;
            // On a declaration the bracket is the buffer's size, not a register it reads, so only
            // the buffer itself is named.
            let declaring = line.trim_start().starts_with("dcl_");
            while at < line.len() {
                if !line.is_char_boundary(at) {
                    at += 1;
                    continue;
                }
                // Only at a token start, so the `s` of `mors` is not read as sampler zero.
                let boundary = at == 0 || !line.as_bytes()[at - 1].is_ascii_alphanumeric();
                match boundary.then(|| reference(line, at)).flatten() {
                    Some((register, slot, index, end)) => {
                        let index = match declaring {
                            true => None,
                            false => index,
                        };
                        let swizzle = line[end..]
                            .strip_prefix('.')
                            .map(|rest| {
                                let end = rest
                                    .find(|c: char| !"xyzw".contains(c))
                                    .unwrap_or(rest.len());
                                &rest[..end]
                            })
                            .unwrap_or_default();
                        if let Some(text) =
                            explain(naming, shader, (register, slot, index), swizzle)
                            && !seen.contains(&text)
                        {
                            seen.push(text);
                        }
                        at = end;
                    }
                    None => at += 1,
                }
            }
            match seen.is_empty() {
                true => line.to_owned(),
                false => format!("{line}  // {}", seen.join(", ")),
            }
        })
        .collect()
}
