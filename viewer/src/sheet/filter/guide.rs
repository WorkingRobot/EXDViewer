use egui::{
    Align, Color32, FontId, Label, Layout, Margin, RichText, TextStyle, TextWrapMode, Vec2, Window,
};

use crate::settings::FILTER_GUIDE_VISIBLE;

const SECTIONS: &[(&str, &[[&str; 3]])] = &[
    (
        "Example",
        &[
            ["Name = potion", "column, comparator, value", ""],
            ["Name", "column: which column to read", ""],
            ["=", "comparator: how to compare it", ""],
            ["potion", "value: what to compare against", ""],
        ],
    ),
    (
        "Comparators",
        &[
            ["=", "equals", "Name = potion"],
            [
                "^=, $=, *=",
                "starts with, ends with, contains",
                "Name *= potion",
            ],
            ["~=", "fuzzy, sorts rows by score", "Name ~= ptn"],
            ["?=", "wildcard", "Name ?= \"*potion?\""],
            ["/=", "regex", "Name /= /^potion/i"],
            ["|=", "in range", "Icon |= 100..200"],
            [">, >=, <, <=", "numeric compare", "Icon > 0"],
            ["!=, not ^=", "negate any of them", "Name != potion"],
            [
                "(cmp)=",
                "append a = to force all columns to match",
                "Item[*] $== potion",
            ],
        ],
    ),
    (
        "Values",
        &[
            ["potion", "letters, digits, _ - . /", "Name = potion"],
            [
                "\"a potion\", 'a potion'",
                "quote anything else",
                "Name = \"a potion\"",
            ],
            [
                "\\\" \\\\ \\n \\r \\t",
                "escapes inside quotes",
                "Name = \"say \\\"hi\\\"\"",
            ],
            ["-12", "integer, no leading zeros", "Icon = -12"],
            ["10..20, 10.., ..20", "inclusive ranges", "Icon |= 10.."],
            [
                "/pattern/flags",
                "regex; flags: i m s U x R u",
                "Name /= /^a.*b$/i",
            ],
        ],
    ),
    (
        "Columns",
        &[
            ["Name", "a column by name", "Name = potion"],
            ["Text[3]", "one array element", "Text[3] *= hi"],
            [
                "Item[0].Name",
                "a field inside an array",
                "Item[0].Name = a",
            ],
            ["*", "wildcard (any characters)", "Text[*] = potion"],
            ["?", "wildcard (one character)", "Text? = potion"],
            ["*", "any column", "* = potion"],
            ["#", "the row id", "# = 42"],
        ],
    ),
    (
        "Any or all columns",
        &[
            ["=", "any of the columns matched", "Text* = potion"],
            ["==", "all of the columns must match", "Text* == potion"],
        ],
    ),
    (
        "Combining",
        &[
            ["and, &&", "both sides match", "Name = a and Icon > 0"],
            ["or, ||", "either side matches", "Name = a || Name = b"],
            ["not, !", "flips whatever follows", "not Name = a"],
            ["( )", "groups terms together", "(a or b) and c"],
            [
                "a or b and c",
                "not first, then and, then or",
                "a or (b and c)",
            ],
        ],
    ),
];

const NOTES: &[&str] = &[
    "~= reorders rows by score, so row ids stop being in order.",
    "Subrow ids are text: \"12.3\", so # > 5 matches nothing.",
    "Regexes cannot use lookaround or backreferences.",
    "An unfinished filter turns the box red and keeps the last results.",
];

pub fn draw(ctx: &egui::Context) {
    let visible = FILTER_GUIDE_VISIBLE.get(ctx);
    let mut open = visible;
    Window::new("Filter Guide")
        .open(&mut open)
        .default_height(620.0)
        .vscroll(true)
        .show(ctx, draw_contents);
    if open != visible {
        FILTER_GUIDE_VISIBLE.set(ctx, open);
    }
}

fn draw_contents(ui: &mut egui::Ui) {
    let widths = column_widths(ui);
    let width = widths.iter().sum::<f32>() + ui.spacing().item_spacing.x * 2.0;
    ui.set_min_width(width);

    for (title, rows) in SECTIONS {
        header(ui, title, width);
        for row in *rows {
            ui.horizontal(|ui| {
                for (column, text) in row.iter().enumerate() {
                    cell(ui, text, column, widths[column]);
                }
            });
        }
    }

    header(ui, "Notes", width);
    for note in NOTES {
        ui.label(*note);
    }
}

fn header(ui: &mut egui::Ui, title: &str, width: f32) {
    ui.add_space(8.0);
    egui::Frame::NONE
        .fill(ui.visuals().selection.bg_fill.gamma_multiply(0.4))
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.set_min_width(width - 12.0);
            ui.label(RichText::new(title).strong());
        });
    ui.add_space(2.0);
}

fn cell(ui: &mut egui::Ui, text: &str, column: usize, width: f32) {
    ui.allocate_ui_with_layout(
        Vec2::new(width, 0.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_width(width);
            if !text.is_empty() {
                let text = RichText::new(text).font(font(ui, text, column));
                let text = if column == 0 { text.strong() } else { text };
                ui.add(Label::new(text).wrap_mode(TextWrapMode::Extend));
            }
        },
    );
}

fn font(ui: &egui::Ui, text: &str, column: usize) -> FontId {
    if column == 1 || !text.is_ascii() {
        TextStyle::Body.resolve(ui.style())
    } else {
        TextStyle::Monospace.resolve(ui.style())
    }
}

fn column_widths(ui: &egui::Ui) -> [f32; 3] {
    let mut widths = [0.0f32; 3];
    for (_, rows) in SECTIONS {
        for row in *rows {
            for (column, text) in row.iter().enumerate() {
                if text.is_empty() {
                    continue;
                }
                let font = font(ui, text, column);
                let galley = ui.fonts_mut(|fonts| {
                    fonts.layout_no_wrap((*text).to_owned(), font, Color32::PLACEHOLDER)
                });
                widths[column] = widths[column].max(galley.size().x);
            }
        }
    }
    widths
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders() {
        let ctx = egui::Context::default();
        crate::settings::FILTER_GUIDE_VISIBLE.set(&ctx, true);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| super::draw(ui.ctx()));
        assert!(!output.shapes.is_empty());
    }
}
