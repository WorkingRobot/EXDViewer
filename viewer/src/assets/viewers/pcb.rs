//! Collision `.pcb` files, drawn and tabulated.

use std::cell::{Cell, RefCell};
use std::io::Cursor;

use anyhow::Result;
use egui::{RichText, ScrollArea};
use ironworks::file::{File, pcb};

use super::{Preview, facts, line, placed, section, table};

fn axes(values: [f32; 3]) -> String {
    format!("{:.3}, {:.3}, {:.3}", values[0], values[1], values[2])
}

pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    scene: RefCell<Option<placed::View>>,
    placed: Cell<bool>,
    section: &'static str,
    columns: Vec<(&'static str, usize)>,
    rows: Vec<Vec<String>>,
    instances: Vec<placed::Instance>,
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    match pcb::Collision::read(Cursor::new(bytes.to_vec()))? {
        pcb::Collision::Mesh(mesh) => Ok(Preview::Pcb(Box::new(render_mesh(path, mesh)))),
        pcb::Collision::List(list) => Ok(Preview::Pcb(Box::new(render_list(path, list)))),
    }
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) -> Option<String> {
    let follow = None;
    ui.horizontal(|ui| {
        if ui.selectable_label(!file.placed.get(), "Table").clicked() {
            file.placed.set(false);
        }
        if ui.selectable_label(file.placed.get(), "Scene").clicked() {
            file.placed.set(true);
        }
    });
    ui.add_space(4.0);

    if file.placed.get() {
        let mut held = file.scene.borrow_mut();
        held.get_or_insert_with(|| file.build()).ui(ui);
        return follow;
    }

    section(ui, file.section);
    table(ui, &file.columns, file.rows.len(), |ui, index| {
        let cells = file.rows[index].iter().map(String::as_str);
        ui.label(RichText::new(line(&file.columns, cells)).monospace());
    });

    follow
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| facts(ui, "pcb_identity", &self.identity));
    }

    fn build(&self) -> placed::View {
        placed::View::new(vec![placed::Batch {
            shape: placed::Shape::Wire,
            instances: self.instances.clone(),
        }])
    }
}

fn render_mesh(path: &str, mesh: pcb::Mesh) -> Rendered {
    let root = mesh.root();
    let mut rows = Vec::new();
    let mut instances = Vec::new();
    let mut nodes = 0usize;
    let mut leaves = 0usize;
    let mut vertices = 0usize;
    let mut primitives = 0usize;

    collect_node(
        root,
        &mut Vec::new(),
        0,
        &mut rows,
        &mut instances,
        &mut nodes,
        &mut leaves,
        &mut vertices,
        &mut primitives,
    );

    let identity = vec![
        ("Version", mesh.version().to_string()),
        ("Nodes", nodes.to_string()),
        ("Leaves", leaves.to_string()),
        ("Vertices", vertices.to_string()),
        ("Primitives", primitives.to_string()),
        ("Root min", axes(root.bounds().min())),
        ("Root max", axes(root.bounds().max())),
    ];

    log::info!("assets/pcb: {path} {nodes} nodes");

    Rendered {
        identity,
        scene: RefCell::new(None),
        placed: Cell::new(false),
        section: "Nodes",
        columns: vec![
            ("Depth", 5),
            ("Path", 12),
            ("Vertices", 9),
            ("Primitives", 10),
            ("Children", 8),
            ("Min", 26),
            ("Max", 26),
        ],
        rows,
        instances,
    }
}

fn render_list(path: &str, list: pcb::MeshList) -> Rendered {
    let rows = list
        .entries()
        .iter()
        .map(|entry| {
            vec![
                pcb::MeshList::mesh_file(entry.id()),
                axes(entry.bounds().min()),
                axes(entry.bounds().max()),
            ]
        })
        .collect::<Vec<_>>();
    let instances = list
        .entries()
        .iter()
        .enumerate()
        .map(|(index, entry)| instance(entry.bounds(), index))
        .collect::<Vec<_>>();
    let identity = vec![
        ("Entries", list.entries().len().to_string()),
        ("Min", axes(list.bounds().min())),
        ("Max", axes(list.bounds().max())),
    ];

    log::info!("assets/pcb: {path} {} entries", list.entries().len());

    Rendered {
        identity,
        scene: RefCell::new(None),
        placed: Cell::new(false),
        section: "Meshes",
        columns: vec![("Mesh", 12), ("Min", 26), ("Max", 26)],
        rows,
        instances,
    }
}

fn collect_node(
    node: &pcb::Node,
    path: &mut Vec<usize>,
    depth: usize,
    rows: &mut Vec<Vec<String>>,
    instances: &mut Vec<placed::Instance>,
    nodes: &mut usize,
    leaves: &mut usize,
    vertices: &mut usize,
    primitives: &mut usize,
) {
    let bounds = node.bounds();
    rows.push(vec![
        depth.to_string(),
        if path.is_empty() {
            "root".to_owned()
        } else {
            path.iter().map(usize::to_string).collect::<Vec<_>>().join(".")
        },
        node.vertices().len().to_string(),
        node.primitives().len().to_string(),
        node.children().len().to_string(),
        axes(bounds.min()),
        axes(bounds.max()),
    ]);
    instances.push(instance(bounds, rows.len() - 1));

    *nodes += 1;
    *vertices += node.vertices().len();
    *primitives += node.primitives().len();
    if node.children().is_empty() {
        *leaves += 1;
    }

    for (index, child) in node.children().iter().enumerate() {
        path.push(index);
        collect_node(
            child,
            path,
            depth + 1,
            rows,
            instances,
            nodes,
            leaves,
            vertices,
            primitives,
        );
        path.pop();
    }
}

fn instance(bounds: pcb::BoundingBox, index: usize) -> placed::Instance {
    let min = bounds.min();
    let max = bounds.max();
    placed::Instance {
        center: std::array::from_fn(|axis| (min[axis] + max[axis]) * 0.5),
        scale: std::array::from_fn(|axis| (max[axis] - min[axis]) * 0.5),
        turn: [0.0, 0.0, 0.0, 1.0],
        color: placed::tint(index),
    }
}