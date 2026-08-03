//! What an effect animates from: a list of keyframes, what the curve does either side of them, and
//! how both are drawn.

use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, Vec2, pos2, vec2};
use ironworks::file::avfx::{Block, CurveBehaviour, CurveKey, KeyKind, RandomKind};

/// How far past the keyed span a curve is drawn, as a fraction of that span, so what it does either
/// side of its keys is visible.
const MARGIN: f32 = 0.25;

/// Room left above and below the values a plot covers.
const HEADROOM: f32 = 0.08;

/// Size a keyframe marker is drawn at.
const MARKER: f32 = 3.0;

/// Height a plot is drawn at.
const PLOT: f32 = 110.0;

/// Size of the sparkline that sits on a tree row.
pub(super) const SPARK: Vec2 = vec2(56.0, 14.0);

/// Most samples a curve is drawn from, whatever the width it is drawn across.
const SAMPLES: usize = 512;

/// One animated value.
pub struct Curve {
    /// The tag the curve was written under, for the panel that stacks several of them.
    pub(super) name: String,

    /// The keyframes, in time order.
    pub(super) keys: Vec<CurveKey>,

    pub(super) pre: CurveBehaviour,
    pub(super) post: CurveBehaviour,

    /// `RanT`, where the curve carries one.
    pub(super) random: Option<RandomKind>,

    /// Whether the keys carry a color: an `RGB` curve writes a channel in each of its three
    /// floats, where every other curve writes two tangent scales and a value.
    pub(super) color: bool,
}

/// The curve a block holds, where it holds one: any block carrying a key list.
pub fn read(block: &Block) -> Option<Curve> {
    let mut keys = block.find("Keys")?.keys()?.to_vec();
    keys.sort_by_key(CurveKey::time);
    Some(Curve {
        name: block.name().to_string(),
        pre: block
            .find("BvPr")
            .and_then(Block::i32)
            .unwrap_or_default()
            .into(),
        post: block
            .find("BvPo")
            .and_then(Block::i32)
            .unwrap_or_default()
            .into(),
        random: block
            .find("RanT")
            .and_then(Block::i32)
            .map(RandomKind::from),
        color: block.name() == "RGB",
        keys,
    })
}

impl Curve {
    /// What the curve's row says beside its tag.
    pub fn summary(&self) -> String {
        let keys = match self.keys.len() {
            1 => "1 key".to_owned(),
            count => format!("{count} keys"),
        };
        match self.span() {
            Some((start, end)) if end > start => format!("{keys}  {start}..{end}"),
            _ => keys,
        }
    }

    /// The value at `time`, all three floats interpolated together: a color needs every one of
    /// them and a scalar reads the last.
    pub fn sample(&self, time: f32) -> [f32; 3] {
        let (Some(first), Some(last)) = (self.keys.first(), self.keys.last()) else {
            return [0.0; 3];
        };
        let (start, end) = (f32::from(first.time()), f32::from(last.time()));
        if end <= start {
            return first.data();
        }

        let behaviour = match time < start {
            true => self.pre,
            false => self.post,
        };
        let mut cycles = 0.0;
        let time = match (time < start || time > end, behaviour) {
            (true, CurveBehaviour::Repeat | CurveBehaviour::Add) => {
                cycles = ((time - start) / (end - start)).floor();
                time - cycles * (end - start)
            }
            _ => time.clamp(start, end),
        };

        let value = self.interpolate(time);
        match behaviour {
            CurveBehaviour::Add => std::array::from_fn(|axis| {
                value[axis] + cycles * (last.data()[axis] - first.data()[axis])
            }),
            _ => value,
        }
    }

    /// The color at `time`, for the curves whose keys hold one.
    pub fn swatch(&self, time: f32) -> Color32 {
        let [r, g, b] = self.sample(time).map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
        Color32::from_rgb(r, g, b)
    }

    fn span(&self) -> Option<(i16, i16)> {
        Some((self.keys.first()?.time(), self.keys.last()?.time()))
    }

    /// The value inside the keyed span, where `time` is known to sit between two keys.
    fn interpolate(&self, time: f32) -> [f32; 3] {
        let index = self
            .keys
            .partition_point(|key| f32::from(key.time()) <= time)
            .saturating_sub(1)
            .min(self.keys.len() - 2);
        let (before, after) = (&self.keys[index], &self.keys[index + 1]);
        let (start, end) = (f32::from(before.time()), f32::from(after.time()));
        let along = (time - start) / (end - start);

        match before.kind() {
            // A key holds until the next one, which the next one then takes over from.
            KeyKind::Step => match along < 1.0 {
                true => before.data(),
                false => after.data(),
            },
            // The tangent scales the key carries beside its value go unread, so a spline is drawn
            // through its keys rather than along the shape they would give it.
            KeyKind::Spline => {
                let (from, to) = (self.tangent(index), self.tangent(index + 1));
                let (t2, t3) = (along * along, along * along * along);
                std::array::from_fn(|axis| {
                    (2.0 * t3 - 3.0 * t2 + 1.0) * before.data()[axis]
                        + (t3 - 2.0 * t2 + along) * (end - start) * from[axis]
                        + (-2.0 * t3 + 3.0 * t2) * after.data()[axis]
                        + (t3 - t2) * (end - start) * to[axis]
                })
            }
            _ => std::array::from_fn(|axis| {
                before.data()[axis] + along * (after.data()[axis] - before.data()[axis])
            }),
        }
    }

    /// How fast the curve is moving at a key, taken from the keys either side of it.
    fn tangent(&self, index: usize) -> [f32; 3] {
        let before = &self.keys[index.saturating_sub(1)];
        let after = &self.keys[(index + 1).min(self.keys.len() - 1)];
        let span = f32::from(after.time()) - f32::from(before.time());
        match span > 0.0 {
            true => std::array::from_fn(|axis| (after.data()[axis] - before.data()[axis]) / span),
            false => [0.0; 3],
        }
    }
}

/// The window a curve is drawn across: its keyed span, reaching past it either side so the pre and
/// post behaviours show.
fn window(curve: &Curve) -> (f32, f32) {
    let Some((start, end)) = curve.span() else {
        return (0.0, 1.0);
    };
    let (start, end) = (f32::from(start), f32::from(end));
    let margin = ((end - start) * MARGIN).max(1.0);
    (start - margin, end + margin)
}

/// Draws `curve` into `rect`. `detailed` marks the keys and the edges of the keyed span, which a
/// plot has room for and a sparkline does not. Returns the value range the plot covered.
fn paint(ui: &egui::Ui, curve: &Curve, rect: Rect, detailed: bool) -> Option<(f32, f32)> {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let (Some(first), Some(last)) = (curve.keys.first(), curve.keys.last()) else {
        return None;
    };

    let (from, to) = window(curve);
    let steps = (rect.width() as usize).clamp(2, SAMPLES);
    let along = |step: usize| step as f32 / (steps - 1) as f32;
    let values: Vec<[f32; 3]> = (0..steps)
        .map(|step| curve.sample(from + (to - from) * along(step)))
        .collect();
    let x = |time: f32| rect.left() + rect.width() * (time - from) / (to - from);

    if curve.color {
        let width = rect.width() / (steps - 1) as f32;
        for (step, value) in values.iter().enumerate() {
            let [r, g, b] = value.map(|channel| (channel.clamp(0.0, 1.0) * 255.0) as u8);
            let at = pos2(rect.left() + rect.width() * along(step), rect.center().y);
            painter.rect_filled(
                Rect::from_center_size(at, vec2(width + 1.0, rect.height())),
                0.0,
                Color32::from_rgb(r, g, b),
            );
        }
        if detailed {
            for key in &curve.keys {
                let at = x(f32::from(key.time()));
                painter.vline(
                    at,
                    rect.bottom() - MARKER * 2.0..=rect.bottom(),
                    Stroke::new(1.0, ui.visuals().strong_text_color()),
                );
            }
        }
        return None;
    }

    let (low, high) = values
        .iter()
        .fold((f32::MAX, f32::MIN), |(low, high), value| {
            (low.min(value[2]), high.max(value[2]))
        });
    let (low, high) = match high > low {
        true => (
            low - (high - low) * HEADROOM,
            high + (high - low) * HEADROOM,
        ),
        false => (low - 1.0, high + 1.0),
    };
    let y = |value: f32| rect.bottom() - rect.height() * (value - low) / (high - low);

    let points: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(step, value)| pos2(rect.left() + rect.width() * along(step), y(value[2])))
        .collect();
    let accent = ui.visuals().selection.stroke.color;
    let faint = Stroke::new(1.0, accent.gamma_multiply(0.35));
    let split = |time: f32| {
        (((time - from) / (to - from) * (steps - 1) as f32).round() as usize).min(steps - 1)
    };
    let (start, end) = (
        split(f32::from(first.time())),
        split(f32::from(last.time())),
    );
    painter.add(Shape::line(points[..=start].to_vec(), faint));
    painter.add(Shape::line(
        points[start..=end].to_vec(),
        Stroke::new(1.5, accent),
    ));
    painter.add(Shape::line(points[end..].to_vec(), faint));

    if detailed {
        let strong = ui.visuals().strong_text_color();
        for key in &curve.keys {
            let at = pos2(x(f32::from(key.time())), y(key.data()[2]));
            match key.kind() {
                KeyKind::Step => painter.rect_filled(
                    Rect::from_center_size(at, Vec2::splat(MARKER * 1.5)),
                    0.0,
                    strong,
                ),
                KeyKind::Spline => painter.add(Shape::convex_polygon(
                    vec![
                        pos2(at.x, at.y - MARKER * 1.3),
                        pos2(at.x + MARKER * 1.3, at.y),
                        pos2(at.x, at.y + MARKER * 1.3),
                        pos2(at.x - MARKER * 1.3, at.y),
                    ],
                    strong,
                    Stroke::NONE,
                )),
                _ => painter.circle_filled(at, MARKER * 0.9, strong),
            };
        }
    }
    Some((low, high))
}

/// A frame count as the time it falls at, at `rate` frames a second.
pub fn seconds(frames: f32, rate: f32) -> String {
    format!("{:.2}s", frames / rate)
}

/// One curve, drawn large enough to read. Returns the value range it covered.
pub fn plot(ui: &mut egui::Ui, curve: &Curve, rate: f32) -> Option<(f32, f32)> {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), PLOT), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return None;
    }
    let range = paint(ui, curve, rect, true);

    if let Some(pointer) = response.hover_pos() {
        let (from, to) = window(curve);
        let time = from + (to - from) * (pointer.x - rect.left()) / rect.width();
        ui.painter_at(rect).vline(
            pointer.x,
            rect.y_range(),
            Stroke::new(1.0, ui.visuals().weak_text_color()),
        );
        let value = curve.sample(time);
        let at = format!("frame {time:.0}  {}", seconds(time, rate));
        response.on_hover_text(match curve.color {
            true => {
                let [r, g, b] = value.map(|channel| (channel.clamp(0.0, 1.0) * 255.0) as u8);
                format!("{at}   {r}, {g}, {b}")
            }
            false => format!("{at}   {:.4}", value[2]),
        });
    }
    range
}

/// The same curve, small enough to sit on a tree row.
pub fn spark(ui: &mut egui::Ui, curve: &Curve) {
    let (rect, _) = ui.allocate_exact_size(SPARK, Sense::hover());
    if ui.is_rect_visible(rect) {
        paint(ui, curve, rect, false);
    }
}
