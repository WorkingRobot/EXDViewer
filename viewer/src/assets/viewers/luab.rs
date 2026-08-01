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
    /// How much of the chunk came back as source rather than instructions.
    read: (usize, usize),
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
        ("Functions", chunk.function_count().to_string()),
        (
            "Read as source",
            format!(
                "{} of {}",
                read.functions,
                read.functions + read.disassembled
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
        read: (read.functions, read.functions + read.disassembled),
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
        ui.label(RichText::new(format!("{} lines", lines.len())).weak().small());
        if ui.small_button("Copy").clicked() {
            ui.ctx().copy_text(lines.join("\n"));
        }
        if source {
            let (read, held) = file.read;
            ui.label(
                RichText::new(match read == held {
                    true => "compiles, but not guaranteed to be perfect".to_owned(),
                    false => format!(
                        "{} of {held} functions read as source; the rest are commented instructions",
                        read
                    ),
                })
                .weak()
                .small(),
            );
        }
    });
    ui.data_mut(|data| data.insert_temp(slot, source));

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
