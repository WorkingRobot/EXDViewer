//! `.luab` compiled Lua: the game's quest and event scripts, read back as source.

use anyhow::Result;
use egui::{RichText, ScrollArea};
use luadec::Chunk;

use super::shader::code::listing;
use super::{Preview, facts};

/// A chunk, read and ready to draw.
pub struct Rendered {
    identity: Vec<(&'static str, String)>,
    source: Vec<String>,
    assembly: Vec<String>,
    /// Statements the reading recovered, which a chunk compiled from an empty file has none of.
    statements: usize,
    /// Functions left as commented instructions, each under a line saying why.
    commented: usize,
    /// Which reading is on show, kept per file the way the shader viewers keep theirs.
    state: egui::Id,
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Preview> {
    let chunk = Chunk::parse(bytes)?;
    let header = chunk.header();
    let read = luadec::decompile(&chunk);
    let units = luadec::units(&chunk).map_or(1, <[_]>::len);

    let main = chunk.main();
    let mut identity = vec![
        (
            "Version",
            format!("Lua {:X}.{:X}", header.version >> 4, header.version & 0xF),
        ),
        (
            "Layout",
            format!(
                "{}-bit {}-endian, {}",
                u16::from(header.size_size) * 8,
                match header.little_endian {
                    0 => "big",
                    _ => "little",
                },
                match (header.integral, header.size_number) {
                    (0, 8) => "double".to_owned(),
                    (0, size) => format!("{}-bit float", u16::from(size) * 8),
                    (_, size) => format!("{}-bit integer", u16::from(size) * 8),
                },
            ),
        ),
        ("Units", units.to_string()),
        (
            "Functions",
            format!(
                "{} read as source, {} left as bytecode",
                read.functions, read.disassembled
            ),
        ),
        (
            "Debug info",
            match main.lines().is_empty() && main.locals().is_empty() {
                true => "stripped".to_owned(),
                false => "kept".to_owned(),
            },
        ),
    ];
    // Only a chunk that kept its debug info says where it was compiled from.
    if let Some(source) = main.source() {
        identity.push(("Source", String::from_utf8_lossy(source).into_owned()));
    }

    Ok(Preview::Luab(Box::new(Rendered {
        identity,
        source: read.lines,
        assembly: luadec::disassemble(&chunk),
        statements: read.statements,
        commented: read.disassembled,
        state: egui::Id::new(("luab code", path)),
    })))
}

pub fn ui(ui: &mut egui::Ui, file: &Rendered) {
    let slot = file.state.with("source");
    let mut source = ui.data(|data| data.get_temp::<bool>(slot)).unwrap_or(true);
    let lines = match source {
        true => &file.source,
        false => &file.assembly,
    };

    ui.horizontal(|ui| {
        ui.selectable_value(&mut source, true, "Lua");
        ui.selectable_value(&mut source, false, "Bytecode");
        ui.label(
            RichText::new(format!("{} lines", lines.len()))
                .weak()
                .small(),
        );
        if ui.small_button("Copy").clicked() {
            ui.ctx().copy_text(lines.join("\n"));
        }
        if source {
            ui.label(
                RichText::new(match file.commented {
                    0 => "compiles, but not guaranteed to be perfect".to_owned(),
                    1 => "1 function is commented bytecode below".to_owned(),
                    held => format!("{held} functions are commented bytecode below"),
                })
                .weak()
                .small(),
            );
        }
    });
    ui.data_mut(|data| data.insert_temp(slot, source));

    // A chunk compiled from a file holding only comments has nothing to show, which is worth saying
    // rather than leaving the page blank.
    if source && file.statements == 0 {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("Compiled from a file with no statements.").weak());
        });
        return;
    }

    listing(
        ui,
        "luab_code",
        lines,
        0,
        match source {
            true => "Lua",
            false => "Lua bytecode",
        },
    );
}

impl Rendered {
    pub fn details_ui(&self, ui: &mut egui::Ui) {
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            facts(ui, "luab_identity", &self.identity);
        });
    }
}
