//! `.uld` layouts: a screen of the game's interface.

use anyhow::Result;
use egui::{
    Align2, Button, Color32, Rect, RichText, ScrollArea, Sense, Vec2,
    collapsing_header::paint_default_icon, load::SizedTexture, pos2, vec2,
};
use ironworks::file::{
    File,
    uld::{self, NodeKind},
};
use std::collections::HashSet;
use std::io::Cursor;

use super::{Preview, link, section};
use crate::assets::deps::{Dep, Deps};
use crate::backend::Backend;
use crate::utils::file_name;

/// Edge length a texture thumbnail is drawn at, matching the material viewer.
const THUMBNAIL: f32 = 64.0;

/// Longest edge a sprite is drawn at in the parts list.
const SPRITE: f32 = 56.0;

/// How far each level of the node tree is indented.
const INDENT: f32 = 14.0;

/// Deepest indent the tree draws, so a deeply instanced node still leaves room for its label in a
/// narrow panel.
const MAX_INDENT: usize = 8;

/// Width reserved for a tree row's disclosure triangle.
const TRIANGLE: f32 = 12.0;

/// A tree row's visibility toggle.
const EYE: &str = "👁";

/// Width the toggle is given, ahead of the indent rather than after it so the column stays straight
/// however deep the row sits.
const GUTTER: f32 = 18.0;

/// Height the node tree takes before it scrolls on its own, leaving the panel below it for the
/// selected node's properties.
const TREE_HEIGHT: f32 = 260.0;

/// Characters of a node's text a tree row shows before cutting it short.
const SNIPPET: usize = 24;

/// Smallest a font is drawn at on the canvas. A layout shrunk far enough for its text to fall under
/// this is better off without the smudge.
const LEGIBLE: f32 = 4.0;

/// Bits of a text node's second flag byte that hand its colors to the current UI theme, leaving
/// the color fields holding a palette row rather than a color.
const THEME_FILL: u8 = 0x02;
const THEME_EDGE: u8 = 0x04;

/// How deep component instancing is followed. Components may only reference ones defined before
/// them.
const MAX_DEPTH: usize = 16;

/// How many repeats a tiling piece is drawn as before it is simply stretched. A window background
/// tiles a 32px cell across a few hundred pixels; anything far past that is not worth the quads.
const MAX_TILES: f32 = 4096.0;

/// A rectangle of a texture, resolved to the file it lives in.
#[derive(Clone)]
struct Sprite {
    /// `None` when the part names a texture its own layout does not declare.
    texture: Option<String>,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

/// One sprite painted into one rectangle of the canvas. A plain image is a single piece; a
/// nine-grid is up to nine, each with its own source rectangle and often its own texture.
struct Piece {
    sprite: Sprite,
    dest: Rect,
    /// Whether the sprite repeats to fill `dest` rather than stretching to it.
    tile: bool,
}

/// One node's worth of painting, positioned in the widget's own coordinates.
struct DrawItem {
    pieces: Vec<Piece>,
    tint: Color32,
    /// The node's own box. A resource or collision node paints nothing but still occupies this,
    /// which is what the inspector outlines and picks against.
    bounds: Rect,
    /// Which [`NodeRow`] this came from, so a pick can name the node it hit.
    row: usize,
}

/// A texture the layout draws from.
struct TextureRow {
    id: u32,
    path: String,
}

struct PartListRow {
    id: u32,
    parts: Vec<Sprite>,
}

/// A node, flattened out of the tree with its depth so drawing needs no recursion.
struct NodeRow {
    depth: usize,
    id: u32,
    kind: String,
    geometry: String,
    detail: String,
    visible: bool,
    /// Everything the node carries, for the panel that inspects one.
    props: Vec<(&'static str, String)>,
    /// Where this node's text is kept, for the ones naming a row rather than being filled in as
    /// the game runs.
    text: Option<TextRef>,
    parent: Option<usize>,
    children: Vec<usize>,
    /// The innermost component instance this node came from. Pointing at a component's insides on
    /// the canvas selects the instance, the way a click lands on a widget rather than its parts.
    component: Option<usize>,
    /// Whether this node's children are an instanced component's, which start collapsed.
    instances: bool,
}

/// The row a text node draws, and how it draws it. A layout carries no text of its own: it names a
/// row of `Addon`, or of `Lobby` for the screens that run before a character is loaded.
struct TextRef {
    sheet: &'static str,
    row: u32,
    align: Align2,
    size: f32,
    /// `None` where the node takes the current UI theme's color, which is what nearly all of them
    /// do; the rest state one outright.
    color: Option<Color32>,
}

struct WidgetRow {
    id: u32,
    summary: String,
    nodes: Vec<NodeRow>,
    items: Vec<DrawItem>,
    /// Size of the composed layout, in its own coordinates.
    extent: Vec2,
}

/// A layout, decoded and ready to draw.
pub struct Rendered {
    textures: Vec<TextureRow>,
    part_lists: Vec<PartListRow>,
    widgets: Vec<WidgetRow>,
    identity: Vec<(&'static str, String)>,
    /// Timeline id, animations, label sets, and total key groups across the animations.
    timelines: Vec<(u32, usize, usize, usize)>,
    /// Where the pinned node is kept. The canvas sets it and the details panel reads it, and the
    /// two are drawn by different callers, so it lives in egui's store keyed by the file rather
    /// than in here -- opening another layout starts with nothing pinned.
    selection: egui::Id,
}

/// The node the inspector has pinned, as an index into [`Rendered::widgets`] and that widget's
/// nodes.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Selected(usize, usize);

/// What the tree remembers between frames, kept beside the selection and for the same reason: the
/// rows collapsed away from their default, and the ones switched off. Keyed by widget and row.
#[derive(Clone, Default)]
struct Toggles {
    open: HashSet<(usize, usize)>,
    hidden: HashSet<(usize, usize)>,
}

/// What a click on the canvas landed on.
enum Pick {
    Node(usize),
    Nothing,
}

/// Where a texture entry's pixels live. An entry with no path names an icon by id instead, which
/// resolves to a path by convention.
fn texture_path(texture: &uld::Texture) -> String {
    match texture.path().is_empty() {
        false => texture.path().to_owned(),
        true => {
            let id = texture.icon_id();
            format!("ui/icon/{:06}/{:06}.tex", id / 1000 * 1000, id)
        }
    }
}

impl Sprite {
    /// The part of this sprite `rect` covers, in the sprite's own pixels.
    fn sub(&self, x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            texture: self.texture.clone(),
            x: self.x + x as u16,
            y: self.y + y as u16,
            width: width as u16,
            height: height as u16,
        }
    }
}

fn sprite_of(layout: &uld::UiLayout, list_id: u32, part_id: u32) -> Option<Sprite> {
    let part = layout
        .part_list(list_id)?
        .parts()
        .get(usize::try_from(part_id).ok()?)?;
    Some(Sprite {
        texture: layout.texture(part.texture_id()).map(texture_path),
        x: part.u(),
        y: part.v(),
        width: part.width(),
        height: part.height(),
    })
}

/// What a text node draws, or `None` where there is nothing to look up: row zero is the game
/// writing the string itself as it runs, which is most of them.
fn text_ref(node: &uld::Node) -> Option<TextRef> {
    let NodeKind::Text(text) = node.kind() else {
        return None;
    };
    let sheet = match text.sheet_type {
        0 => "Addon",
        1 => "Lobby",
        _ => return None,
    };
    (text.text_id != 0).then(|| TextRef {
        sheet,
        row: text.text_id,
        align: aligned(text.alignment),
        size: f32::from(text.font_size),
        color: (text.flags2 & THEME_FILL == 0).then(|| color_of(text.color)),
    })
}

/// A color as a text node states it: the channels in memory order, so the byte written first is
/// red rather than the alpha the hex reading suggests.
fn color_of(value: u32) -> Color32 {
    let [r, g, b, a] = value.to_le_bytes();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Where in its own box a node's text sits.
fn aligned(alignment: u8) -> Align2 {
    match uld::Alignment::from(alignment) {
        uld::Alignment::TopLeft | uld::Alignment::Unknown(_) => Align2::LEFT_TOP,
        uld::Alignment::Top => Align2::CENTER_TOP,
        uld::Alignment::TopRight => Align2::RIGHT_TOP,
        uld::Alignment::Left => Align2::LEFT_CENTER,
        uld::Alignment::Center => Align2::CENTER_CENTER,
        uld::Alignment::Right => Align2::RIGHT_CENTER,
        uld::Alignment::BottomLeft => Align2::LEFT_BOTTOM,
        uld::Alignment::Bottom => Align2::CENTER_BOTTOM,
        uld::Alignment::BottomRight => Align2::RIGHT_BOTTOM,
    }
}

/// A node's tint: the per-channel multiply, where 100 leaves a channel alone, with alpha folded in.
fn tint(node: &uld::Node) -> Color32 {
    let channel = |value: i16| u8::try_from(i32::from(value) * 255 / 100).unwrap_or(u8::MAX);
    let [r, g, b] = node.multiply();
    Color32::from_rgba_unmultiplied(channel(r), channel(g), channel(b), node.alpha())
}

/// Where a child sits once its parent has been resized, in the parent's coordinates.
///
/// A node anchored to both edges of an axis stretches along it, one anchored to the far edge alone
/// rides that edge at its own size, and one anchored to the near edge alone stays where it was
/// authored. A node marked to fill takes the parent's box outright. `delta` is how much the parent
/// grew by, so an unresized parent leaves every child exactly as written.
fn placed(node: &uld::Node, parent: Vec2, delta: Vec2) -> Rect {
    let flags = node.flags();
    let axis = |near, far, at: f32, size: f32, parent: f32, delta: f32| match (near, far) {
        _ if flags.fill() => (0.0, parent),
        (true, true) => (at, size + delta),
        (false, true) => (at + delta, size),
        _ => (at, size),
    };

    let (x, width) = axis(
        flags.anchor_left(),
        flags.anchor_right(),
        f32::from(node.x()),
        f32::from(node.width()),
        parent.x,
        delta.x,
    );
    let (y, height) = axis(
        flags.anchor_top(),
        flags.anchor_bottom(),
        f32::from(node.y()),
        f32::from(node.height()),
        parent.y,
        delta.y,
    );
    Rect::from_min_size(pos2(x, y), vec2(width, height))
}

/// A node's kind as a short label, plus whatever else is worth a line about it.
/// What a component is called, falling back to the generic name for a kind ironworks does not know.
fn component_kind(kind: uld::ComponentKind) -> String {
    match kind {
        uld::ComponentKind::Unknown(_) => "Component".to_owned(),
        kind => format!("{kind:?}"),
    }
}

fn describe(node: &uld::Node, layout: &uld::UiLayout) -> (String, String) {
    let part = |list: u32, part: u32| format!("part {part} of list {list}");
    match node.kind() {
        NodeKind::Res => ("Res".to_owned(), String::new()),
        NodeKind::Image(image) => ("Image".to_owned(), part(image.part_list_id, image.part_id)),
        NodeKind::Text(text) => (
            "Text".to_owned(),
            format!("text {}, {:?} {}", text.text_id, text.font, text.font_size),
        ),
        NodeKind::NineGrid(grid) => ("NineGrid".to_owned(), part(grid.part_list_id, grid.part_id)),
        NodeKind::Counter(counter) => (
            "Counter".to_owned(),
            format!("list {}", counter.part_list_id),
        ),
        NodeKind::Collision(_) => ("Collision".to_owned(), String::new()),
        NodeKind::ClippingMask(mask) => (
            "ClippingMask".to_owned(),
            part(mask.part_list_id, mask.part_id),
        ),
        // The tag names a component, and the node is an instance of it -- a prefab of further nodes,
        // which get listed underneath. It goes by what the component is, so a window reads as a
        // Window rather than as an id to look up.
        NodeKind::Component { component_id, .. } => (
            layout
                .component(*component_id)
                .map_or_else(|| "Component".to_owned(), |c| component_kind(c.kind())),
            format!("component {component_id}"),
        ),
        NodeKind::Unknown { node_type, .. } => {
            (format!("Type {node_type}"), "unmodelled".to_owned())
        }
    }
}

/// One of a text node's two color fields as written: a row of the theme's palette where the node
/// defers to it, and a color of its own otherwise.
fn stated(value: u32, themed: bool) -> String {
    if themed {
        return format!("theme {value}");
    }
    let [r, g, b, a] = value.to_le_bytes();
    match a {
        0xff => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => format!("#{r:02x}{g:02x}{b:02x}{a:02x}"),
    }
}

/// Everything a node carries, for the panel that inspects one. `rect` is where it actually landed,
/// which is worth stating next to what it was authored as because instancing moves it.
fn properties(node: &uld::Node, layout: &uld::UiLayout, rect: Rect) -> Vec<(&'static str, String)> {
    let mut props = vec![
        ("Node", node.id().to_string()),
        ("Type", node.node_type().to_string()),
        ("Position", format!("{}, {}", node.x(), node.y())),
        ("Size", format!("{} x {}", node.width(), node.height())),
    ];
    if rect.min != pos2(f32::from(node.x()), f32::from(node.y()))
        || rect.size() != vec2(f32::from(node.width()), f32::from(node.height()))
    {
        props.push((
            "Drawn at",
            format!(
                "{}, {}  {} x {}",
                rect.min.x.round(),
                rect.min.y.round(),
                rect.width().round(),
                rect.height().round()
            ),
        ));
    }

    props.push(("Flags", format!("{:?}", node.flags()).replace('"', "")));
    if node.parent_id() != 0 {
        props.push(("Parent", node.parent_id().to_string()));
    }
    if node.alpha() != 255 {
        props.push(("Alpha", node.alpha().to_string()));
    }
    let [mr, mg, mb] = node.multiply();
    if [mr, mg, mb] != [100, 100, 100] {
        props.push(("Multiply", format!("{mr}, {mg}, {mb}")));
    }
    let [ar, ag, ab] = node.add();
    if [ar, ag, ab] != [0, 0, 0] {
        props.push(("Add", format!("{ar}, {ag}, {ab}")));
    }
    if node.rotation() != 0.0 {
        props.push(("Rotation", format!("{:.3}", node.rotation())));
    }
    if (node.scale_x(), node.scale_y()) != (1.0, 1.0) {
        props.push((
            "Scale",
            format!("{:.3}, {:.3}", node.scale_x(), node.scale_y()),
        ));
    }
    if (node.origin_x(), node.origin_y()) != (0, 0) {
        props.push((
            "Origin",
            format!("{}, {}", node.origin_x(), node.origin_y()),
        ));
    }
    if node.timeline_id() != 0 {
        props.push(("Timeline", node.timeline_id().to_string()));
    }

    let part = |list: u32, id: u32| {
        let sprite = sprite_of(layout, list, id);
        let where_ = format!("part {id} of list {list}");
        match sprite {
            Some(sprite) => vec![
                ("Part", where_),
                (
                    "Sprite",
                    format!(
                        "{},{}  {} x {}",
                        sprite.x, sprite.y, sprite.width, sprite.height
                    ),
                ),
                (
                    "Texture",
                    sprite.texture.unwrap_or_else(|| "undeclared".to_owned()),
                ),
            ],
            None => vec![("Part", format!("{where_} (missing)"))],
        }
    };

    match node.kind() {
        NodeKind::Image(image) => {
            props.extend(part(image.part_list_id, image.part_id));
            if image.flip_horizontal != 0 || image.flip_vertical != 0 {
                props.push((
                    "Flip",
                    format!("{}, {}", image.flip_horizontal, image.flip_vertical),
                ));
            }
            props.push(("Wrap", image.wrap.to_string()));
        }
        NodeKind::NineGrid(grid) => {
            props.extend(part(grid.part_list_id, grid.part_id));
            props.push((
                "Insets",
                format!(
                    "l{} r{} t{} b{}",
                    grid.left_offset, grid.right_offset, grid.top_offset, grid.bottom_offset
                ),
            ));
            props.push((
                "Grid",
                match grid.parts_type {
                    1 => "nine parts",
                    _ => "one part, divided",
                }
                .to_owned(),
            ));
            props.push((
                "Middle",
                match grid.render_type {
                    1 => "tiled",
                    _ => "stretched",
                }
                .to_owned(),
            ));
        }
        NodeKind::ClippingMask(mask) => props.extend(part(mask.part_list_id, mask.part_id)),
        NodeKind::Counter(counter) => {
            props.extend(part(counter.part_list_id, u32::from(counter.part_id)));
            props.push(("Digit", format!("{} wide", counter.number_width)));
        }
        NodeKind::Text(text) => {
            props.push((
                "Text",
                match text.sheet_type {
                    0 => format!("{} (addon)", text.text_id),
                    1 => format!("{} (lobby)", text.text_id),
                    sheet => format!("{} (sheet {sheet})", text.text_id),
                },
            ));
            props.push(("Font", format!("{:?} {}", text.font, text.font_size)));
            props.push(("Color", stated(text.color, text.flags2 & THEME_FILL != 0)));
            props.push((
                "Edge",
                stated(text.edge_color, text.flags2 & THEME_EDGE != 0),
            ));
            props.push(("Align", text.alignment.to_string()));
            props.push(("Style", format!("{:?}", text.flags).replace('"', "")));
        }
        NodeKind::Component {
            component_id,
            instance,
        } => {
            props.push((
                "Component",
                match layout.component(*component_id) {
                    Some(component) => format!("{component_id} ({:?})", component.kind()),
                    None => format!("{component_id} (undefined)"),
                },
            ));
            props.push(("Instance", format!("{:?}", instance.index)));
        }
        NodeKind::Unknown { node_type, data } => {
            props.push(("Payload", format!("type {node_type}, {} bytes", data.len())));
        }
        NodeKind::Res | NodeKind::Collision(_) => {}
    }
    props
}

/// Everything needed while walking one tree, so the recursion carries two arguments rather than
/// eight.
struct Walk<'a> {
    layout: &'a uld::UiLayout,
    rows: Vec<NodeRow>,
    items: Vec<DrawItem>,
    /// Components currently being expanded, so a file whose components reference each other in a
    /// loop stops rather than recursing forever.
    open: Vec<u32>,
}

impl Walk<'_> {
    /// Both walk a bare node slice, since the recursion crosses from a widget's nodes into a
    /// component's. Reversed for the same reason `Widget::children` is: siblings are held
    /// backwards through the file, so the last one written is the first drawn.
    fn roots(nodes: &[uld::Node]) -> impl Iterator<Item = &uld::Node> {
        nodes.iter().rev().filter(|node| node.parent_id() == 0)
    }

    fn children(nodes: &[uld::Node], id: u32) -> impl Iterator<Item = &uld::Node> {
        let parent = i32::try_from(id).unwrap_or(-1);
        nodes
            .iter()
            .rev()
            .filter(move |node| node.parent_id() == parent)
    }

    /// `rect` is where this node ends up, which the caller has already resolved against whatever
    /// resizing its parent went through.
    fn node(
        &mut self,
        nodes: &[uld::Node],
        node: &uld::Node,
        rect: Rect,
        depth: usize,
        parent: Option<usize>,
        component: Option<usize>,
    ) {
        let (kind, detail) = describe(node, self.layout);
        let declared = vec2(f32::from(node.width()), f32::from(node.height()));
        let row = self.rows.len();
        self.rows.push(NodeRow {
            depth,
            id: node.id(),
            kind,
            geometry: format!(
                "{},{}  {}x{}",
                node.x(),
                node.y(),
                node.width(),
                node.height()
            ),
            detail,
            visible: node.flags().visible(),
            props: properties(node, self.layout, rect),
            text: text_ref(node),
            parent,
            children: Vec::new(),
            component,
            instances: false,
        });
        if let Some(parent) = parent {
            self.rows[parent].children.push(row);
        }

        // A hidden node still positions its children, so its subtree is walked either way; only
        // its own painting is skipped.
        if node.flags().visible() {
            self.paint(node, rect, row);
        }

        // How much this node was stretched by is what its own children have to resolve against.
        let delta = rect.size() - declared;

        // Instancing a component stretches it to the size the instance asks for, which is how one
        // 144x144 window prefab becomes every window in the game.
        let layout = self.layout;
        if let NodeKind::Component { component_id, .. } = node.kind()
            && depth < MAX_DEPTH
            && !self.open.contains(component_id)
            && let Some(component) = layout.component(*component_id)
        {
            self.open.push(*component_id);
            self.rows[row].instances = true;
            let inner = component.nodes();
            for root in Self::roots(inner) {
                let root_size = vec2(f32::from(root.width()), f32::from(root.height()));
                let at = placed(root, rect.size(), rect.size() - root_size);
                let at = at.translate(rect.min.to_vec2());
                self.node(inner, root, at, depth + 1, Some(row), Some(row));
            }
            self.open.pop();
        }

        for child in Self::children(nodes, node.id()) {
            let at = placed(child, rect.size(), delta);
            let at = at.translate(rect.min.to_vec2());
            self.node(nodes, child, at, depth + 1, Some(row), component);
        }
    }

    fn paint(&mut self, node: &uld::Node, rect: Rect, row: usize) {
        let pieces = match node.kind() {
            NodeKind::Image(image) => sprite_of(self.layout, image.part_list_id, image.part_id)
                .map(|sprite| {
                    vec![Piece {
                        sprite,
                        dest: rect,
                        tile: false,
                    }]
                })
                .unwrap_or_default(),
            NodeKind::NineGrid(grid) => nine_grid(self.layout, grid, rect),
            _ => Vec::new(),
        };
        // Pushed even with nothing to paint: a node that only groups or catches the pointer is
        // still something to hover and inspect.
        self.items.push(DrawItem {
            pieces,
            tint: tint(node),
            bounds: rect,
            row,
        });
    }
}

/// The pieces a nine-grid paints as.
///
/// The two kinds are laid out differently on disk. A *divided* grid is one part cut up by the
/// insets it declares. A *composed* one is nine consecutive parts -- often from nine different
/// textures, as the window frames are -- whose own sizes give the border widths, leaving the insets
/// unused. Either way the middle stretches, or tiles when the grid says so.
fn nine_grid(layout: &uld::UiLayout, grid: &uld::NineGrid, dest: Rect) -> Vec<Piece> {
    const COMPOSE: u8 = 1;
    const TILE: u8 = 1;
    let tile = grid.render_type == TILE;

    let cells: Vec<(Sprite, usize, usize)> = match grid.parts_type == COMPOSE {
        true => (0..9)
            .filter_map(|index| {
                let sprite = sprite_of(layout, grid.part_list_id, grid.part_id + index as u32)?;
                Some((sprite, index % 3, index / 3))
            })
            .collect(),
        false => {
            let Some(sprite) = sprite_of(layout, grid.part_list_id, grid.part_id) else {
                return Vec::new();
            };
            let (w, h) = (f32::from(sprite.width), f32::from(sprite.height));
            let (left, right) = fit(f32::from(grid.left_offset), f32::from(grid.right_offset), w);
            let (top, bottom) = fit(f32::from(grid.top_offset), f32::from(grid.bottom_offset), h);
            let columns = [(0.0, left), (left, w - left - right), (w - right, right)];
            let rows = [(0.0, top), (top, h - top - bottom), (h - bottom, bottom)];
            rows.iter()
                .enumerate()
                .flat_map(|(row, &(y, height))| {
                    columns
                        .iter()
                        .enumerate()
                        .map(move |(column, &(x, width))| (row, column, x, y, width, height))
                })
                .map(|(row, column, x, y, width, height)| {
                    (sprite.sub(x, y, width, height), column, row)
                })
                .collect()
        }
    };
    if cells.len() != 9 {
        return Vec::new();
    }

    // Borders keep their own size; whatever is left over goes to the middle.
    let (left, right) = fit(
        f32::from(cells[0].0.width),
        f32::from(cells[2].0.width),
        dest.width(),
    );
    let (top, bottom) = fit(
        f32::from(cells[0].0.height),
        f32::from(cells[6].0.height),
        dest.height(),
    );
    let columns = [
        (dest.left(), left),
        (dest.left() + left, dest.width() - left - right),
        (dest.right() - right, right),
    ];
    let rows = [
        (dest.top(), top),
        (dest.top() + top, dest.height() - top - bottom),
        (dest.bottom() - bottom, bottom),
    ];

    cells
        .into_iter()
        .filter_map(|(sprite, column, row)| {
            let (x, width) = columns[column];
            let (y, height) = rows[row];
            (width > 0.0 && height > 0.0 && sprite.width > 0 && sprite.height > 0).then(|| Piece {
                sprite,
                dest: Rect::from_min_size(pos2(x, y), vec2(width, height)),
                tile,
            })
        })
        .collect()
}

/// Shrink a pair of border sizes so they leave something between them.
fn fit(a: f32, b: f32, total: f32) -> (f32, f32) {
    let (a, b) = (a.max(0.0), b.max(0.0));
    match a + b > total && a + b > 0.0 {
        true => (a * total / (a + b), b * total / (a + b)),
        false => (a, b),
    }
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let layout = uld::UiLayout::read(Cursor::new(bytes.to_vec()))?;

    let textures = layout
        .textures()
        .iter()
        .map(|texture| TextureRow {
            id: texture.id(),
            path: texture_path(texture),
        })
        .collect::<Vec<_>>();

    let part_lists = layout
        .part_lists()
        .iter()
        .map(|list| PartListRow {
            id: list.id(),
            parts: (0..list.parts().len())
                .filter_map(|index| sprite_of(&layout, list.id(), index as u32))
                .collect(),
        })
        .collect::<Vec<_>>();

    let widgets = layout
        .widgets()
        .iter()
        .map(|widget| {
            let mut walk = Walk {
                layout: &layout,
                rows: Vec::new(),
                items: Vec::new(),
                open: Vec::new(),
            };
            let nodes = widget.nodes();
            for root in Walk::roots(nodes) {
                let size = vec2(f32::from(root.width()), f32::from(root.height()));
                let at = pos2(f32::from(root.x()), f32::from(root.y()));
                walk.node(nodes, root, Rect::from_min_size(at, size), 0, None, None);
            }
            // The composed size is whatever the nodes actually cover, which a root sized zero --
            // or one whose children hang outside it -- would otherwise get wrong.
            let extent = walk
                .items
                .iter()
                .flat_map(|item| item.pieces.iter().map(|piece| piece.dest))
                .chain(Walk::roots(nodes).map(|n| {
                    Rect::from_min_size(
                        pos2(f32::from(n.x()), f32::from(n.y())),
                        vec2(f32::from(n.width()), f32::from(n.height())),
                    )
                }))
                .reduce(Rect::union)
                .map_or(Vec2::ZERO, |rect| rect.max.to_vec2());

            WidgetRow {
                id: widget.id(),
                summary: format!(
                    "{:?} at {},{}  {} nodes",
                    widget.alignment(),
                    widget.x(),
                    widget.y(),
                    nodes.len()
                ),
                nodes: walk.rows,
                items: walk.items,
                extent,
            }
        })
        .collect::<Vec<_>>();

    let identity = vec![
        ("Version", format!("{:?}", layout.version())),
        ("Textures", layout.textures().len().to_string()),
        ("Part lists", layout.part_lists().len().to_string()),
        ("Components", layout.components().len().to_string()),
        ("Widgets", layout.widgets().len().to_string()),
    ];

    let timelines = layout
        .timelines()
        .iter()
        .map(|timeline| {
            let groups = timeline
                .animations()
                .iter()
                .map(|animation| animation.groups().len())
                .sum();
            (
                timeline.id(),
                timeline.animations().len(),
                timeline.label_sets().len(),
                groups,
            )
        })
        .collect();

    log::info!(
        "assets/uld: {path} {} textures, {} part lists, {} components, {} widgets",
        layout.textures().len(),
        layout.part_lists().len(),
        layout.components().len(),
        layout.widgets().len()
    );

    Ok(Preview::Uld(Box::new(Rendered {
        textures,
        part_lists,
        widgets,
        identity,
        timelines,
        selection: egui::Id::new(("uld selection", path)),
    })))
}

/// Which of a widget's rows the canvas leaves out: the ones the user hid, each spread over its
/// subtree, since hiding a node hides what it holds. Rows are held parents-first, so one pass over
/// them settles it.
fn concealed(widget: &WidgetRow, index: usize, hidden: &HashSet<(usize, usize)>) -> Vec<bool> {
    let mut out = vec![false; widget.nodes.len()];
    for (row, node) in widget.nodes.iter().enumerate() {
        out[row] = hidden.contains(&(index, row)) || node.parent.is_some_and(|parent| out[parent]);
    }
    out
}

/// The composed layout, drawn to scale. Returns the node picked out of it this frame, if the
/// pointer was over one.
fn canvas(
    ui: &mut egui::Ui,
    widget: &WidgetRow,
    concealed: &[bool],
    pinned: Option<usize>,
    deps: &mut Deps,
    backend: &Backend,
) -> Option<Pick> {
    if widget.extent.x < 1.0 || widget.extent.y < 1.0 {
        return None;
    }
    // Shrink to fit, never magnify -- an upscaled layout is just a blurrier one.
    let scale = (ui.available_width() / widget.extent.x).min(1.0);
    let (rect, response) = ui.allocate_exact_size(widget.extent * scale, Sense::click());
    if !ui.is_rect_visible(rect) {
        return None;
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    let place = |dest: &Rect| {
        Rect::from_min_size(rect.min + dest.min.to_vec2() * scale, dest.size() * scale)
    };

    for item in &widget.items {
        // Ahead of the pieces *and* the text below them, so a hidden node keeps neither.
        if concealed[item.row] {
            continue;
        }
        for piece in &item.pieces {
            let Some(path) = &piece.sprite.texture else {
                continue;
            };
            let Dep::Ready(atlas) = deps.atlas(ui.ctx(), backend, path) else {
                continue;
            };
            let s = &piece.sprite;
            let uv = atlas.uv(s.x, s.y, s.width, s.height);
            let target = place(&piece.dest);
            let draw = |at: Rect| {
                egui::Image::new(SizedTexture::new(atlas.texture(), at.size()))
                    .uv(uv)
                    .tint(item.tint)
                    .paint_at(ui, at);
            };

            // A tiling piece repeats at its own size instead of stretching, which is what keeps a
            // window's background texture from smearing across the whole frame.
            let step = vec2(f32::from(s.width) * scale, f32::from(s.height) * scale);
            let tiles = match piece.tile && step.x >= 1.0 && step.y >= 1.0 {
                true => (target.width() / step.x).ceil() * (target.height() / step.y).ceil(),
                false => 1.0,
            };
            if tiles <= 1.0 || tiles > MAX_TILES {
                draw(target);
                continue;
            }

            let clip = ui.painter().clip_rect().intersect(target);
            let mut y = target.top();
            while y < target.bottom() {
                let mut x = target.left();
                while x < target.right() {
                    let cell = Rect::from_min_size(pos2(x, y), step).intersect(clip);
                    if !cell.is_negative() {
                        draw(Rect::from_min_size(pos2(x, y), step));
                    }
                    x += step.x;
                }
                y += step.y;
            }
        }

        // Drawn after this node's sprites and before the next node's, so a label lands over the
        // panel behind it.
        let Some(text) = &widget.nodes[item.row].text else {
            continue;
        };
        let size = text.size * scale;
        if size < LEGIBLE {
            continue;
        }
        let Some(string) = deps
            .text(ui.ctx(), backend, text.sheet, text.row)
            .map(str::to_owned)
        else {
            continue;
        };
        let target = place(&item.bounds);
        let color = text
            .color
            .unwrap_or_else(|| ui.visuals().text_color())
            .gamma_multiply(f32::from(item.tint.a()) / 255.0);
        let galley = painter.layout(
            string,
            egui::FontId::proportional(size),
            color,
            target.width(),
        );
        let at = text.align.align_size_within_rect(galley.size(), target);
        // The game's font is not this one, so a string that no longer fits its node is clipped to it
        // rather than left to run across the layout.
        painter
            .with_clip_rect(target.intersect(rect))
            .galley(at.min, galley, color);
    }

    // The deepest node containing the pointer wins, the way a browser's inspector picks: a window
    // is wrapped in full-size backgrounds and collision regions, and topmost-wins would resolve to
    // one of those wherever it was pointed. Paint order settles ties between siblings.
    let deepest = response.hover_pos().and_then(|pointer| {
        widget
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| !concealed[item.row] && place(&item.bounds).contains(pointer))
            .max_by_key(|(index, item)| (widget.nodes[item.row].depth, *index))
            .map(|(_, item)| item.row)
    });

    let within = |mut row: usize, ancestor: usize| loop {
        if row == ancestor {
            return true;
        }
        match widget.nodes[row].parent {
            Some(parent) => row = parent,
            None => return false,
        }
    };

    // A hit inside an instanced component resolves to the component, so a button is picked as a
    // button rather than as the image drawn inside it. Once something in that component is
    // selected, though, pointing at it goes all the way down again, which is what reaches the
    // nodes a component is built from. The outline follows suit, previewing what a click selects.
    let hovered = deepest.map(|row| {
        let grouped = match widget.nodes[row].instances {
            true => row,
            false => widget.nodes[row].component.unwrap_or(row),
        };
        match pinned.is_some_and(|pinned| within(pinned, grouped)) {
            true => row,
            false => grouped,
        }
    });

    for (row, stroke) in [
        (pinned, ui.visuals().selection.stroke),
        (hovered, ui.visuals().widgets.hovered.fg_stroke),
    ] {
        let Some(bounds) = row
            .filter(|row| !concealed[*row])
            .and_then(|row| widget.items.iter().find(|item| item.row == row))
            .map(|item| place(&item.bounds))
        else {
            continue;
        };
        painter.rect_stroke(bounds, 0.0, stroke, egui::StrokeKind::Inside);
    }

    if let Some(row) = hovered {
        let node = &widget.nodes[row];
        response.clone().on_hover_ui(|ui| {
            ui.label(RichText::new(format!("{} {}", node.kind, node.id)).monospace());
            ui.label(RichText::new(&node.geometry).weak());
        });
    }

    match (response.clicked(), hovered) {
        (true, Some(row)) => Some(Pick::Node(row)),
        (true, None) => Some(Pick::Nothing),
        (false, _) => None,
    }
}

/// One sprite from a part list, cropped out of its atlas.
fn sprite(ui: &mut egui::Ui, part: &Sprite, deps: &mut Deps, backend: &Backend) {
    let scale = (SPRITE / f32::from(part.width.max(part.height).max(1))).min(1.0);
    let size = vec2(
        f32::from(part.width) * scale,
        f32::from(part.height) * scale,
    );

    let (rect, response) = ui.allocate_exact_size(size.max(Vec2::splat(8.0)), Sense::hover());
    // Layout runs for every part in the list while only a fraction is on screen, and asking for a
    // texture is what starts fetching and decoding it. An off-screen sprite keeps its space and
    // fetches nothing.
    if !ui.is_rect_visible(rect) {
        return;
    }

    let Some(path) = &part.texture else {
        ui.painter().rect_stroke(
            rect,
            0.0,
            ui.visuals().noninteractive().bg_stroke,
            egui::StrokeKind::Inside,
        );
        return;
    };

    match deps.atlas(ui.ctx(), backend, path) {
        Dep::Ready(atlas) => {
            egui::Image::new(SizedTexture::new(atlas.texture(), size))
                .uv(atlas.uv(part.x, part.y, part.width, part.height))
                .paint_at(ui, rect);
        }
        Dep::Pending => {
            egui::Spinner::new().paint_at(ui, rect);
        }
        Dep::Failed => {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "⚠",
                egui::FontId::default(),
                Color32::LIGHT_RED,
            );
        }
    }

    response.on_hover_ui(|ui| {
        ui.label(RichText::new(file_name(path)).monospace());
        ui.label(
            RichText::new(format!(
                "{},{}  {}x{}",
                part.x, part.y, part.width, part.height
            ))
            .weak(),
        );
    });
}

pub fn ui(
    ui: &mut egui::Ui,
    layout: &Rendered,
    deps: &mut Deps,
    backend: &Backend,
) -> Option<String> {
    let mut follow = None;
    let mut selected = layout.selected(ui);
    let hidden = layout.toggles(ui).hidden;

    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, widget) in layout.widgets.iter().enumerate() {
                if widget.items.is_empty() {
                    continue;
                }
                section(ui, &format!("Widget {}", widget.id));
                ui.label(RichText::new(&widget.summary).weak());
                ui.add_space(4.0);
                let pinned = selected.filter(|s| s.0 == index).map(|s| s.1);
                let concealed = concealed(widget, index, &hidden);
                match canvas(ui, widget, &concealed, pinned, deps, backend) {
                    Some(Pick::Node(row)) => selected = Some(Selected(index, row)),
                    Some(Pick::Nothing) => selected = None,
                    None => {}
                }
                ui.add_space(8.0);
            }

            if !layout.textures.is_empty() {
                ui.separator();
                section(ui, "Textures");
                for texture in &layout.textures {
                    ui.horizontal(|ui| {
                        match deps.texture(ui.ctx(), backend, &texture.path) {
                            Dep::Ready(handle) => {
                                let size = handle.size_vec2();
                                let scale = THUMBNAIL / size.x.max(size.y).max(1.0);
                                ui.add(
                                    egui::Image::new(SizedTexture::new(handle, size * scale))
                                        .maintain_aspect_ratio(true),
                                );
                            }
                            Dep::Pending => {
                                ui.add_sized(
                                    Vec2::splat(THUMBNAIL),
                                    egui::Spinner::new().size(THUMBNAIL / 2.0),
                                );
                            }
                            Dep::Failed => {
                                ui.add_sized(
                                    Vec2::splat(THUMBNAIL),
                                    egui::Label::new(RichText::new("⚠").color(Color32::LIGHT_RED)),
                                )
                                .on_hover_text("Failed to load");
                            }
                        }
                        ui.vertical(|ui| {
                            ui.label(RichText::new(format!("#{}", texture.id)).strong());
                            if link(ui, file_name(&texture.path), &texture.path) {
                                follow = Some(texture.path.clone());
                            }
                        });
                    });
                }
            }

            if !layout.part_lists.is_empty() {
                ui.add_space(8.0);
                ui.separator();
                section(ui, "Parts");
                for list in &layout.part_lists {
                    ui.label(
                        RichText::new(format!("list {} ({})", list.id, list.parts.len())).weak(),
                    );
                    ui.horizontal_wrapped(|ui| {
                        for part in &list.parts {
                            sprite(ui, part, deps, backend);
                        }
                    });
                    ui.add_space(4.0);
                }
            }
        });

    layout.store(ui, selected);
    follow
}

impl Rendered {
    pub fn has_details(&self) -> bool {
        true
    }

    fn selected(&self, ui: &egui::Ui) -> Option<Selected> {
        ui.data(|data| data.get_temp::<Selected>(self.selection))
    }

    fn toggles_id(&self) -> egui::Id {
        self.selection.with("toggles")
    }

    /// The tree sets these and the canvas reads them, and the two are drawn by different callers.
    fn toggles(&self, ui: &egui::Ui) -> Toggles {
        ui.data(|data| data.get_temp(self.toggles_id()).unwrap_or_default())
    }

    fn store(&self, ui: &egui::Ui, selected: Option<Selected>) {
        ui.data_mut(|data| match selected {
            Some(selected) => {
                data.insert_temp(self.selection, selected);
            }
            None => data.remove::<Selected>(self.selection),
        });
    }
}

/// A tree row's worth of a string: its first line, cut short enough to sit beside a node's name.
fn snippet(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    match line.char_indices().nth(SNIPPET) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_owned(),
    }
}

/// One widget's nodes, as an indented tree of the kind a browser's element panel draws.
fn tree(
    ui: &mut egui::Ui,
    toggles: &mut Toggles,
    index: usize,
    widget: &WidgetRow,
    selected: &mut Option<Selected>,
    deps: &mut Deps,
    backend: &Backend,
) {
    // A selection made on the canvas can sit inside a collapsed component, so its ancestors are
    // forced open rather than leaving the tree pointing at nothing.
    let mut reveal = HashSet::new();
    if let Some(Selected(widget_index, row)) = *selected
        && widget_index == index
    {
        let mut at = widget.nodes[row].parent;
        while let Some(row) = at {
            reveal.insert(row);
            at = widget.nodes[row].parent;
        }
    }

    let concealed = concealed(widget, index, &toggles.hidden);

    // Rows are in depth-first order, so a collapsed node's subtree is every row after it that is
    // deeper than it.
    let mut collapsed_at = None;
    for (row, node) in widget.nodes.iter().enumerate() {
        match collapsed_at {
            Some(depth) if node.depth > depth => continue,
            _ => collapsed_at = None,
        }

        // An instanced component starts collapsed; everything else starts open. The set holds the
        // rows that have been clicked away from that default.
        let default_open = !node.instances;
        let expanded =
            (default_open != toggles.open.contains(&(index, row))) || reveal.contains(&row);
        if !node.children.is_empty() && !expanded {
            collapsed_at = Some(node.depth);
        }

        // The id stays alongside the text, since parent and sibling links name each other by it.
        let named = match node
            .text
            .as_ref()
            .and_then(|text| deps.text(ui.ctx(), backend, text.sheet, text.row))
        {
            Some(text) => format!("{} {} {:?}", node.kind, node.id, snippet(text)),
            None => format!("{} {}", node.kind, node.id),
        };

        ui.horizontal(|ui| {
            // Hiding a node hides its subtree, so a row under a hidden one has nothing of its own
            // left to switch off.
            let inherited = node.parent.is_some_and(|parent| concealed[parent]);
            let eye = match concealed[row] {
                true => RichText::new(EYE).weak(),
                false => RichText::new(EYE),
            };
            let toggle = ui.add_enabled(
                !inherited,
                Button::new(eye).frame(false).min_size(vec2(GUTTER, 0.0)),
            );
            if toggle
                .on_hover_text(match concealed[row] {
                    true => "Show",
                    false => "Hide",
                })
                .clicked()
            {
                match toggles.hidden.contains(&(index, row)) {
                    true => toggles.hidden.remove(&(index, row)),
                    false => toggles.hidden.insert((index, row)),
                };
            }

            ui.add_space(node.depth.min(MAX_INDENT) as f32 * INDENT);
            match node.children.is_empty() {
                true => ui.add_space(TRIANGLE),
                false => {
                    let (_, response) =
                        ui.allocate_exact_size(Vec2::splat(TRIANGLE), Sense::click());
                    let openness = match expanded {
                        true => 1.0,
                        false => 0.0,
                    };
                    paint_default_icon(ui, openness, &response);
                    if response.clicked() {
                        match toggles.open.contains(&(index, row)) {
                            true => toggles.open.remove(&(index, row)),
                            false => toggles.open.insert((index, row)),
                        };
                    }
                }
            }

            let label = RichText::new(&named);
            // A node that is not drawn -- because the file says so, or because the user switched it
            // off -- is still part of the layout, so it is dimmed rather than dropped.
            let label = match node.visible && !concealed[row] {
                true => label,
                false => label.weak(),
            };
            let picked = *selected == Some(Selected(index, row));
            if ui
                .selectable_label(picked, label)
                .on_hover_text(&node.geometry)
                .clicked()
            {
                *selected = Some(Selected(index, row));
            }
        });
    }
}

/// Identity and the timeline table, drawn into the browser's Details panel.
pub fn details_ui(ui: &mut egui::Ui, layout: &Rendered, deps: &mut Deps, backend: &Backend) {
    let mut selected = layout.selected(ui);
    if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        selected = None;
    }
    let mut toggles = layout.toggles(ui);

    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        if !layout.widgets.is_empty() {
            ScrollArea::vertical()
                .id_salt("uld_tree")
                .max_height(TREE_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (index, widget) in layout.widgets.iter().enumerate() {
                        if layout.widgets.len() > 1 {
                            ui.label(RichText::new(format!("Widget {}", widget.id)).weak());
                        }
                        tree(
                            ui,
                            &mut toggles,
                            index,
                            widget,
                            &mut selected,
                            deps,
                            backend,
                        );
                    }
                });
            ui.add_space(4.0);
            ui.separator();
        }

        let node =
            selected.and_then(|Selected(widget, row)| layout.widgets.get(widget)?.nodes.get(row));
        if let Some(node) = node {
            ui.label(RichText::new(format!("{} {}", node.kind, node.id)).strong());
            // Whole, where the tree row only had space for the start of it.
            if let Some(text) = node
                .text
                .as_ref()
                .and_then(|text| deps.text(ui.ctx(), backend, text.sheet, text.row))
            {
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(text).monospace()).wrap());
            }
            ui.add_space(4.0);
            egui::Grid::new("uld_selected")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    for (label, value) in &node.props {
                        ui.label(RichText::new(*label).weak());
                        ui.label(RichText::new(value).monospace());
                        ui.allocate_space(vec2(ui.available_width(), 0.0));
                        ui.end_row();
                    }
                });
            ui.add_space(8.0);
            ui.separator();
        }

        egui::Grid::new("uld_identity")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for (label, value) in &layout.identity {
                    ui.label(RichText::new(*label).weak());
                    ui.label(RichText::new(value).monospace());
                    ui.allocate_space(vec2(ui.available_width(), 0.0));
                    ui.end_row();
                }
            });

        if !layout.timelines.is_empty() {
            ui.add_space(8.0);
            ui.separator();
            ui.label(RichText::new("Timelines").weak());
            ui.add_space(4.0);
            egui::Grid::new("uld_timelines")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    for (id, animations, label_sets, groups) in &layout.timelines {
                        ui.label(RichText::new(id.to_string()).monospace());
                        ui.label(RichText::new(format!("{animations} animations")).weak());
                        ui.label(RichText::new(format!("{groups} keys")).weak());
                        ui.label(
                            RichText::new(match label_sets {
                                0 => String::new(),
                                _ => format!("{label_sets} labels"),
                            })
                            .weak(),
                        );
                        ui.end_row();
                    }
                });
        }

        // Clicking the panel below everything drops the selection, since the canvas is covered by
        // its own root node almost everywhere and rarely offers anywhere to click out.
        let rest = ui.available_size();
        if rest.y > 0.0 && ui.allocate_response(rest, Sense::click()).clicked() {
            selected = None;
        }
    });

    layout.store(ui, selected);
    ui.data_mut(|data| data.insert_temp(layout.toggles_id(), toggles));
}
