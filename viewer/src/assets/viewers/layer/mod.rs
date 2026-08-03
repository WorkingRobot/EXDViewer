//! The tree `.lgb` and `.sgb` hold, and the scene `.sgb` and `.lvb` wrap it in: a group of layers,
//! each holding placed instances, beside the files the zone is drawn from.
//!
//! Almost every instance names the file it draws itself from, so the tree is mostly a way into the
//! models, shared groups and effects a zone is built out of.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use egui::{RichText, ScrollArea, Sense, Vec2, collapsing_header::paint_default_icon, vec2};
use ironworks::file::layer::{Colour, Instance, InstanceData, LayerGroup, Scene, TriggerBox};
use ironworks::file::{lgb::LayerGroupFile, lvb::LevelFile, sgb::SharedGroupFile};

use super::{facts, link, section};
use crate::assets::deps::Deps;
use crate::backend::Backend;

pub mod lgb;
pub mod lvb;
pub mod scene;
pub mod sgb;

/// Space each level of the tree is set in by.
/// The sheet a scene filter names a territory of.
const TERRITORY: &str = "TerritoryType";

/// The sheet it names a duty of, where the territory is entered through one.
const DUTY: &str = "ContentFinderCondition";

const INDENT: f32 = 12.0;

/// Room the expander takes, kept on rows without one so their labels still line up.
const TRIANGLE: f32 = 12.0;

/// Points of a path or a range listed before the rest are left to the count.
const LISTED: usize = 8;

/// The file the tree was read from, which it keeps rather than copying every instance out: all a
/// row needs is where to find its own.
enum Source {
    Group(LayerGroupFile),
    Shared(SharedGroupFile),
    Level(LevelFile),
}

impl Source {
    fn groups(&self) -> &[LayerGroup] {
        match self {
            Self::Group(file) => std::slice::from_ref(file.group()),
            Self::Shared(file) => file.scene().layer_groups(),
            Self::Level(file) => file.scene().layer_groups(),
        }
    }

    fn scene(&self) -> Option<&Scene> {
        match self {
            Self::Group(_) => None,
            Self::Shared(file) => Some(file.scene()),
            Self::Level(file) => Some(file.scene()),
        }
    }
}

/// The files a scene names. A field the scene left empty is dropped rather than listed blank.
fn files(scene: &Scene) -> Vec<(&'static str, String)> {
    let mut files: Vec<(&'static str, String)> = scene
        .layer_group_paths()
        .iter()
        .map(|path| ("Layer group", path.clone()))
        .collect();
    files.push(("Sky visibility", scene.sky_visibility_path().clone()));
    files.push(("Light culling", scene.light_culling_path().clone()));
    for environment in scene.environments() {
        files.push(("Environment", environment.asset_path().clone()));
        files.push(("Environment sound", environment.sound_asset_path().clone()));
    }
    files.retain(|(_, path)| !path.is_empty());
    files
}

/// Where a row sits in the tree.
#[derive(Clone, Copy, PartialEq, Eq)]
enum At {
    Group(usize),
    Layer(usize, usize),
    Instance(usize, usize, usize),
}

impl At {
    fn depth(self) -> usize {
        match self {
            Self::Group(..) => 0,
            Self::Layer(..) => 1,
            Self::Instance(..) => 2,
        }
    }
}

/// One row as it is drawn.
struct Line<'a> {
    label: String,
    /// Where the row sits and what its payload says, drawn weakly after the label.
    detail: String,
    /// The file the row names.
    asset: Option<&'a str>,
}

/// One field of the selected row. A path is drawn as a link, anything else as text.
enum Fact {
    Text(String),
    Path(String),
}

#[derive(Default)]
struct Rows(Vec<(&'static str, Fact)>);

impl Rows {
    fn text(&mut self, label: &'static str, value: impl Into<String>) {
        self.0.push((label, Fact::Text(value.into())));
    }

    /// Dropped where the field is blank, which is how an unset path is written.
    fn path(&mut self, label: &'static str, path: &str) {
        if !path.is_empty() {
            self.0.push((label, Fact::Path(path.to_owned())));
        }
    }
}

pub struct Rendered {
    path: String,
    identity: Vec<(&'static str, String)>,
    /// The files the scene names, each of which the browser can open.
    files: Vec<(&'static str, String)>,
    /// The territories the scene is used from, and the duty each is entered through.
    filters: Vec<(u16, u16)>,
    source: Source,
    rows: Vec<At>,
    /// Instance kinds and how many of each.
    kinds: Vec<(String, usize)>,
    /// Where the open rows and the selected one are kept, since drawing takes the file by
    /// reference.
    state: egui::Id,
    /// Whether the tree or the scene is showing, and the scene itself once it has been asked for.
    /// The scene owns GL objects and fetches of its own, so it is built on the first switch rather
    /// than with the file.
    placed: Cell<bool>,
    scene: RefCell<Option<scene::Scene>>,
}

fn axes(values: [f32; 3]) -> String {
    format!("{:.3}, {:.3}, {:.3}", values[0], values[1], values[2])
}

fn span(values: [f32; 2]) -> String {
    format!("{:.2} to {:.2}", values[0], values[1])
}

fn on(value: bool) -> &'static str {
    match value {
        true => "yes",
        false => "no",
    }
}

fn color(colour: Colour) -> String {
    format!(
        "{}, {}, {}, {} x {:.2}",
        colour.red(),
        colour.green(),
        colour.blue(),
        colour.alpha(),
        colour.intensity()
    )
}

/// How much room a volume covers. Nothing in the payload of a range states its size; the instance
/// is a unit shape and its scale is what gives it one.
fn extent(scale: [f32; 3]) -> String {
    format!(
        "{:.1} x {:.1} x {:.1}",
        scale[0].abs(),
        scale[1].abs(),
        scale[2].abs()
    )
}

fn trigger(rows: &mut Rows, trigger: TriggerBox, scale: [f32; 3]) {
    rows.text("Shape", format!("{:?}  {}", trigger.shape(), extent(scale)));
    rows.text("Priority", trigger.priority().to_string());
    rows.text("Enabled", on(trigger.enabled()));
}

fn listed(points: impl ExactSizeIterator<Item = String>) -> String {
    let count = points.len();
    let mut listed = points.take(LISTED).collect::<Vec<_>>().join("\n");
    if count > LISTED {
        listed.push_str(&format!("\n… {} more", count - LISTED));
    }
    listed
}

/// The file an instance draws itself from, where its payload names one.
fn asset(data: &InstanceData) -> Option<&str> {
    let path: &str = match data {
        InstanceData::BgPart(part) => part.asset_path(),
        InstanceData::SharedGroup(group) => group.asset_path(),
        InstanceData::Vfx(vfx) => vfx.asset_path(),
        InstanceData::Sound(sound) => sound.asset_path(),
        InstanceData::EnvSpace(space) => space.asset_path(),
        InstanceData::Light(light) => light.texture_path(),
        InstanceData::Decal(decal) => decal.diffuse_path(),
        InstanceData::EnvLocation(location) => location.ambient_light_asset_path(),
        InstanceData::CollisionBox(collision) => collision.collision_asset_path(),
        _ => "",
    };
    (!path.is_empty()).then_some(path)
}

/// The one line of a payload worth reading beside the file it names.
fn summary(instance: &Instance) -> String {
    let scale = instance.transform().scale();
    match instance.data() {
        InstanceData::None => String::new(),
        InstanceData::BgPart(part) => match part.visible() {
            true => format!("{:?}", part.collision()),
            false => format!("{:?}, hidden", part.collision()),
        },
        InstanceData::Light(light) => format!("{:?}, range {:.1}", light.kind(), light.range()),
        InstanceData::Vfx(vfx) => match vfx.auto_play() {
            true => "auto play".to_owned(),
            false => String::new(),
        },
        InstanceData::PositionMarker(marker) => format!("{:?}", marker.kind()),
        InstanceData::SharedGroup(group) => format!("{:?}", group.initial_door_state()),
        InstanceData::Sound(sound) => format!("{:?}", sound.kind()),
        InstanceData::EventNpc(npc) => format!("base {}", npc.character().object().base_id()),
        InstanceData::Character(character) => format!("base {}", character.object().base_id()),
        InstanceData::Aetheryte(aetheryte) => format!("base {}", aetheryte.object().base_id()),
        InstanceData::EnvSpace(space) => format!("{:?}", space.shape()),
        InstanceData::Treasure(treasure) => format!("base {}", treasure.object().base_id()),
        InstanceData::Weapon(weapon) => format!("model {}", weapon.model().pattern_id()),
        InstanceData::PopRange(pop) => {
            format!("{:?}, {} positions", pop.kind(), pop.positions().len())
        }
        InstanceData::ExitRange(exit) => format!("{:?}, zone {}", exit.kind(), exit.zone_id()),
        InstanceData::MapRange(range) => format!("map {}", range.map()),
        InstanceData::EventObject(object) => format!("base {}", object.object().base_id()),
        InstanceData::EnvLocation(_) => String::new(),
        InstanceData::EventRange(box_)
        | InstanceData::DoorRange(box_)
        | InstanceData::ClickableRange(box_) => format!("{:?}  {}", box_.shape(), extent(scale)),
        InstanceData::QuestMarker(marker) => format!("{:?}", marker.unknown()),
        InstanceData::CollisionBox(collision) => {
            format!("{:?}  {}", collision.trigger().shape(), extent(scale))
        }
        InstanceData::LineVfx(line) => format!("{:?}", line.style()),
        InstanceData::ClientPath(path) => format!("{} points", path.control_points().len()),
        InstanceData::TargetMarker(marker) => format!("{:?}", marker.kind()),
        InstanceData::ChairMarker(chair) => format!("{:?}", chair.kind()),
        InstanceData::PrefetchRange(range) => {
            format!("{:?}  {}", range.trigger().shape(), extent(scale))
        }
        InstanceData::FateRange(range) => {
            format!("{:?}  {}", range.trigger().shape(), extent(scale))
        }
        InstanceData::Decal(_) => String::new(),
        InstanceData::CullingBox(_) => extent(scale),
        InstanceData::Unknown(bytes) => format!("{} bytes unread", bytes.len()),
    }
}

/// Everything a payload holds, for the panel that inspects one instance.
fn payload(instance: &Instance) -> Rows {
    let scale = instance.transform().scale();
    let mut rows = Rows::default();
    match instance.data() {
        InstanceData::None => {}
        InstanceData::BgPart(part) => {
            rows.path("Model", part.asset_path());
            rows.path("Collision", part.collision_asset_path());
            rows.text("Collision mode", format!("{:?}", part.collision()));
            if part.collision_material_mask() != 0 {
                rows.text(
                    "Collision mask",
                    format!("{:#018x}", part.collision_material_mask()),
                );
            }
            if part.collision_material_id() != 0 {
                rows.text(
                    "Collision material",
                    format!("{:#018x}", part.collision_material_id()),
                );
            }
            rows.text("Visible", on(part.visible()));
            rows.text(
                "World shadow",
                format!("{:?}", part.world_light_shadow_mode()),
            );
            rows.text(
                "Object shadow",
                format!("{:?}", part.object_light_shadow_mode()),
            );
            rows.text("Fade out", format!("{:.1}", part.fade_out_distance()));
            rows.text(
                "Bounding sphere",
                format!("{:.1}", part.bounding_sphere_size()),
            );
        }
        InstanceData::Light(light) => {
            rows.text("Light", format!("{:?}", light.kind()));
            rows.text("Point light", format!("{:?}", light.point_light_kind()));
            rows.text("Range", format!("{:.2}", light.range()));
            rows.text("Attenuation", format!("{:.3}", light.attenuation()));
            rows.text(
                "Cone coefficient",
                format!("{:.3}", light.attenuation_cone_coefficient()),
            );
            rows.text("Spot angle", format!("{:.3}", light.spot_angle()));
            rows.text("Color", color(light.colour()));
            rows.path("Texture", light.texture_path());
            rows.text("Specular", on(light.specular_highlights()));
            rows.text("Scenery shadows", on(light.bg_part_shadows()));
            rows.text("Character shadows", on(light.character_shadows()));
        }
        InstanceData::Vfx(vfx) => {
            rows.path("Effect", vfx.asset_path());
            rows.text("Color", color(vfx.colour()));
            rows.text(
                "Soft particle fade",
                format!("{:.2}", vfx.soft_particle_fade_range()),
            );
            rows.text("Auto play", on(vfx.auto_play()));
            rows.text("No far clip", on(vfx.no_far_clip()));
            rows.text("Fade near", span(vfx.fade_near()));
            rows.text("Fade far", span(vfx.fade_far()));
        }
        InstanceData::PositionMarker(marker) => {
            rows.text("Marker", format!("{:?}", marker.kind()));
            rows.text("Comment", format!("{:#x}", marker.comment_en_offset()));
            rows.text("Comment (JP)", format!("{:#x}", marker.comment_jp_offset()));
        }
        InstanceData::SharedGroup(group) => {
            rows.path("Group", group.asset_path());
            rows.text("Door", format!("{:?}", group.initial_door_state()));
            rows.text("Rotation", format!("{:?}", group.initial_rotation_state()));
            rows.text(
                "Transform",
                format!("{:?}", group.initial_transform_state()),
            );
            rows.text("Color", format!("{:?}", group.initial_colour_state()));
            rows.text(
                "Random timeline",
                format!(
                    "auto play {}, loop {}",
                    on(group.random_timeline_auto_play()),
                    on(group.random_timeline_loop_playback())
                ),
            );
            rows.text(
                "Collision without event object",
                on(group.collision_controllable_without_event_object()),
            );
            if group.bound_client_path_instance_id() != 0 {
                rows.text(
                    "Bound path",
                    group.bound_client_path_instance_id().to_string(),
                );
            }
            let path = group.move_path();
            rows.text("Move path", format!("{:?}", path.mode()));
            rows.text(
                "Move timing",
                format!(
                    "{} over {}, accelerate {}, decelerate {}",
                    on(path.auto_play()),
                    path.time(),
                    path.accelerate_time(),
                    path.decelerate_time()
                ),
            );
            rows.text(
                "Move rotation",
                format!(
                    "{:?}, loop {}, reverse {}",
                    path.rotation(),
                    on(path.loop_playback()),
                    on(path.reverse())
                ),
            );
            rows.text("Swing vertical", span(path.vertical_swing_range()));
            rows.text("Swing horizontal", span(path.horizontal_swing_range()));
            rows.text("Swing speed", span(path.swing_move_speed_range()));
            rows.text("Swing rotation", span(path.swing_rotation()));
            rows.text(
                "Swing rotation speed",
                span(path.swing_rotation_speed_range()),
            );
            if !group.overrides().is_empty() {
                rows.text(
                    "Overrides",
                    format!("{} bytes unread", group.overrides().len()),
                );
            }
        }
        InstanceData::Sound(sound) => {
            rows.path("Sound", sound.asset_path());
            rows.text("Emitter", format!("{:?}", sound.kind()));
            rows.text("Auto play", on(sound.auto_play()));
            rows.text("No far clip", on(sound.no_far_clip()));
            rows.text("Point selection", sound.point_selection().to_string());
            if !sound.binary().is_empty() {
                rows.text("Geometry", format!("{} bytes unread", sound.binary().len()));
            }
        }
        InstanceData::EventNpc(npc) => {
            rows.text("Base", npc.character().object().base_id().to_string());
            rows.text("Character", format!("{:?}", npc.character().unknown()));
            rows.text("Unknown", format!("{:?}", npc.unknown()));
        }
        InstanceData::Character(character) => {
            rows.text("Base", character.object().base_id().to_string());
            rows.text("Unknown", format!("{:?}", character.unknown()));
        }
        InstanceData::Aetheryte(aetheryte) => {
            rows.text("Base", aetheryte.object().base_id().to_string());
            rows.text("Bound instance", aetheryte.bound_instance_id().to_string());
            rows.text("Unknown", aetheryte.unknown().to_string());
        }
        InstanceData::EnvSpace(space) => {
            rows.path("Environment", space.asset_path());
            rows.path("Sound", space.sound_asset_path());
            rows.text("Shape", format!("{:?}  {}", space.shape(), extent(scale)));
            rows.text("Bound instance", space.bound_instance_id().to_string());
            rows.text("Env map shooting point", on(space.env_map_shooting_point()));
            rows.text("Priority", space.priority().to_string());
            rows.text("Effective range", format!("{:.2}", space.effective_range()));
            rows.text("Interpolation", space.interpolation_time().to_string());
            rows.text("Reverb", format!("{:.2}", space.reverb()));
            rows.text("Filter", format!("{:.2}", space.filter()));
        }
        InstanceData::Treasure(treasure) => {
            rows.text("Base", treasure.object().base_id().to_string());
        }
        InstanceData::Weapon(weapon) => {
            let model = weapon.model();
            rows.text("Skeleton", model.skeleton_id().to_string());
            rows.text("Pattern", model.pattern_id().to_string());
            rows.text("Image change", model.image_change_id().to_string());
            rows.text("Staining", model.staining_id().to_string());
            rows.text("Visible", on(weapon.visible()));
        }
        InstanceData::PopRange(pop) => {
            rows.text("Pop", format!("{:?}", pop.kind()));
            rows.text(
                "Inner radius ratio",
                format!("{:.3}", pop.inner_radius_ratio()),
            );
            rows.text("Radius", extent(scale));
            if !pop.positions().is_empty() {
                rows.text(
                    "Positions",
                    listed(pop.positions().iter().map(|point| axes(*point))),
                );
            }
        }
        InstanceData::ExitRange(exit) => {
            trigger(&mut rows, exit.trigger(), scale);
            rows.text("Exit", format!("{:?}", exit.kind()));
            rows.text("Zone", exit.zone_id().to_string());
            rows.text("Territory type", exit.territory_type_id().to_string());
            rows.text("Index", exit.index().to_string());
            rows.text(
                "Destination instance",
                exit.destination_instance_id().to_string(),
            );
            rows.text("Return instance", exit.return_instance_id().to_string());
            rows.text(
                "Running direction",
                format!("{:.3}", exit.player_running_direction()),
            );
        }
        InstanceData::MapRange(range) => {
            trigger(&mut rows, range.trigger(), scale);
            rows.text("Map", range.map().to_string());
            rows.text("Place name", range.place_name_block().to_string());
            rows.text("Place name spot", range.place_name_spot().to_string());
            rows.text("Weather", range.weather().to_string());
            rows.text("Music", range.bgm().to_string());
            rows.text("Housing block", range.housing_block_id().to_string());
            rows.text("Discovery", range.discovery_id().to_string());
            let switches = [
                ("map", range.map_enabled()),
                ("place name", range.place_name_enabled()),
                ("discovery", range.discovery_enabled()),
                ("music", range.bgm_enabled()),
                ("music on entry only", range.bgm_play_zone_in_only()),
                ("weather", range.weather_enabled()),
                ("rest bonus", range.rest_bonus_enabled()),
                ("rest bonus effective", range.rest_bonus_effective()),
                ("lift", range.lift_enabled()),
                ("housing", range.housing_enabled()),
                ("log flying height error", range.log_flying_height_max_err()),
                ("mounts off", range.mounts_and_ornaments_disabled()),
                ("lalafell only", range.lalafell_only()),
            ];
            let on = switches
                .iter()
                .filter(|(_, set)| *set)
                .map(|(name, _)| *name)
                .collect::<Vec<_>>();
            rows.text(
                "Applies",
                match on.is_empty() {
                    true => "nothing".to_owned(),
                    false => on.join(", "),
                },
            );
        }
        InstanceData::EventObject(object) => {
            rows.text("Base", object.object().base_id().to_string());
            rows.text("Bound instance", object.bound_instance_id().to_string());
            rows.text("Unknown", object.unknown().to_string());
        }
        InstanceData::EnvLocation(location) => {
            rows.path("Ambient light", location.ambient_light_asset_path());
            rows.path("Env map", location.env_map_asset_path());
        }
        InstanceData::EventRange(box_)
        | InstanceData::DoorRange(box_)
        | InstanceData::ClickableRange(box_) => trigger(&mut rows, *box_, scale),
        InstanceData::QuestMarker(marker) => {
            rows.text("Unknown", format!("{:?}", marker.unknown()));
        }
        InstanceData::CollisionBox(collision) => {
            trigger(&mut rows, collision.trigger(), scale);
            rows.path("Collision", collision.collision_asset_path());
            rows.text(
                "Collision mask",
                format!("{:#018x}", collision.collision_material_mask()),
            );
            rows.text(
                "Collision material",
                format!("{:#018x}", collision.collision_material_id()),
            );
        }
        InstanceData::LineVfx(line) => rows.text("Style", format!("{:?}", line.style())),
        InstanceData::ClientPath(path) => {
            rows.text("Points", path.control_points().len().to_string());
            rows.text(
                "Control points",
                listed(path.control_points().iter().map(|point| {
                    format!(
                        "{}  id {}{}",
                        axes(point.position()),
                        point.id(),
                        match point.select() {
                            true => ", select",
                            false => "",
                        }
                    )
                })),
            );
        }
        InstanceData::TargetMarker(marker) => {
            rows.text("Anchor", format!("{:?}", marker.kind()));
            rows.text(
                "Nameplate offset",
                format!("{:.3}", marker.nameplate_offset_y()),
            );
        }
        InstanceData::ChairMarker(chair) => {
            rows.text("Seat", format!("{:?}", chair.kind()));
            let sides = [
                ("left", chair.left()),
                ("right", chair.right()),
                ("back", chair.back()),
            ];
            let taken = sides
                .iter()
                .filter(|(_, set)| *set)
                .map(|(name, _)| *name)
                .collect::<Vec<_>>();
            rows.text(
                "Sides",
                match taken.is_empty() {
                    true => "none".to_owned(),
                    false => taken.join(", "),
                },
            );
        }
        InstanceData::PrefetchRange(range) => {
            trigger(&mut rows, range.trigger(), scale);
            rows.text("Bound instance", range.bound_instance_id().to_string());
        }
        InstanceData::FateRange(range) => {
            trigger(&mut rows, range.trigger(), scale);
            rows.text(
                "Fate layout label",
                range.fate_layout_label_id().to_string(),
            );
        }
        InstanceData::Decal(decal) => {
            rows.path("Diffuse", decal.diffuse_path());
            rows.path("Normal", decal.normal_path());
            rows.path("Specular", decal.specular_path());
        }
        InstanceData::CullingBox(box_) => {
            rows.text("Volume", extent(scale));
            rows.text("Unknown", box_.unknown().to_string());
        }
        InstanceData::Unknown(bytes) => rows.text(
            "Payload",
            format!("{:?} unread, {} bytes", instance.kind(), bytes.len()),
        ),
    }
    rows
}

fn rows(groups: &[LayerGroup]) -> Vec<At> {
    let mut rows = Vec::new();
    for (group_index, group) in groups.iter().enumerate() {
        rows.push(At::Group(group_index));
        for (layer_index, layer) in group.layers().iter().enumerate() {
            rows.push(At::Layer(group_index, layer_index));
            rows.extend(
                (0..layer.instances().len())
                    .map(|instance| At::Instance(group_index, layer_index, instance)),
            );
        }
    }
    rows
}

fn tally(groups: &[LayerGroup]) -> Vec<(String, usize)> {
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for instance in groups
        .iter()
        .flat_map(LayerGroup::layers)
        .flat_map(|layer| layer.instances())
    {
        let name = format!("{:?}", instance.kind());
        match kinds.iter_mut().find(|(kind, _)| *kind == name) {
            Some((_, count)) => *count += 1,
            None => kinds.push((name, 1)),
        }
    }
    kinds.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    kinds
}

fn rendered(path: &str, mut identity: Vec<(&'static str, String)>, source: Source) -> Rendered {
    let rows = rows(source.groups());
    let kinds = tally(source.groups());
    let instances = kinds.iter().map(|(_, count)| count).sum::<usize>();
    identity.push((
        "Layers",
        source
            .groups()
            .iter()
            .map(|group| group.layers().len())
            .sum::<usize>()
            .to_string(),
    ));
    identity.push(("Instances", instances.to_string()));

    log::info!("assets/layer: {path} {instances} instances");

    Rendered {
        path: path.to_owned(),
        identity,
        files: source.scene().map(files).unwrap_or_default(),
        filters: source
            .scene()
            .map(|scene| {
                scene
                    .filters()
                    .iter()
                    .map(|filter| (filter.territory_type(), filter.content_finder_condition()))
                    .collect()
            })
            .unwrap_or_default(),
        source,
        rows,
        kinds,
        state: egui::Id::new(path).with("layer_tree"),
        placed: Cell::new(false),
        scene: RefCell::new(None),
    }
}

pub fn ui(
    ui: &mut egui::Ui,
    file: &Rendered,
    deps: &mut Deps,
    backend: &Backend,
) -> Option<String> {
    ui.horizontal(|ui| {
        for (placed, label) in [(false, "Tree"), (true, "Scene")] {
            if ui
                .selectable_label(file.placed.get() == placed, label)
                .clicked()
            {
                file.placed.set(placed);
            }
        }
    });
    ui.add_space(4.0);
    if file.placed.get() {
        let mut held = file.scene.borrow_mut();
        scene::ui(
            ui,
            held.get_or_insert_with(|| scene::Scene::new(&file.path, &file.source)),
            backend,
        );
        return None;
    }

    let mut follow = None;
    if !file.files.is_empty() {
        section(ui, "Files");
        egui::Grid::new("layer_files")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for (label, path) in &file.files {
                    ui.label(RichText::new(*label).weak());
                    if link(ui, crate::utils::file_name(path), path) {
                        follow = Some(path.clone());
                    }
                    ui.allocate_space(vec2(ui.available_width(), 0.0));
                    ui.end_row();
                }
            });
        ui.add_space(8.0);
        ui.separator();
    }

    if !file.filters.is_empty() {
        section(ui, "Used by");
        egui::Grid::new("layer_filters")
            .num_columns(3)
            .striped(true)
            .show(ui, |ui| {
                for &(territory, duty) in &file.filters {
                    ui.label(RichText::new(format!("Territory {territory}")).weak());
                    let named = deps.text(ui.ctx(), backend, TERRITORY, u32::from(territory));
                    ui.label(RichText::new(named.unwrap_or_default()).monospace());
                    if duty > 0 {
                        let named = deps.text(ui.ctx(), backend, DUTY, u32::from(duty));
                        ui.label(
                            RichText::new(named.map_or_else(
                                || format!("duty {duty}"),
                                |name| format!("{name} ({duty})"),
                            ))
                            .monospace(),
                        );
                    }
                    ui.allocate_space(vec2(ui.available_width(), 0.0));
                    ui.end_row();
                }
            });
        ui.add_space(8.0);
        ui.separator();
    }
    // A level names its layer groups rather than holding any, so there is no tree to draw for one.
    if file.rows.is_empty() {
        return follow;
    }

    let mut open = file.open(ui);
    let mut shown = Vec::new();
    let mut collapsed_at = None;
    for (index, at) in file.rows.iter().enumerate() {
        match collapsed_at {
            Some(depth) if at.depth() > depth => continue,
            _ => collapsed_at = None,
        }
        let parent = file.parent(*at);
        // Only a group is open on arrival, since which layer a thing sits on is most of what the
        // file says.
        let expanded = parent && ((at.depth() == 0) != open.contains(&index));
        if parent && !expanded {
            collapsed_at = Some(at.depth());
        }
        shown.push((index, expanded));
    }

    section(ui, "Layers");
    let picked = file.selected(ui);
    let mut selected = picked;
    let mut toggled = None;
    // Every row is one line, but a selectable label pads itself, so the height the scroll area is
    // told has to leave room for that or the rows it places drift out of their own space.
    let height = ui
        .text_style_height(&egui::TextStyle::Monospace)
        .max(TRIANGLE)
        + 2.0 * ui.spacing().button_padding.y
        + ui.spacing().item_spacing.y;
    ScrollArea::vertical()
        .auto_shrink(false)
        .show_rows(ui, height, shown.len(), |ui, range| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for &(index, expanded) in &shown[range] {
                let at = file.rows[index];
                let line = file.line(at);
                ui.horizontal(|ui| {
                    ui.add_space(at.depth() as f32 * INDENT);
                    match file.parent(at) {
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
                            RichText::new(&line.label).monospace(),
                        )
                        .clicked()
                    {
                        selected = Some(index);
                    }
                    if let Some(asset) = line.asset
                        && link(ui, crate::utils::file_name(asset), asset)
                    {
                        follow = Some(asset.to_owned());
                    }
                    if !line.detail.is_empty() {
                        ui.label(RichText::new(&line.detail).monospace().weak());
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

    /// Whether anything sits under a row, which the walk over the whole tree asks of every one of
    /// them and so builds nothing.
    fn parent(&self, at: At) -> bool {
        let groups = self.source.groups();
        match at {
            At::Group(group) => !groups[group].layers().is_empty(),
            At::Layer(group, layer) => !groups[group].layers()[layer].instances().is_empty(),
            At::Instance(..) => false,
        }
    }

    fn instance(&self, at: At) -> Option<&Instance> {
        match at {
            At::Instance(group, layer, instance) => {
                Some(&self.source.groups()[group].layers()[layer].instances()[instance])
            }
            _ => None,
        }
    }

    fn line(&self, at: At) -> Line<'_> {
        let groups = self.source.groups();
        match at {
            At::Group(group) => {
                let group = &groups[group];
                Line {
                    label: match group.name().is_empty() {
                        true => group.id().to_string(),
                        false => group.name().clone(),
                    },
                    detail: format!("{} layers", group.layers().len()),
                    asset: None,
                }
            }
            At::Layer(group, layer) => {
                let layer = &groups[group].layers()[layer];
                Line {
                    label: layer.name().clone(),
                    detail: format!("{} instances", layer.instances().len()),
                    asset: None,
                }
            }
            At::Instance(group, layer, instance) => {
                let instance = &groups[group].layers()[layer].instances()[instance];
                let transform = instance.transform();
                let mut detail = format!("at {}", axes(transform.translation()));
                if transform.rotation() != [0.0; 3] {
                    detail.push_str(&format!("  rotation {}", axes(transform.rotation())));
                }
                if transform.scale() != [1.0; 3] {
                    detail.push_str(&format!("  scale {}", axes(transform.scale())));
                }
                let summary = summary(instance);
                if !summary.is_empty() {
                    detail.push_str("  ");
                    detail.push_str(&summary);
                }
                Line {
                    label: match instance.name().is_empty() {
                        true => format!("{:?} {}", instance.kind(), instance.id()),
                        false => {
                            format!(
                                "{:?} {} {}",
                                instance.kind(),
                                instance.id(),
                                instance.name()
                            )
                        }
                    },
                    detail,
                    asset: asset(instance.data()),
                }
            }
        }
    }

    /// Everything the selected row carries.
    fn fields(&self, at: At) -> Vec<(&'static str, Fact)> {
        let groups = self.source.groups();
        let mut rows = Rows::default();
        match at {
            At::Group(group) => {
                let group = &groups[group];
                rows.text("Group", group.id().to_string());
                rows.text("Layers", group.layers().len().to_string());
            }
            At::Layer(group, layer) => {
                let layer = &groups[group].layers()[layer];
                rows.text("Layer", layer.id().to_string());
                rows.text("Instances", layer.instances().len().to_string());
                rows.text("Visible", on(layer.visible()));
                if layer.festival_id() != 0 {
                    rows.text(
                        "Festival",
                        format!(
                            "{} phase {}",
                            layer.festival_id(),
                            layer.festival_phase_id()
                        ),
                    );
                }
            }
            At::Instance(..) => {
                let instance = self.instance(at).expect("an instance row");
                let transform = instance.transform();
                rows.text("Instance", instance.id().to_string());
                rows.text("Position", axes(transform.translation()));
                rows.text("Rotation", axes(transform.rotation()));
                rows.text("Scale", axes(transform.scale()));
                rows.0.extend(payload(instance).0);
            }
        }
        rows.0
    }

    /// The scene fetches from its own side of the viewer, so the backend only reaches here to keep
    /// both halves of the layer viewer on one signature.
    pub fn details_ui(&self, ui: &mut egui::Ui, follow: &mut Option<String>, _backend: &Backend) {
        if self.placed.get()
            && let Some(scene) = self.scene.borrow_mut().as_mut()
        {
            scene.details_ui(ui);
            return;
        }
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if let Some(index) = self.selected(ui)
                && let Some(&at) = self.rows.get(index)
            {
                ui.label(RichText::new(self.line(at).label).strong());
                ui.add_space(4.0);
                egui::Grid::new("layer_selected")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (label, fact) in self.fields(at) {
                            ui.label(RichText::new(label).weak());
                            match fact {
                                Fact::Text(value) => {
                                    ui.label(RichText::new(value).monospace());
                                }
                                Fact::Path(path) => {
                                    if link(ui, crate::utils::file_name(&path), &path) {
                                        *follow = Some(path);
                                    }
                                }
                            }
                            ui.allocate_space(vec2(ui.available_width(), 0.0));
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
                ui.separator();
            }

            facts(ui, "layer_identity", &self.identity);
            if self.kinds.is_empty() {
                return;
            }
            ui.add_space(8.0);
            ui.separator();
            ui.label(RichText::new("Instance kinds").weak());
            ui.add_space(4.0);
            egui::Grid::new("layer_kinds")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    for (kind, count) in &self.kinds {
                        ui.label(RichText::new(kind).monospace());
                        ui.label(RichText::new(count.to_string()).monospace());
                        ui.allocate_space(vec2(ui.available_width(), 0.0));
                        ui.end_row();
                    }
                });
        });
    }
}
