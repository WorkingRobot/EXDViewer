//! One pass as a single source, instead of one shader at a time.

use std::sync::Arc;

use egui::RichText;
use ironworks::file::shpk;

use super::super::heading;
use super::super::shader::code;
use super::Rendered;

/// What a merge produced, or why it did not.
type Outcome = Result<Arc<shadermerge::Merged>, String>;

/// Whether the pass is being read as one source rather than a shader at a time. The filtering and
/// the shader list have nothing to say about a merged source, so they come off while it is on.
pub fn reading(ui: &egui::Ui, state: egui::Id) -> bool {
    ui.data(|data| data.get_temp::<bool>(state.with("merged_mode")))
        .unwrap_or(false)
}

pub fn set_reading(ui: &mut egui::Ui, state: egui::Id, held: bool) {
    ui.data_mut(|data| data.insert_temp(state.with("merged_mode"), held));
}

/// The merged source for one stage of one pass.
///
/// Merging reads and canonicalises every shader of the pass at once, which for the largest of them
/// is most of a second on a native build and longer in a browser, so it happens when asked for
/// rather than when the pass is picked, and is held afterwards.
pub fn ui(ui: &mut egui::Ui, package: &Rendered, bytes: &[u8], stage: usize, pass: u32) {
    let slot = package.state.with("merged");
    let held = ui.data(|data| data.get_temp::<((usize, u32), Outcome)>(slot));
    let at = (stage, pass);
    let outcome = match held {
        Some((was, outcome)) if was == at => Some(outcome),
        _ => None,
    };

    // Asking for the reading is what asks for the merge; there is nothing else the reader could
    // have meant by choosing it, and a second button to confirm would only be in the way.
    let outcome = outcome.unwrap_or_else(|| {
        // Parsing again costs a walk of the tables, which is nothing beside reading every shader of
        // the pass, and it saves `Rendered` holding a borrow of the file.
        let fresh: Outcome = shpk::ShaderPackage::parse(bytes)
            .map_err(|why| why.to_string())
            .and_then(|package| {
                shadermerge::pass(&package, bytes, stage, pass)
                    .map(Arc::new)
                    .map_err(|why| why.to_string())
            });
        ui.data_mut(|data| data.insert_temp(slot, (at, fresh.clone())));
        fresh
    });

    let merged = match &outcome {
        Ok(merged) => merged,
        Err(why) => {
            heading(ui, "Merged pass");
            ui.label(RichText::new(why).weak());
            return;
        }
    };

    // The header is the slot table and the variant list. It is worth having, but it is not what the
    // reader came for, so it starts folded.
    let folded = package.state.with("merged_header");
    let mut hide = ui
        .data(|data| data.get_temp::<bool>(folded))
        .unwrap_or(true);
    let from = match hide {
        true => merged.body,
        false => 0,
    };

    let reading = package.state.with("merged_source");
    let mut source = ui
        .data(|data| data.get_temp::<bool>(reading))
        .unwrap_or(true);
    ui.horizontal(|ui| {
        heading(ui, "Merged pass");
        if ui.selectable_label(source, "HLSL").clicked() {
            source = true;
        }
        if ui.selectable_label(!source, "Assembly").clicked() {
            source = false;
        }
    });
    ui.data_mut(|data| data.insert_temp(reading, source));

    let lines = match source {
        true => &merged.lines,
        false => &merged.asm,
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{} lines", lines.len() - from))
                .weak()
                .small(),
        );
        if ui.small_button("Copy").clicked() {
            // What is on screen, so a fold takes the header out of the clipboard too.
            ui.ctx().copy_text(lines[from..].join("\n"));
        }
        ui.checkbox(&mut hide, RichText::new("Hide header").small());
        ui.label(
            RichText::new(format!(
                "{} shaders, {} key combinations, {} #if regions",
                merged.blobs, merged.variants, merged.regions
            ))
            .weak()
            .small(),
        );
    });
    ui.data_mut(|data| data.insert_temp(folded, hide));

    code::listing(
        ui,
        "shpk_merged_code",
        lines,
        from,
        match source {
            true => "HLSL",
            false => "DXBC",
        },
    );
}
