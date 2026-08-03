//! `.avfx` effects: what the game spawns, over what span of frames, and the curves driving it.
//!
//! An effect is a tree of tagged blocks. Its own settings are named, and everything below them goes
//! by the four-character tag it is written under, so the tree here is the file as it stands rather
//! than a reading of it. What is read is the shape: schedulers start timelines, a timeline runs
//! items over a span of frames, an emitter spawns particles and further emitters, and anything
//! animated is a curve.
//!
//! Nothing in the file states the rate its frames are counted at, so the rate is a setting and a
//! curve reads out in both.

use std::collections::HashSet;
use std::io::Cursor;

use anyhow::Result;
use egui::{RichText, ScrollArea, Sense, Vec2, collapsing_header::paint_default_icon, vec2};
use ironworks::file::{
    File,
    avfx::{Avfx, Block, Clip, Item, Model, Payload},
};

use super::{Preview, facts, headers, heading, link, section};
use crate::{settings::AVFX_FRAME_RATE, utils::file_name};

mod curve;

use curve::Curve;

/// Space each level of the tree is set in by.
const INDENT: f32 = 12.0;

/// Room the expander takes, kept on rows without one so their labels still line up.
const TRIANGLE: f32 = 12.0;

/// Side of the square a color is drawn in.
const CHIP: f32 = 10.0;

/// One row of the tree.
struct Row {
    depth: u8,

    label: String,

    /// What the row says beside its label, drawn weakly.
    detail: String,

    /// The file this row names, drawn as a link.
    asset: Option<String>,

    /// The curve this row draws, indexed into [`Rendered::curves`].
    curve: Option<usize>,

    /// Everything the row carries, for the rows the file gives a shape of their own. A block row
    /// says all it has to say in its label and detail, and leaves this empty.
    fields: Vec<(&'static str, String)>,

    /// Whether the row starts expanded.
    open: bool,
}

pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    settings: Vec<(&'static str, String)>,
    rows: Vec<Row>,
    curves: Vec<Curve>,
    /// Where the open rows and the selected one are kept, since drawing takes the file by
    /// reference.
    state: egui::Id,
}

/// The first block carrying `name`.
fn find<'a>(blocks: &'a [Block], name: &str) -> Option<&'a Block> {
    blocks.iter().find(|block| block.name() == name)
}

fn integer(blocks: &[Block], name: &str) -> Option<i32> {
    find(blocks, name)?.i32()
}

/// An index into one of the effect's lists, which is written `-1` where there is none.
fn reference(value: Option<i32>) -> String {
    match value {
        Some(-1) | None => "none".to_owned(),
        Some(index) => index.to_string(),
    }
}

/// What a row says about an entry that can be switched off.
fn disabled(blocks: &[Block], name: &str) -> &'static str {
    match find(blocks, name).and_then(Block::bool) {
        Some(false) => "  off",
        _ => "",
    }
}

/// A four-byte payload, read as whichever of the two the bits can be. An integer small enough for
/// this format to be writing lands in the exponent range a float leaves for zero, so a normal float
/// is a float and anything else is an integer.
fn scalar(bytes: [u8; 4]) -> String {
    let value = f32::from_le_bytes(bytes);
    match value.is_normal() {
        true => value.to_string(),
        false => i32::from_le_bytes(bytes).to_string(),
    }
}

/// A block's payload as it reads.
fn payload(block: &Block, bytes: &[u8]) -> String {
    let name = block.name();
    match (name.as_str(), bytes) {
        ("SdNm" | "Name", _) => block.text().unwrap_or_default(),
        (_, [byte]) => byte.to_string(),
        (_, [a, b, c, d]) => scalar([*a, *b, *c, *d]),
        (_, []) => String::new(),
        _ => format!("{} bytes", bytes.len()),
    }
}

fn axes(values: [f32; 3]) -> String {
    format!("{:.3}, {:.3}, {:.3}", values[0], values[1], values[2])
}

fn numbers<const N: usize>(values: [impl ToString; N]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Default)]
struct Build {
    rows: Vec<Row>,
    curves: Vec<Curve>,
}

impl Build {
    fn row(
        &mut self,
        depth: usize,
        label: impl Into<String>,
        detail: impl Into<String>,
    ) -> &mut Row {
        self.rows.push(Row {
            depth: depth as u8,
            label: label.into(),
            detail: detail.into(),
            asset: None,
            curve: None,
            fields: Vec::new(),
            open: false,
        });
        self.rows.last_mut().unwrap()
    }

    /// One of the effect's lists, which is left out where the file holds none of it.
    fn list(&mut self, label: &str, count: usize) -> bool {
        if count > 0 {
            self.row(0, label, count.to_string()).open = true;
        }
        count > 0
    }

    /// One block and everything under it. A curve collapses into the row it sits on: the tags
    /// beside its keys say what happens either side of them, and the plot says both.
    fn block(&mut self, block: &Block, depth: usize) {
        if let Some(curve) = curve::read(block) {
            let (detail, index) = (curve.summary(), self.curves.len());
            self.curves.push(curve);
            self.row(depth, block.name().as_str(), detail).curve = Some(index);
            return;
        }

        match block.payload() {
            Payload::Blocks(blocks) => {
                self.row(depth, block.name().as_str(), String::new());
                for child in blocks {
                    self.block(child, depth + 1);
                }
            }
            Payload::Keys(keys) => {
                self.row(depth, block.name().as_str(), format!("{} keys", keys.len()));
            }
            Payload::Bytes(bytes) => {
                let value = payload(block, bytes);
                let row = self.row(depth, block.name().as_str(), value.clone());
                if block.name() == "SdNm" && !value.is_empty() {
                    row.detail = String::new();
                    row.asset = Some(value);
                }
            }
        }
    }

    /// One entry of a scheduler, timeline or emitter list.
    fn item(&mut self, label: String, detail: String, item: &Item, depth: usize) {
        self.row(depth, label, detail);
        for block in item.blocks() {
            self.block(block, depth + 1);
        }
    }

    /// The effect's own settings, which start collapsed: the panel already reads out the ones that
    /// are named.
    fn settings(&mut self, file: &Avfx) {
        if file.properties().is_empty() {
            return;
        }
        self.row(0, "Settings", file.properties().len().to_string());
        for block in file.properties() {
            self.block(block, 1);
        }
    }

    fn schedulers(&mut self, file: &Avfx) {
        if !self.list("Schedulers", file.schedulers().len()) {
            return;
        }
        for (index, scheduler) in file.schedulers().iter().enumerate() {
            let detail = format!(
                "{} items, {} triggers",
                scheduler.items().len(),
                scheduler.triggers().len()
            );
            self.row(1, format!("Scheduler {index}"), detail);
            for block in scheduler.properties() {
                self.block(block, 2);
            }
            for (label, items) in [
                ("Item", scheduler.items()),
                ("Trigger", scheduler.triggers()),
            ] {
                for (index, item) in items.iter().enumerate() {
                    let blocks = item.blocks();
                    let detail = format!(
                        "timeline {}  start {}{}",
                        reference(integer(blocks, "TlNo")),
                        reference(integer(blocks, "StTm")),
                        disabled(blocks, "bEna")
                    );
                    self.item(format!("{label} {index}"), detail, item, 2);
                }
            }
        }
    }

    fn timelines(&mut self, file: &Avfx) {
        if !self.list("Timelines", file.timelines().len()) {
            return;
        }
        for (index, timeline) in file.timelines().iter().enumerate() {
            let properties = timeline.properties();
            let detail = format!(
                "loop {}..{}  {} items",
                reference(integer(properties, "LpSt")),
                reference(integer(properties, "LpEd")),
                timeline.items().len()
            );
            self.row(1, format!("Timeline {index}"), detail).fields = vec![
                (
                    "Loop",
                    format!(
                        "{}..{}",
                        reference(integer(properties, "LpSt")),
                        reference(integer(properties, "LpEd"))
                    ),
                ),
                ("Binder", reference(integer(properties, "BnNo"))),
                ("Items", timeline.items().len().to_string()),
                ("Clips", timeline.clips().len().to_string()),
            ];
            for block in properties {
                self.block(block, 2);
            }
            for (index, item) in timeline.items().iter().enumerate() {
                let blocks = item.blocks();
                let detail = format!(
                    "{}..{}  emitter {}  binder {}  effector {}{}",
                    reference(integer(blocks, "StTm")),
                    reference(integer(blocks, "EdTm")),
                    reference(integer(blocks, "EmNo")),
                    reference(integer(blocks, "BdNo")),
                    reference(integer(blocks, "EfNo")),
                    disabled(blocks, "bEna")
                );
                self.item(format!("Item {index}"), detail, item, 2);
            }
            for (index, clip) in timeline.clips().iter().enumerate() {
                self.clip(index, clip);
            }
        }
    }

    fn clip(&mut self, index: usize, clip: &Clip) {
        let kind = format!("{:?}", clip.kind());
        self.row(2, format!("Clip {index}"), &kind).fields = vec![
            ("Kind", kind.clone()),
            ("Integers", numbers(clip.integers())),
            (
                "Floats",
                numbers(clip.floats().map(|value| format!("{value:.3}"))),
            ),
        ];
    }

    fn emitters(&mut self, file: &Avfx) {
        if !self.list("Emitters", file.emitters().len()) {
            return;
        }
        for (index, emitter) in file.emitters().iter().enumerate() {
            let properties = emitter.properties();
            let detail = format!(
                "kind {}  life {}  {} particles, {} emitters",
                reference(integer(properties, "EVT")),
                reference(integer(properties, "Life")),
                emitter.particles().len(),
                emitter.emitters().len()
            );
            self.row(1, format!("Emitter {index}"), detail).fields = vec![
                (
                    "Kind",
                    format!("EVT {}", reference(integer(properties, "EVT"))),
                ),
                ("Life", reference(integer(properties, "Life"))),
                (
                    "Loop",
                    format!(
                        "{}..{}",
                        reference(integer(properties, "LpSt")),
                        reference(integer(properties, "LpEd"))
                    ),
                ),
                ("Sound", reference(integer(properties, "SdNo"))),
                ("Particles", emitter.particles().len().to_string()),
                ("Emitters", emitter.emitters().len().to_string()),
            ];
            for block in properties {
                self.block(block, 2);
            }
            for (label, items) in [
                ("Particle", emitter.particles()),
                ("Emitter", emitter.emitters()),
            ] {
                for (index, item) in items.iter().enumerate() {
                    let blocks = item.blocks();
                    let detail = format!(
                        "{} {}{}",
                        label.to_lowercase(),
                        reference(integer(blocks, "TgtB")),
                        disabled(blocks, "bEnb")
                    );
                    self.item(format!("{label} {index}"), detail, item, 2);
                }
            }
        }
    }

    /// The three lists the file writes as one block each, which are read only by their tags.
    fn blocks(&mut self, label: &str, kind: &str, blocks: &[Block]) {
        if !self.list(&format!("{label}s"), blocks.len()) {
            return;
        }
        for (index, block) in blocks.iter().enumerate() {
            let inner = block.blocks();
            // An effector carries no life of its own, where a particle and a binder both do.
            let life = integer(inner, "Life");
            let mut fields = vec![(
                "Kind",
                format!("{kind} {}", reference(integer(inner, kind))),
            )];
            fields.extend(life.map(|life| ("Life", life.to_string())));
            fields.push((
                "Loop",
                format!(
                    "{}..{}",
                    reference(integer(inner, "LpSt")),
                    reference(integer(inner, "LpEd"))
                ),
            ));

            let detail = match life {
                Some(life) => format!("kind {}  life {life}", reference(integer(inner, kind))),
                None => format!("kind {}", reference(integer(inner, kind))),
            };
            self.row(1, format!("{label} {index}"), detail).fields = fields;
            for child in inner {
                self.block(child, 2);
            }
        }
    }

    fn textures(&mut self, file: &Avfx) {
        if !self.list("Textures", file.textures().len()) {
            return;
        }
        for (index, path) in file.textures().iter().enumerate() {
            self.row(1, format!("Tex {index}"), String::new()).asset = Some(path.clone());
        }
    }

    fn models(&mut self, file: &Avfx) {
        if !self.list("Models", file.models().len()) {
            return;
        }
        for (index, model) in file.models().iter().enumerate() {
            self.row(1, format!("Model {index}"), summary(model)).fields = vec![
                ("Vertices", model.vertices().len().to_string()),
                ("Triangles", model.triangles().len().to_string()),
                ("Emit points", model.emit_vertices().len().to_string()),
            ];
        }
    }
}

/// A model, which the file holds whole rather than naming a `.mdl` beside it.
fn summary(model: &Model) -> String {
    format!(
        "{} vertices, {} triangles, {} emit points",
        model.vertices().len(),
        model.triangles().len(),
        model.emit_vertices().len()
    )
}

fn identity(file: &Avfx) -> Vec<(&'static str, String)> {
    vec![
        ("Version", format!("{:#010x}", file.version())),
        ("Schedulers", file.schedulers().len().to_string()),
        ("Timelines", file.timelines().len().to_string()),
        ("Emitters", file.emitters().len().to_string()),
        ("Particles", file.particles().len().to_string()),
        ("Effectors", file.effectors().len().to_string()),
        ("Binders", file.binders().len().to_string()),
        ("Textures", file.textures().len().to_string()),
        ("Models", file.models().len().to_string()),
    ]
}

/// The effect's own settings, leaving out the ones sitting at a value that does nothing.
fn settings(file: &Avfx) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    rows.extend(file.draw_layer().map(|v| ("Draw layer", format!("{v:?}"))));
    rows.extend(file.draw_order().map(|v| ("Draw order", format!("{v:?}"))));
    rows.extend(
        file.directional_light_source()
            .map(|v| ("Directional light", format!("{v:?}"))),
    );
    for (label, source) in ["Point light 1", "Point light 2"]
        .into_iter()
        .zip(file.point_light_sources())
    {
        rows.extend(source.map(|v| (label, format!("{v:?}"))));
    }

    if file.clip_box_enabled() == Some(true) {
        rows.extend(file.clip_box().map(|v| ("Clip box", axes(v))));
        rows.extend(file.clip_box_size().map(|v| ("Clip box size", axes(v))));
    }
    if file.clip_own_setting() == Some(true) {
        rows.extend(
            file.near_clip()
                .map(|(from, to)| ("Near clip", format!("{from:.3} to {to:.3}"))),
        );
        rows.extend(
            file.far_clip()
                .map(|(from, to)| ("Far clip", format!("{from:.3} to {to:.3}"))),
        );
    }
    if file.global_fog_enabled() == Some(true) {
        rows.extend(
            file.global_fog_influence()
                .map(|v| ("Global fog", format!("{v:.3}"))),
        );
    }

    rows.extend(
        file.revised_position()
            .filter(|v| *v != [0.0; 3])
            .map(|v| ("Position", axes(v))),
    );
    rows.extend(
        file.revised_rotation()
            .filter(|v| *v != [0.0; 3])
            .map(|v| ("Rotation", axes(v))),
    );
    rows.extend(
        file.revised_scale()
            .filter(|v| *v != [1.0; 3])
            .map(|v| ("Scale", axes(v))),
    );
    rows.extend(
        file.revised_colour()
            .filter(|v| *v != [1.0; 3])
            .map(|v| ("Color", axes(v))),
    );

    for (label, fade) in [
        ("Fade X", file.fade_x()),
        ("Fade Y", file.fade_y()),
        ("Fade Z", file.fade_z()),
    ] {
        rows.extend(
            fade.filter(|fade| fade.enabled())
                .map(|fade| (label, format!("{:.3} to {:.3}", fade.inner(), fade.outer()))),
        );
    }

    for (label, value) in [
        ("Soft particle fade", file.soft_particle_fade_range()),
        ("Sort key offset", file.sort_key_offset()),
        ("Bias Z scale", file.bias_z_max_scale()),
        ("Bias Z distance", file.bias_z_max_distance()),
    ] {
        rows.extend(
            value
                .filter(|v| *v != 0.0)
                .map(|v| (label, format!("{v:.3}"))),
        );
    }

    for (label, value) in [
        ("Delay fast particle", file.is_delay_fast_particle()),
        ("Fit ground", file.is_fit_ground()),
        ("Transform skip", file.is_transform_skip()),
        ("All stop on hide", file.is_all_stop_on_hide()),
        ("Can be clipped out", file.can_be_clipped_out()),
        ("Camera space", file.is_camera_space()),
        ("Full env light", file.is_full_env_light()),
    ] {
        rows.extend(value.map(|v| {
            (
                label,
                match v {
                    true => "yes",
                    false => "no",
                }
                .to_owned(),
            )
        }));
    }
    rows
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let file = Avfx::read(Cursor::new(bytes.to_vec()))?;

    let mut build = Build::default();
    build.settings(&file);
    build.schedulers(&file);
    build.timelines(&file);
    build.emitters(&file);
    build.blocks("Particle", "PrVT", file.particles());
    build.blocks("Effector", "EfVT", file.effectors());
    build.blocks("Binder", "BnVr", file.binders());
    build.textures(&file);
    build.models(&file);

    log::info!(
        "assets/avfx: {path} {} timelines, {} emitters, {} particles, {} curves",
        file.timelines().len(),
        file.emitters().len(),
        file.particles().len(),
        build.curves.len()
    );

    Ok(Preview::Avfx(Box::new(Rendered {
        identity: identity(&file),
        settings: settings(&file),
        rows: build.rows,
        curves: build.curves,
        state: egui::Id::new(path).with("avfx_tree"),
    })))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) -> Option<String> {
    let mut follow = None;
    let mut open = file.open(ui);
    let mut shown = Vec::new();
    let mut collapsed_at = None;
    for (index, row) in file.rows.iter().enumerate() {
        match collapsed_at {
            Some(depth) if row.depth > depth => continue,
            _ => collapsed_at = None,
        }
        let parent = file.parent(index);
        let expanded = parent && (row.open != open.contains(&index));
        if parent && !expanded {
            collapsed_at = Some(row.depth);
        }
        shown.push((index, expanded));
    }

    section(ui, "Effect");
    let picked = file.selected(ui);
    let mut selected = picked;
    let mut toggled = None;
    // A curve's row carries a sparkline, which is taller than either the text or the expander, and
    // the height handed to the scroll area has to cover the tallest of the three or the rows it
    // places drift out of their own space.
    let height = ui
        .text_style_height(&egui::TextStyle::Monospace)
        .max(TRIANGLE)
        .max(curve::SPARK.y)
        + 2.0 * ui.spacing().button_padding.y
        + ui.spacing().item_spacing.y;
    ScrollArea::vertical()
        .auto_shrink(false)
        .show_rows(ui, height, shown.len(), |ui, range| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for &(index, expanded) in &shown[range] {
                let row = &file.rows[index];
                ui.horizontal(|ui| {
                    ui.add_space(f32::from(row.depth) * INDENT);
                    match file.parent(index) {
                        false => ui.add_space(TRIANGLE),
                        true => {
                            let (_, response) =
                                ui.allocate_exact_size(Vec2::splat(TRIANGLE), Sense::click());
                            let openness = match expanded {
                                true => 1.0,
                                false => 0.0,
                            };
                            paint_default_icon(ui, openness, &response);
                            if response.clicked() {
                                toggled = Some(index);
                            }
                        }
                    }

                    if ui
                        .selectable_label(
                            picked == Some(index),
                            RichText::new(&row.label).monospace(),
                        )
                        .clicked()
                    {
                        selected = Some(index);
                    }
                    if let Some(curve) = row.curve {
                        curve::spark(ui, &file.curves[curve]);
                    }
                    if let Some(asset) = &row.asset
                        && link(ui, file_name(asset), asset)
                    {
                        follow = Some(asset.clone());
                    }
                    if !row.detail.is_empty() {
                        ui.label(RichText::new(&row.detail).monospace().weak());
                    }
                });
            }
        });

    if selected != picked {
        ui.data_mut(|data| data.insert_temp(file.state.with("selected"), selected));
    }
    if let Some(index) = toggled {
        if !open.insert(index) {
            open.remove(&index);
        }
        ui.data_mut(|data| data.insert_temp(file.state, open));
    }
    follow
}

impl Rendered {
    fn open(&self, ui: &egui::Ui) -> HashSet<usize> {
        ui.data(|data| data.get_temp(self.state).unwrap_or_default())
    }

    fn selected(&self, ui: &egui::Ui) -> Option<usize> {
        ui.data(|data| data.get_temp(self.state.with("selected")).flatten())
    }

    /// Whether anything sits under a row. Rows are held parents-first, so the row after one is its
    /// first child where it has any.
    fn parent(&self, index: usize) -> bool {
        self.rows
            .get(index + 1)
            .is_some_and(|next| next.depth > self.rows[index].depth)
    }

    /// The curves a row draws: its own where it is one, and the ones written directly under it
    /// otherwise, which is how a position or a color arrives: one curve an axis.
    fn drawn(&self, index: usize) -> Vec<usize> {
        let row = &self.rows[index];
        if let Some(curve) = row.curve {
            return vec![curve];
        }
        self.rows[index + 1..]
            .iter()
            .take_while(|under| under.depth > row.depth)
            .filter(|under| under.depth == row.depth + 1)
            .filter_map(|under| under.curve)
            .collect()
    }

    pub fn details_ui(&self, ui: &mut egui::Ui, follow: &mut Option<String>) {
        let mut rate = AVFX_FRAME_RATE.get(ui.ctx());
        ui.horizontal(|ui| {
            ui.label(RichText::new("Frame rate").weak());
            if ui
                .add(
                    egui::DragValue::new(&mut rate)
                        .speed(1.0)
                        .range(1.0..=240.0)
                        .suffix(" fps"),
                )
                .changed()
            {
                AVFX_FRAME_RATE.set(ui.ctx(), rate);
            }
        });
        ui.separator();

        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if let Some(index) = self.selected(ui)
                && let Some(row) = self.rows.get(index)
            {
                ui.label(RichText::new(&row.label).strong());
                ui.add_space(4.0);
                if !row.detail.is_empty() {
                    ui.label(RichText::new(&row.detail).monospace());
                    ui.add_space(4.0);
                }
                if let Some(path) = &row.asset
                    && link(ui, path, path)
                {
                    *follow = Some(path.clone());
                }
                if !row.fields.is_empty() {
                    facts(ui, "avfx_selected", &row.fields);
                }

                let drawn = self.drawn(index);
                for (position, curve) in drawn.iter().enumerate() {
                    let curve = &self.curves[*curve];
                    ui.add_space(8.0);
                    ui.separator();
                    if drawn.len() > 1 || curve.name != row.label {
                        heading(ui, &curve.name);
                    }
                    curve_ui(ui, curve, position, rate);
                }

                ui.add_space(8.0);
                ui.separator();
            }

            facts(ui, "avfx_identity", &self.identity);
            if !self.settings.is_empty() {
                ui.add_space(8.0);
                ui.separator();
                heading(ui, "Settings");
                facts(ui, "avfx_settings", &self.settings);
            }
        });
    }
}

/// One curve: the plot, what it does either side of its keys, and every key it holds.
fn curve_ui(ui: &mut egui::Ui, curve: &Curve, position: usize, rate: f32) {
    let range = curve::plot(ui, curve, rate);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("before {:?}, after {:?}", curve.pre, curve.post))
                .monospace()
                .weak(),
        );
        if let Some(random) = curve.random {
            ui.label(RichText::new(format!("{random:?}")).monospace().weak());
        }
    });
    if let Some((low, high)) = range {
        ui.label(
            RichText::new(format!("{low:.3} to {high:.3}"))
                .monospace()
                .weak(),
        );
    }
    ui.add_space(4.0);

    let columns = match curve.color {
        true => 5,
        false => 4,
    };
    egui::Grid::new(("avfx_keys", position))
        .num_columns(columns)
        .striped(true)
        .show(ui, |ui| {
            match curve.color {
                true => headers(ui, &["Frame", "Time", "Kind", "", "Color"]),
                false => headers(ui, &["Frame", "Time", "Kind", "Value"]),
            }
            for key in &curve.keys {
                ui.label(RichText::new(key.time().to_string()).monospace());
                ui.label(
                    RichText::new(curve::seconds(f32::from(key.time()), rate))
                        .monospace()
                        .weak(),
                );
                ui.label(
                    RichText::new(format!("{:?}", key.kind()))
                        .monospace()
                        .weak(),
                );
                match curve.color {
                    true => {
                        let (at, _) = ui.allocate_exact_size(Vec2::splat(CHIP), Sense::hover());
                        ui.painter()
                            .rect_filled(at, 2.0, curve.swatch(f32::from(key.time())));
                        let [r, g, b] = key.data().map(|channel| (channel * 255.0).round() as u8);
                        ui.label(RichText::new(format!("{r}, {g}, {b}")).monospace());
                    }
                    false => {
                        ui.label(RichText::new(format!("{}", key.value())).monospace());
                    }
                }
                ui.allocate_space(vec2(ui.available_width(), 0.0));
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One block: the tag back to front, then its length, then its payload padded out to four.
    fn block(tag: &str, payload: &[u8]) -> Vec<u8> {
        let mut bytes = tag.bytes().rev().collect::<Vec<_>>();
        bytes.resize(4, 0);
        bytes.extend(u32::try_from(payload.len()).unwrap().to_le_bytes());
        bytes.extend(payload);
        bytes.resize(8 + payload.len().next_multiple_of(4), 0);
        bytes
    }

    fn nest(tag: &str, children: &[Vec<u8>]) -> Vec<u8> {
        block(tag, &children.concat())
    }

    fn integer(value: i32) -> Vec<u8> {
        value.to_le_bytes().into()
    }

    /// One curve, as the file writes it: the behaviours either side of its keys, then the keys.
    fn curve(tag: &str, pre: i32, post: i32, keys: &[(i16, i16, [f32; 3])]) -> Vec<u8> {
        let mut written = Vec::new();
        for &(time, kind, data) in keys {
            written.extend(time.to_le_bytes());
            written.extend(kind.to_le_bytes());
            written.extend(data.iter().flat_map(|value| value.to_le_bytes()));
        }
        nest(
            tag,
            &[
                block("BvPr", &integer(pre)),
                block("BvPo", &integer(post)),
                block("Keys", &written),
            ],
        )
    }

    fn read(children: &[Vec<u8>]) -> Rendered {
        let mut blocks = vec![block("Ver", &integer(0x2011_0913))];
        blocks.extend_from_slice(children);
        match decode("test.avfx", &nest("AVFX", &blocks)).unwrap() {
            Preview::Avfx(effect) => *effect,
            _ => panic!("read as something other than an effect"),
        }
    }

    /// The one curve a file holds, which every curve test builds under a particle.
    fn only(keys: &[(i16, i16, [f32; 3])], tag: &str, pre: i32, post: i32) -> Rendered {
        read(&[nest("Ptcl", &[curve(tag, pre, post, keys)])])
    }

    fn scalars(values: &[(i16, i16, f32)]) -> Vec<(i16, i16, [f32; 3])> {
        values
            .iter()
            .map(|&(time, kind, value)| (time, kind, [1.0, 1.0, value]))
            .collect()
    }

    #[test]
    fn a_payload_reads_as_whichever_of_the_two_its_bits_can_be() {
        assert_eq!(scalar(0i32.to_le_bytes()), "0");
        assert_eq!(scalar(30i32.to_le_bytes()), "30");
        assert_eq!(scalar((-1i32).to_le_bytes()), "-1");
        assert_eq!(scalar(1.0f32.to_le_bytes()), "1");
        assert_eq!(scalar(0.5f32.to_le_bytes()), "0.5");
        assert_eq!(scalar((-2.25f32).to_le_bytes()), "-2.25");
    }

    #[test]
    fn a_curve_collapses_into_the_row_it_sits_on() {
        let effect = only(&scalars(&[(0, 1, 0.0), (10, 1, 1.0)]), "X", 0, 0);
        let labels = effect
            .rows
            .iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(labels, ["Particles", "Particle 0", "X"]);
        assert_eq!(effect.curves.len(), 1);
        assert_eq!(effect.rows[2].curve, Some(0));
        assert_eq!(effect.rows[2].detail, "2 keys  0..10");
    }

    #[test]
    fn linear_runs_between_its_keys() {
        let effect = only(&scalars(&[(0, 1, 0.0), (10, 1, 1.0)]), "X", 0, 0);
        let curve = &effect.curves[0];
        assert_eq!(curve.sample(0.0)[2], 0.0);
        assert_eq!(curve.sample(5.0)[2], 0.5);
        assert_eq!(curve.sample(10.0)[2], 1.0);
    }

    #[test]
    fn a_step_holds_until_the_next_key() {
        let effect = only(&scalars(&[(0, 2, 3.0), (10, 1, 7.0)]), "X", 0, 0);
        let curve = &effect.curves[0];
        assert_eq!(curve.sample(0.0)[2], 3.0);
        assert_eq!(curve.sample(9.9)[2], 3.0);
        assert_eq!(curve.sample(10.0)[2], 7.0);
    }

    /// The tangent scales beside a spline key go unread, so all that is pinned down is that the
    /// curve meets its keys and bulges between them.
    #[test]
    fn a_spline_meets_its_keys() {
        let effect = only(
            &scalars(&[(0, 0, 0.0), (10, 0, 1.0), (20, 0, 0.0)]),
            "X",
            0,
            0,
        );
        let curve = &effect.curves[0];
        assert_eq!(curve.sample(0.0)[2], 0.0);
        assert_eq!(curve.sample(10.0)[2], 1.0);
        assert_eq!(curve.sample(20.0)[2], 0.0);
        assert!(curve.sample(5.0)[2] > 0.5);
    }

    #[test]
    fn behaviours_carry_a_curve_outside_its_keys() {
        let keys = scalars(&[(0, 1, 1.0), (10, 1, 3.0)]);

        let hold = only(&keys, "X", 0, 0);
        assert_eq!(hold.curves[0].sample(-10.0)[2], 1.0);
        assert_eq!(hold.curves[0].sample(20.0)[2], 3.0);

        let repeat = only(&keys, "X", 1, 1);
        assert_eq!(repeat.curves[0].sample(15.0)[2], 2.0);
        assert_eq!(repeat.curves[0].sample(-5.0)[2], 2.0);

        let add = only(&keys, "X", 2, 2);
        assert_eq!(add.curves[0].sample(15.0)[2], 4.0);
        assert_eq!(add.curves[0].sample(25.0)[2], 6.0);
    }

    #[test]
    fn a_single_key_holds_everywhere() {
        let effect = only(&scalars(&[(30, 1, 2.5)]), "X", 1, 1);
        assert_eq!(effect.curves[0].sample(-100.0)[2], 2.5);
        assert_eq!(effect.curves[0].sample(100.0)[2], 2.5);
    }

    #[test]
    fn keys_out_of_order_are_read_in_time_order() {
        let effect = only(&scalars(&[(10, 1, 2.0), (0, 1, 1.0)]), "X", 0, 0);
        assert_eq!(effect.curves[0].sample(5.0)[2], 1.5);
    }

    /// An `RGB` curve writes a channel in each of its three floats rather than a value and two
    /// tangent scales, so all three interpolate.
    #[test]
    fn a_color_curve_interpolates_every_channel() {
        let effect = only(
            &[(0, 1, [1.0, 0.0, 0.0]), (10, 1, [0.0, 1.0, 0.0])],
            "RGB",
            0,
            0,
        );
        let curve = &effect.curves[0];
        assert!(curve.color);
        assert_eq!(curve.sample(5.0), [0.5, 0.5, 0.0]);
        assert_eq!(curve.swatch(0.0), egui::Color32::from_rgb(255, 0, 0));
    }

    #[test]
    fn an_absent_reference_reads_as_none() {
        let effect = read(&[nest(
            "TmLn",
            &[
                block("LpSt", &integer(0)),
                block("LpEd", &integer(-1)),
                block("TICn", &integer(1)),
                block(
                    "Item",
                    &[
                        block("bEna", &integer(1)),
                        block("StTm", &integer(0)),
                        block("EdTm", &integer(60)),
                        block("BdNo", &integer(-1)),
                        block("EfNo", &integer(2)),
                        block("EmNo", &integer(-1)),
                    ]
                    .concat(),
                ),
            ],
        )]);
        let item = effect
            .rows
            .iter()
            .find(|row| row.label == "Item 0")
            .expect("the timeline's item");
        assert_eq!(item.detail, "0..60  emitter none  binder none  effector 2");
        let timeline = &effect.rows[1];
        assert_eq!(timeline.detail, "loop 0..none  1 items");
    }

    #[test]
    fn a_sound_is_a_link_and_a_texture_is_its_own_row() {
        let effect = read(&[
            nest("Emit", &[block("SdNm", b"sound/vfx/se_vfx_test.scd\0")]),
            block("Tex", b"vfx/common/texture/uv_r.atex\0"),
        ]);
        let sound = effect
            .rows
            .iter()
            .find(|row| row.label == "SdNm")
            .expect("the emitter's sound");
        assert_eq!(sound.asset.as_deref(), Some("sound/vfx/se_vfx_test.scd"));
        let texture = effect
            .rows
            .iter()
            .find(|row| row.label == "Tex 0")
            .expect("the effect's texture");
        assert_eq!(
            texture.asset.as_deref(),
            Some("vfx/common/texture/uv_r.atex")
        );
    }

    #[test]
    fn a_container_draws_the_curves_written_under_it() {
        let effect = read(&[nest(
            "Ptcl",
            &[nest(
                "Col",
                &[
                    curve("RGB", 0, 0, &[(0, 1, [1.0, 1.0, 1.0])]),
                    curve("A", 0, 0, &scalars(&[(0, 1, 1.0)])),
                ],
            )],
        )]);
        let column = effect
            .rows
            .iter()
            .position(|row| row.label == "Col")
            .expect("the colour container");
        assert_eq!(effect.drawn(column), vec![0, 1]);
        assert_eq!(effect.drawn(column + 1), vec![0]);
    }
}
