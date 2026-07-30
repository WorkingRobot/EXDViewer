//! The shader list: which of a package's thousands of shaders to read, and what one of them is for.

use egui::{RichText, ScrollArea, vec2};

use super::super::hashed;
use super::super::shader::{Register, code, named};
use super::Rendered;
use crate::assets::Bytes;

/// Key and value pairs across one row of the condition filter. More than this and a long key name
/// pushes the row off a narrow panel.
const CONDITION_PAIRS: usize = 2;
/// Width every condition box takes, so that a column of them lines up.
const CONDITION_WIDTH: f32 = 110.0;

/// Rows of the shader list kept on screen at once, which bounds it against a package holding six
/// thousand shaders.
const LIST_ROWS: usize = 8;

pub fn ui(ui: &mut egui::Ui, package: &Rendered, bytes: &[u8]) {
    let (mut stage, mut pass, mut picked) = ui
        .data(|data| data.get_temp::<(usize, usize, usize)>(package.state))
        .unwrap_or((0, 0, 0));

    ui.horizontal_wrapped(|ui| {
        if ui
            .selectable_label(stage == 0, format!("All ({})", package.shaders.len()))
            .clicked()
        {
            stage = 0;
        }
        for (index, (name, count, size)) in package.stages.iter().enumerate() {
            let label = format!("{name} ({count}, {})", Bytes(*size));
            if ui.selectable_label(stage == index + 1, label).clicked() {
                stage = index + 1;
            }
        }
    });
    let slot = package.state.with("conditions");
    let mut chosen: Vec<Option<u32>> = ui
        .data(|data| data.get_temp::<Vec<Option<u32>>>(slot))
        .filter(|held| held.len() == package.keys.columns.len())
        .unwrap_or_else(|| vec![None; package.keys.columns.len()]);
    if package.keys.columns.iter().any(|key| key.values.len() > 1) {
        let set = chosen.iter().flatten().count();
        let title = match set {
            0 => "Conditions".to_owned(),
            set => format!("Conditions ({set} set)"),
        };
        egui::CollapsingHeader::new(title)
            .id_salt("shpk_conditions")
            .show(ui, |ui| {
                let filterable: Vec<_> = package
                    .keys
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(_, key)| key.values.len() > 1)
                    .collect();
                egui::Grid::new("shpk_conditions_grid")
                    .num_columns(CONDITION_PAIRS * 2)
                    .show(ui, |ui| {
                        for (at, (index, key)) in filterable.iter().enumerate() {
                            let held = chosen[*index];
                            let selected = held
                                .and_then(|value| {
                                    key.values.iter().find(|(_, held)| *held == value)
                                })
                                .map_or("any", |(short, _)| short.as_str());
                            // The grid puts every box at its column's edge; the name only needs to
                            // stand as tall as one so the two share a line.
                            ui.allocate_ui_with_layout(
                                vec2(0.0, ui.spacing().interact_size.y),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| hashed(ui, "Key", &key.name, key.id, held.is_none()),
                            );
                            egui::ComboBox::from_id_salt(("shpk_condition", key.id, index))
                                .width(CONDITION_WIDTH)
                                .selected_text(RichText::new(selected).monospace())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut chosen[*index], None, "any");
                                    for (short, value) in &key.values {
                                        ui.selectable_value(
                                            &mut chosen[*index],
                                            Some(*value),
                                            short,
                                        );
                                    }
                                });
                            if at % CONDITION_PAIRS == CONDITION_PAIRS - 1 {
                                ui.end_row();
                            }
                        }
                        if !filterable.len().is_multiple_of(CONDITION_PAIRS) {
                            ui.end_row();
                        }
                    });
                if set > 0 && ui.small_button("Clear").clicked() {
                    chosen = vec![None; package.keys.columns.len()];
                }
            });
    }
    ui.data_mut(|data| data.insert_temp(slot, chosen.clone()));

    if !package.keys.passes.is_empty() {
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(pass == 0, "Any pass").clicked() {
                pass = 0;
            }
            for (index, row) in package.keys.passes.iter().enumerate() {
                let label = format!("{} ({})", row.name, row.shaders.len());
                if ui.selectable_label(pass == index + 1, label).clicked() {
                    pass = index + 1;
                }
            }
        });
    }
    ui.add_space(4.0);

    // Zero is the unfiltered chip, so the stages are offset by one.
    let shown = stage
        .checked_sub(1)
        .and_then(|index| package.stages.get(index))
        .map(|(name, _, _)| *name);
    // Zero is the unfiltered chip here too.
    let drawn = pass
        .checked_sub(1)
        .and_then(|index| package.keys.passes.get(index))
        .map(|row| row.shaders.as_slice());
    let listed: Vec<usize> = package
        .shaders
        .iter()
        .enumerate()
        .filter(|(_, shader)| shown.is_none_or(|name| shader.stage == name))
        .filter(|(index, _)| drawn.is_none_or(|shaders| shaders.binary_search(index).is_ok()))
        .filter(|(index, _)| {
            let defines = package.keys.defines.get(*index);
            chosen.iter().enumerate().all(|(column, want)| match want {
                None => true,
                Some(value) => defines.is_some_and(|held| held.contains(&(column, *value))),
            })
        })
        .map(|(index, _)| index)
        .collect();
    // Narrowing to a stage the picked shader is not in would otherwise leave the reading below
    // showing something the list no longer offers.
    if listed.binary_search(&picked).is_err() {
        picked = listed.first().copied().unwrap_or(0);
    }

    let varying = package.keys.discriminating(&listed);
    let height = ui.text_style_height(&egui::TextStyle::Monospace) + ui.spacing().item_spacing.y;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ScrollArea::both()
            .id_salt("shpk_shader_list")
            .max_height(height * LIST_ROWS as f32)
            .auto_shrink([false, true])
            .show_rows(ui, height, listed.len(), |ui, rows| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                for &index in &listed[rows] {
                    let shader = &package.shaders[index];
                    // Laid out in fixed columns so that moving down the list, the same key stays
                    // under the same place and a difference shows itself.
                    let mut flags = String::new();
                    for (column, width) in &varying {
                        let value = package.keys.condition(index, *column).unwrap_or("-");
                        flags.push_str(&format!("{value:<width$} ", width = width));
                    }
                    let label = format!(
                        "#{index:<5} {:<7}{:>8} {:>3} bound   {}",
                        shader.stage,
                        Bytes(shader.blob.len()).to_string(),
                        shader.bindings.len(),
                        flags.trim_end()
                    );
                    let row = ui
                        .selectable_label(picked == index, RichText::new(label).monospace())
                        .on_hover_ui(|ui| tooltip(ui, package, index));
                    if row.clicked() {
                        picked = index;
                    }
                }
            });
    });
    ui.data_mut(|data| data.insert_temp(package.state, (stage, pass, picked)));

    let Some(shader) = package.shaders.get(picked) else {
        return;
    };
    ui.add_space(4.0);
    code::ui(
        ui,
        package.state,
        &format!("Shader #{picked}"),
        shader,
        &package.naming,
        bytes,
    );
}

/// What a list row cannot hold: where the shader sits, what it binds, which passes reach it, and
/// every condition it was compiled under rather than only the ones that vary.
fn tooltip(ui: &mut egui::Ui, package: &Rendered, index: usize) {
    let Some(shader) = package.shaders.get(index) else {
        return;
    };
    ui.label(
        RichText::new(format!("Shader #{index}  {}", shader.stage))
            .monospace()
            .strong(),
    );
    ui.label(
        RichText::new(format!(
            "{} at {:#X}",
            Bytes(shader.blob.len()),
            shader.blob.start
        ))
        .weak()
        .small(),
    );

    let reached: Vec<&str> = package
        .keys
        .passes
        .iter()
        .filter(|pass| pass.shaders.binary_search(&index).is_ok())
        .map(|pass| pass.name.as_str())
        .collect();
    if !reached.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new("Drawn in").weak().small());
        for pass in reached {
            ui.label(RichText::new(pass).monospace());
        }
    }

    if !shader.bindings.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new("Binds").weak().small());
        egui::Grid::new(("shpk_tooltip_binds", index))
            .num_columns(2)
            .show(ui, |ui| {
                for binding in &shader.bindings {
                    let prefix = match binding.register {
                        Register::Constant => "cb",
                        Register::Sampler => "s",
                        Register::Texture => "t",
                    };
                    ui.label(
                        RichText::new(format!("{prefix}{}", binding.slot))
                            .monospace()
                            .weak(),
                    );
                    ui.label(
                        RichText::new(
                            package
                                .naming
                                .resources
                                .get(&binding.id)
                                .cloned()
                                .unwrap_or_else(|| named(binding.id)),
                        )
                        .monospace(),
                    );
                    ui.end_row();
                }
            });
    }

    // Only what a variant set, since the grid under the list already carries the rest.
    let set: Vec<(&str, &str)> = package
        .keys
        .defines
        .get(index)
        .into_iter()
        .flatten()
        .filter_map(|(column, value)| {
            let key = package.keys.columns.get(*column)?;
            if key.default == *value {
                return None;
            }
            let (short, _) = key.values.iter().find(|(_, held)| held == value)?;
            Some((key.name.as_str(), short.as_str()))
        })
        .collect();
    if !set.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new("Compiled for").weak().small());
        egui::Grid::new(("shpk_tooltip_defines", index))
            .num_columns(2)
            .show(ui, |ui| {
                for (key, value) in set {
                    ui.label(RichText::new(key).monospace().weak());
                    ui.label(RichText::new(value).monospace());
                    ui.end_row();
                }
            });
    }
}
