//! What an effect does, read out of the tags it is written under and stepped a frame at a time.
//!
//! A scheduler starts a timeline, a timeline runs an emitter over a span of frames, and an emitter
//! bursts particles at an interval. A particle carries its own velocity forward and reads its
//! position, rotation, scale and color off curves indexed by how long it has been alive.
//!
//! What the tags mean comes from VFXEditor, which is the only place they are named. Only the ones
//! the corpus actually writes are read here; the rest of a particle's `Data` goes unread, so a kind
//! that draws a ribbon along its own path or warps what is behind it falls back to the sprite the
//! rest use. Nothing random is read: the `R`-suffixed curves beside the ones below go unread, so an
//! effect plays the same way every time and scrubbing back to a frame lands where it did before.

use glam::{Quat, Vec3, Vec4};
use ironworks::file::avfx::{Avfx, Block, Item, Model as Geometry};

use super::curve::{self, Curve};
use super::find;

/// Live particles and running emitters one effect may hold. Both counts come off the file unchecked
/// and this ships to a browser, where an effect asking for millions takes the tab with it.
const PARTICLES: usize = 8192;
const EMITTERS: usize = 512;

/// How deep an emitter may spawn another.
const DEPTH: u8 = 4;

/// Frames a loop runs for where nothing in the file bounds it, and the longest one it may reach.
const LOOP: i32 = 300;
const LONGEST: i32 = 3600;

/// Frames a fit is taken over.
const FITTED: i32 = 300;

/// The Euler order the rest of the browser reads these files under: about X first, then Y, then Z.
fn rotation(angles: Vec3) -> Quat {
    Quat::from_rotation_z(angles.z)
        * Quat::from_rotation_y(angles.y)
        * Quat::from_rotation_x(angles.x)
}

fn integer(blocks: &[Block], name: &str) -> Option<i32> {
    find(blocks, name)?.i32()
}

fn nested<'a>(blocks: &'a [Block], name: &str) -> &'a [Block] {
    find(blocks, name).map_or(&[][..], Block::blocks)
}

/// A tag naming one of the effect's lists. These are written as a list of one-byte indices where
/// the tag allows several, and as a plain integer where it does not.
fn index(blocks: &[Block], name: &str) -> Option<usize> {
    let block = find(blocks, name)?;
    let value = match block.bytes() {
        [only] => i32::from(*only),
        bytes if bytes.len() == 4 => block.i32()?,
        [first, ..] => i32::from(*first),
        [] => return None,
    };
    usize::try_from(value).ok()
}

/// Whether something the file can switch off is switched off.
fn off(blocks: &[Block], name: &str) -> bool {
    find(blocks, name).and_then(Block::bool) == Some(false)
}

/// How long something lives, in frames. A life it never reaches is written as `-1`.
fn life(blocks: &[Block]) -> Option<f32> {
    let value = find(blocks, "Life")?.find("Val")?.f32()?;
    (value >= 0.0).then_some(value)
}

/// One animated value, or the constant the file leaves where it writes no curve.
struct Track {
    curve: Option<Curve>,
    idle: f32,
}

impl Track {
    fn read(blocks: &[Block], name: &str, idle: f32) -> Self {
        Self {
            curve: find(blocks, name).and_then(curve::read),
            idle,
        }
    }

    fn at(&self, frame: f32) -> f32 {
        self.curve
            .as_ref()
            .map_or(self.idle, |curve| curve.sample(frame)[2])
    }
}

fn triple(blocks: &[Block], names: [&str; 3], idle: f32) -> [Track; 3] {
    names.map(|name| Track::read(blocks, name, idle))
}

fn read(tracks: &[Track; 3], frame: f32) -> Vec3 {
    Vec3::from(tracks.each_ref().map(|track| track.at(frame)))
}

/// A value the file writes one curve an axis for, under a container of its own.
struct Axes([Track; 3]);

impl Axes {
    fn read(blocks: &[Block], name: &str, idle: f32) -> Self {
        Self(triple(nested(blocks, name), ["X", "Y", "Z"], idle))
    }

    fn at(&self, frame: f32) -> Vec3 {
        read(&self.0, frame)
    }
}

/// A color: three channels in one curve, with an alpha, a brightness and a per-channel scale
/// written beside them.
struct Tint {
    rgb: Option<Curve>,
    alpha: Track,
    brightness: Track,
    scale: [Track; 4],
}

impl Tint {
    fn read(blocks: &[Block], name: &str) -> Self {
        let inner = nested(blocks, name);
        Self {
            rgb: find(inner, "RGB").and_then(curve::read),
            alpha: Track::read(inner, "A", 1.0),
            brightness: Track::read(inner, "Bri", 1.0),
            scale: ["SclR", "SclG", "SclB", "SclA"].map(|name| Track::read(inner, name, 1.0)),
        }
    }

    fn at(&self, frame: f32) -> Vec4 {
        let rgb = self
            .rgb
            .as_ref()
            .map_or([1.0; 3], |curve| curve.sample(frame));
        let brightness = self.brightness.at(frame);
        Vec4::new(
            rgb[0] * brightness * self.scale[0].at(frame),
            rgb[1] * brightness * self.scale[1].at(frame),
            rgb[2] * brightness * self.scale[2].at(frame),
            self.alpha.at(frame) * self.scale[3].at(frame),
        )
    }
}

/// Where something sits, so a spawned thing can be placed under whatever spawned it.
#[derive(Clone, Copy)]
struct Place {
    origin: Vec3,
    turn: Quat,
    scale: Vec3,
}

impl Place {
    const NONE: Self = Self {
        origin: Vec3::ZERO,
        turn: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    fn under(&self, inner: Place) -> Place {
        Place {
            origin: self.origin + self.turn * (inner.origin * self.scale),
            turn: self.turn * inner.turn,
            scale: self.scale * inner.scale,
        }
    }
}

/// How a particle's color reaches what is already drawn. `RMT`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Blend {
    Opaque,
    Alpha,
    Multiply,
    Screen,
    Subtract,
    Add,
}

impl From<i32> for Blend {
    fn from(value: i32) -> Self {
        match value {
            1 | 9 => Self::Multiply,
            2 | 10 => Self::Add,
            3 | 11 => Self::Subtract,
            4 | 12 => Self::Screen,
            8 => Self::Opaque,
            _ => Self::Alpha,
        }
    }
}

/// What a particle draws as.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Shape {
    /// A quad turned to face the camera.
    Sprite,
    /// One of the effect's own models, indexed into [`Effect::models`].
    Model(usize),
}

struct Particle {
    life: Option<f32>,
    gravity: Track,
    drag: Track,
    position: Axes,
    rotation: Axes,
    scale: Axes,
    spin: [Track; 3],
    color: Tint,
    texture: Option<usize>,
    shape: Shape,
    blend: Blend,
}

impl Particle {
    fn read(block: &Block, models: usize) -> Self {
        let blocks = block.blocks();
        let data = nested(blocks, "Data");
        let model = |name| match index(data, name) {
            Some(model) if model < models => Shape::Model(model),
            _ => Shape::Sprite,
        };
        Self {
            life: life(blocks),
            gravity: Track::read(blocks, "Gra", 0.0),
            drag: Track::read(blocks, "ARs", 0.0),
            position: Axes::read(blocks, "Pos", 0.0),
            rotation: Axes::read(blocks, "Rot", 0.0),
            scale: Axes::read(blocks, "Scl", 1.0),
            spin: triple(blocks, ["VRX", "VRY", "VRZ"], 0.0),
            color: Tint::read(blocks, "Col"),
            texture: index(nested(blocks, "TC1"), "TLst"),
            // The kinds that draw geometry name it under a tag of their own.
            shape: match integer(blocks, "PrVT").unwrap_or_default() {
                5 | 14 => model("MdNo"),
                13 => model("MNO"),
                _ => Shape::Sprite,
            },
            blend: integer(blocks, "RMT").unwrap_or_default().into(),
        }
    }
}

/// One entry of an emitter's particle or emitter list.
struct Spawn {
    target: usize,
    count: i32,
    delay: f32,
}

impl Spawn {
    fn read(item: &Item, of: usize) -> Option<Self> {
        let blocks = item.blocks();
        let target = usize::try_from(integer(blocks, "TgtB")?).ok()?;
        (target < of && !off(blocks, "bEnb")).then(|| Self {
            target,
            count: integer(blocks, "CrCn").unwrap_or(1).clamp(0, 64),
            delay: integer(blocks, "GenD").unwrap_or_default() as f32,
        })
    }
}

struct Emitter {
    life: Option<f32>,
    count: Track,
    interval: Track,
    position: Axes,
    rotation: Axes,
    scale: Axes,
    color: Tint,
    /// `Data/IjS`, how fast a particle leaves, along the direction `Data/AnX`..`AnZ` turns `+Y` to.
    speed: Track,
    heading: [Track; 3],
    particles: Vec<Spawn>,
    emitters: Vec<Spawn>,
}

impl Emitter {
    fn read(emitter: &ironworks::file::avfx::Emitter, particles: usize, emitters: usize) -> Self {
        let blocks = emitter.properties();
        let data = nested(blocks, "Data");
        Self {
            life: life(blocks),
            count: Track::read(blocks, "CrC", 1.0),
            interval: Track::read(blocks, "CrI", 1.0),
            position: Axes::read(blocks, "Pos", 0.0),
            rotation: Axes::read(blocks, "Rot", 0.0),
            scale: Axes::read(blocks, "Scl", 1.0),
            color: Tint::read(blocks, "Col"),
            speed: Track::read(data, "IjS", 0.0),
            heading: triple(data, ["AnX", "AnY", "AnZ"], 0.0),
            particles: emitter
                .particles()
                .iter()
                .filter_map(|item| Spawn::read(item, particles))
                .collect(),
            emitters: emitter
                .emitters()
                .iter()
                .filter_map(|item| Spawn::read(item, emitters))
                .collect(),
        }
    }
}

/// One emitter a timeline runs, and the frames it runs over.
struct Run {
    emitter: usize,
    start: i32,
    until: i32,
}

/// The emitters one timeline runs, added to `runs` at `at`.
fn timeline(file: &Avfx, index: usize, at: i32, runs: &mut Vec<Run>) {
    let Some(timeline) = file.timelines().get(index) else {
        return;
    };
    for item in timeline.items() {
        let blocks = item.blocks();
        if off(blocks, "bEna") {
            continue;
        }
        let Some(emitter) = integer(blocks, "EmNo")
            .and_then(|value| usize::try_from(value).ok())
            .filter(|&emitter| emitter < file.emitters().len())
        else {
            continue;
        };
        let end = integer(blocks, "EdTm").unwrap_or(-1);
        runs.push(Run {
            emitter,
            start: at + integer(blocks, "StTm").unwrap_or_default(),
            until: match end < 0 {
                true => i32::MAX,
                false => at + end,
            },
        });
    }
}

fn runs(file: &Avfx) -> Vec<Run> {
    let mut runs = Vec::new();
    for scheduler in file.schedulers() {
        for item in scheduler.items() {
            let blocks = item.blocks();
            if off(blocks, "bEna") {
                continue;
            }
            let Some(index) = integer(blocks, "TlNo").and_then(|value| usize::try_from(value).ok())
            else {
                continue;
            };
            timeline(
                file,
                index,
                integer(blocks, "StTm").unwrap_or_default(),
                &mut runs,
            );
        }
    }
    // An effect whose schedulers start nothing still holds the timelines and emitters it would
    // have run, and is worth showing rather than leaving blank.
    if runs.is_empty() {
        for index in 0..file.timelines().len() {
            timeline(file, index, 0, &mut runs);
        }
    }
    if runs.is_empty() {
        runs.extend((0..file.emitters().len()).map(|emitter| Run {
            emitter,
            start: 0,
            until: i32::MAX,
        }));
    }
    runs
}

/// A model vertex as the shader reads it.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [u8; 4],
}

pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u16>,
}

fn mesh(model: &Geometry) -> Mesh {
    Mesh {
        vertices: model
            .vertices()
            .iter()
            .map(|vertex| Vertex {
                position: [
                    vertex.position()[0],
                    vertex.position()[1],
                    vertex.position()[2],
                ],
                uv: vertex.uv()[0],
                color: vertex.colour(),
            })
            .collect(),
        indices: model
            .triangles()
            .iter()
            .flat_map(|triangle| triangle.indices())
            .collect(),
    }
}

/// One emitter running: a timeline started it, or a parent emitter did.
struct Running {
    def: usize,
    born: i32,
    until: i32,
    place: Place,
    tint: Vec4,
    /// Frames since the last burst.
    since: f32,
    depth: u8,
}

struct Live {
    def: usize,
    born: i32,
    life: f32,
    /// How far it has carried itself under its own velocity, in the frame it was spawned into.
    at: Vec3,
    velocity: Vec3,
    /// Where the emitter stood when it spawned, which its own curves run under.
    place: Place,
    tint: Vec4,
}

pub struct State {
    pub frame: i32,
    running: Vec<Running>,
    particles: Vec<Live>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            // Nothing has run yet, so the first step lands on frame zero.
            frame: -1,
            running: Vec::new(),
            particles: Vec::new(),
        }
    }
}

/// One thing to draw, in the effect's own space.
pub struct Drawn {
    pub center: [f32; 3],
    pub scale: [f32; 3],
    pub turn: [f32; 4],
    pub color: [f32; 4],
    pub texture: Option<usize>,
    pub shape: Shape,
    pub blend: Blend,
}

pub struct Effect {
    emitters: Vec<Emitter>,
    particles: Vec<Particle>,
    runs: Vec<Run>,
    /// The `.atex` files the particles sample, in the order they index them.
    pub textures: Vec<String>,
    pub models: Vec<Mesh>,
    /// Frames the effect runs for before it starts over.
    pub length: i32,
}

impl Effect {
    pub fn read(file: &Avfx) -> Self {
        let particles: Vec<Particle> = file
            .particles()
            .iter()
            .map(|particle| Particle::read(particle, file.models().len()))
            .collect();
        let emitters = file
            .emitters()
            .iter()
            .map(|emitter| Emitter::read(emitter, particles.len(), file.emitters().len()))
            .collect();
        let runs = runs(file);

        // An effect ends when the last emitter has stopped and the last particle it left has
        // expired. Where either never does, the loop is the viewer's to pick.
        let bounded = runs.iter().all(|run| run.until != i32::MAX)
            && particles.iter().all(|particle| particle.life.is_some());
        let tail = particles
            .iter()
            .filter_map(|particle| particle.life)
            .fold(0.0f32, f32::max) as i32;
        let length = match bounded {
            true => runs.iter().map(|run| run.until).max().unwrap_or_default() + tail,
            false => LOOP,
        }
        .clamp(1, LONGEST);

        Self {
            emitters,
            particles,
            runs,
            textures: file.textures().to_vec(),
            models: file.models().iter().map(mesh).collect(),
            length,
        }
    }

    /// Steps to `frame`, replaying from the start where the state sits past it: a particle's
    /// position is the sum of every step it has taken, so there is no stepping backwards.
    pub fn seek(&self, state: &mut State, frame: i32) {
        if frame < state.frame {
            *state = State::default();
        }
        while state.frame < frame {
            self.step(state);
        }
    }

    fn step(&self, state: &mut State) {
        let frame = state.frame + 1;
        state.frame = frame;

        state.particles.retain_mut(|live| {
            let age = (frame - live.born) as f32;
            if age > live.life {
                return false;
            }
            let def = &self.particles[live.def];
            live.velocity *= (1.0 - def.drag.at(age)).clamp(0.0, 1.0);
            live.velocity.y -= def.gravity.at(age);
            live.at += live.velocity;
            true
        });

        for run in &self.runs {
            if run.start == frame && state.running.len() < EMITTERS {
                state.running.push(Running {
                    def: run.emitter,
                    born: frame,
                    until: run.until,
                    place: Place::NONE,
                    tint: Vec4::ONE,
                    since: f32::INFINITY,
                    depth: 0,
                });
            }
        }
        state.running.retain(|running| frame <= running.until);

        let mut spawned = Vec::new();
        let room = EMITTERS.saturating_sub(state.running.len());
        for running in &mut state.running {
            let def = &self.emitters[running.def];
            let local = (frame - running.born) as f32;
            if def.life.is_some_and(|life| local > life) {
                continue;
            }
            running.since += 1.0;
            if running.since < def.interval.at(local).max(1.0) {
                continue;
            }
            running.since = 0.0;

            let burst = def.count.at(local).round().clamp(0.0, 64.0) as i32;
            if burst == 0 {
                continue;
            }
            let place = running.place.under(Place {
                origin: def.position.at(local),
                turn: rotation(def.rotation.at(local)),
                scale: def.scale.at(local),
            });
            let tint = running.tint * def.color.at(local);
            let velocity = rotation(read(&def.heading, local)) * Vec3::Y * def.speed.at(local);

            for spawn in &def.particles {
                if local < spawn.delay {
                    continue;
                }
                let life = self.particles[spawn.target]
                    .life
                    .unwrap_or(self.length as f32);
                for _ in 0..burst * spawn.count {
                    if state.particles.len() >= PARTICLES {
                        break;
                    }
                    state.particles.push(Live {
                        def: spawn.target,
                        born: frame,
                        life,
                        at: Vec3::ZERO,
                        velocity,
                        place,
                        tint,
                    });
                }
            }

            if running.depth < DEPTH {
                for spawn in &def.emitters {
                    if local < spawn.delay || spawned.len() >= room {
                        break;
                    }
                    spawned.push(Running {
                        def: spawn.target,
                        born: frame,
                        until: self.emitters[spawn.target]
                            .life
                            .map_or(i32::MAX, |life| frame + life as i32),
                        place,
                        tint,
                        since: f32::INFINITY,
                        depth: running.depth + 1,
                    });
                }
            }
        }
        state.running.extend(spawned);
    }

    pub fn drawn(&self, state: &State) -> Vec<Drawn> {
        state
            .particles
            .iter()
            .map(|live| {
                let def = &self.particles[live.def];
                let age = (state.frame - live.born) as f32;
                let place = live.place.under(Place {
                    origin: live.at + def.position.at(age),
                    turn: rotation(def.rotation.at(age) + read(&def.spin, age) * age),
                    scale: def.scale.at(age),
                });
                Drawn {
                    center: place.origin.to_array(),
                    scale: place.scale.to_array(),
                    turn: place.turn.to_array(),
                    color: (live.tint * def.color.at(age)).to_array(),
                    texture: def.texture,
                    shape: def.shape,
                    blend: def.blend,
                }
            })
            .collect()
    }

    /// A sphere the whole run fits inside, for the camera to open on.
    pub fn fit(&self) -> (Vec3, f32) {
        let mut state = State::default();
        let (mut low, mut high) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for _ in 0..self.length.min(FITTED) {
            self.step(&mut state);
            for live in &state.particles {
                let def = &self.particles[live.def];
                let age = (state.frame - live.born) as f32;
                let at = live.place.origin
                    + live.place.turn * ((live.at + def.position.at(age)) * live.place.scale);
                let reach = (def.scale.at(age) * live.place.scale)
                    .abs()
                    .max_element()
                    .max(0.05);
                low = low.min(at - reach);
                high = high.max(at + reach);
            }
        }
        match low.cmple(high).all() {
            true => ((low + high) * 0.5, ((high - low).length() * 0.5).max(0.1)),
            false => (Vec3::ZERO, 1.0),
        }
    }
}
