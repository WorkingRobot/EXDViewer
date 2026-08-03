use std::cell::LazyCell;

use egui::{
    Frame, Id, Key, Layout, Modal, Modifiers, Popup, PopupCloseBehavior, Rect, RectAlign, RichText,
    StrokeKind, TextEdit,
    containers::scroll_area::ScrollAreaOutput,
    text::{CCursor, CCursorRange},
    text_edit::TextEditOutput,
};
use itertools::EitherOrBoth;

use crate::utils::FuzzyMatcher;

type PatternMatch<'a> = EitherOrBoth<Vec<&'a str>, (u32, Option<u16>)>;
type GoToMatch = EitherOrBoth<String, (u32, Option<u16>)>;

/// How many suggestions a palette offers.
pub const SUGGESTIONS: usize = 10;

const PALETTE_WIDTH: f32 = 460.0;

/// A Ctrl+K modal over one tab's own filter. What the query matches and what picking a row does are
/// the tab's business; this holds the text and the cursor.
pub struct Palette {
    title: &'static str,
    hint: &'static str,
    query: String,
    focused: bool,
    index: usize,
}

impl Palette {
    pub fn new(title: &'static str, hint: &'static str, query: String) -> Self {
        Self {
            title,
            hint,
            query,
            focused: false,
            index: 0,
        }
    }

    /// `suggestions` is handed the query as it now reads and answers with the rows to offer, each
    /// paired with what picking it should yield. `Err` means the palette is still open.
    pub fn draw<T>(
        mut self,
        ctx: &egui::Context,
        mut suggestions: impl FnMut(&str) -> Vec<(T, String)>,
    ) -> Result<Option<T>, Self> {
        let mut ret = None;
        Modal::default_area("palette-modal".into())
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                Frame::window(ui.style()).show(ui, |ui| {
                    ui.heading(self.title);
                    ui.separator();

                    let up_pressed =
                        ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::ArrowUp));
                    let down_pressed =
                        ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::ArrowDown));
                    let enter_pressed =
                        ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
                    let esc_pressed =
                        ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));

                    let output = TextEdit::singleline(&mut self.query)
                        .hint_text(self.hint)
                        .desired_width(PALETTE_WIDTH)
                        .return_key(None)
                        .show(ui);

                    if !self.focused {
                        output.response.request_focus();
                        set_cursor_position(ctx, &output, self.query.chars().count());
                        self.focused = true;
                    }

                    if esc_pressed {
                        ret = Some(None);
                    }

                    let items = suggestions(&self.query);
                    self.index = self.index.min(items.len().saturating_sub(1));
                    if !items.is_empty() {
                        if down_pressed {
                            self.index = (self.index + 1) % items.len();
                        } else if up_pressed {
                            self.index = (self.index + items.len() - 1) % items.len();
                        }
                    }

                    let mut suggestion_clicked = false;
                    Popup::from_response(&output.response)
                        .layout(Layout::top_down_justified(egui::Align::LEFT))
                        .close_behavior(PopupCloseBehavior::IgnoreClicks)
                        .align(RectAlign::BOTTOM_START)
                        .width(output.response.rect.width())
                        .open(true)
                        .show(|ui| {
                            ui.set_min_width(ui.available_width());
                            if items.is_empty() {
                                ui.label(RichText::new("No matches").weak());
                            }
                            for (i, (_, label)) in items.iter().enumerate() {
                                let mut selected = self.index == i;
                                let toggle = ui.toggle_value(&mut selected, label);
                                if toggle.hovered() {
                                    self.index = i;
                                }
                                if toggle.clicked() {
                                    self.index = i;
                                    suggestion_clicked = true;
                                }
                            }
                        });

                    if enter_pressed || suggestion_clicked {
                        ret = Some(items.into_iter().nth(self.index).map(|(item, _)| item));
                    }
                })
            });

        ret.ok_or(self)
    }
}

/// Which arrow presses a list took for itself this frame.
#[derive(Default, Clone, Copy)]
struct Keys {
    up: bool,
    down: bool,
    enter: bool,
}

/// A keyboard cursor over a virtualised list: the row it sits on, and enough of where the list last
/// scrolled to that moving it can bring the new row into view without jumping any further.
#[derive(Default)]
pub struct ListNav {
    cursor: Option<usize>,
    keys: Keys,
    reveal: bool,
    offset: f32,
    viewport: f32,
}

impl ListNav {
    /// Take the arrow keys for this list, which has to happen before any text field is drawn: a
    /// focused `TextEdit` moves its caret on an arrow it never consumes.
    ///
    /// Only while the list is on screen, and only while nothing but its own `filter` box is being
    /// typed into.
    pub fn claim(&mut self, ctx: &egui::Context, shown: bool, filter: Option<Id>) {
        self.keys = Keys::default();
        let focus = ctx.memory(|memory| memory.focused());
        if !shown || !(focus.is_none() || focus == filter) {
            return;
        }
        ctx.input_mut(|input| {
            self.keys = Keys {
                up: input.consume_key(Modifiers::NONE, Key::ArrowUp),
                down: input.consume_key(Modifiers::NONE, Key::ArrowDown),
                enter: self.cursor.is_some() && input.consume_key(Modifiers::NONE, Key::Enter),
            };
        });
    }

    /// Move the cursor over a list of `len` rows, and report the row Enter picked.
    pub fn apply(&mut self, len: usize) -> Option<usize> {
        if len == 0 {
            self.cursor = None;
            return None;
        }
        let at = self.cursor.map(|at| at.min(len - 1));
        self.cursor = match (at, self.keys.down, self.keys.up) {
            (_, false, false) => at,
            (None, true, _) => Some(0),
            (None, _, true) => Some(len - 1),
            (Some(at), true, _) => Some((at + 1) % len),
            (Some(at), _, true) => Some((at + len - 1) % len),
        };
        self.reveal |= self.keys.up || self.keys.down;
        self.keys.enter.then_some(self.cursor).flatten()
    }

    /// What the list has to scroll to for the cursor to be on screen, or `None` if it already is.
    pub fn scroll(&mut self, ui: &egui::Ui, row_height: f32, len: usize) -> Option<f32> {
        if !std::mem::take(&mut self.reveal) {
            return None;
        }
        let at = self.cursor?;
        let spacing = ui.spacing().item_spacing.y;
        let pitch = row_height + spacing;
        let viewport = if self.viewport > 0.0 {
            self.viewport
        } else {
            ui.available_height()
        };
        let last = (len as f32 * pitch - spacing - viewport).max(0.0);
        // A row of slack below: the rows are drawn a little taller than the height the list reserves
        // for them, so one that ends exactly at the fold by this arithmetic is really past it.
        let offset = self
            .offset
            .min(at as f32 * pitch)
            .max((at + 2) as f32 * pitch - viewport)
            .clamp(0.0, last);
        (offset != self.offset).then_some(offset)
    }

    /// Record where the list ended up, so the next move scrolls as little as it can.
    pub fn seen<R>(&mut self, output: &ScrollAreaOutput<R>) {
        self.offset = output.state.offset.y;
        self.viewport = output.inner_rect.height();
    }

    /// Outline the row the keyboard is on.
    pub fn mark(&self, ui: &egui::Ui, index: usize, rect: Rect) {
        if self.cursor == Some(index) {
            ui.painter()
                .rect_stroke(rect, 2.0, ui.visuals().selection.stroke, StrokeKind::Inside);
        }
    }
}

fn set_cursor_position(ctx: &egui::Context, output: &TextEditOutput, position: usize) {
    let mut state = output.state.clone();
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(position))));
    state.store(ctx, output.response.id);
}

#[derive(Default)]
pub struct GoToWindow {
    requested_focused: bool,
    hint: String,
    string_buffer: String,
    selected_index: Option<usize>,
}

impl GoToWindow {
    pub fn to_sheet() -> Self {
        Self {
            hint: "Sheet:Row.Subrow".to_string(),
            ..Default::default()
        }
    }

    pub fn to_row() -> Self {
        Self {
            hint: "Row.Subrow".to_string(),
            ..Default::default()
        }
    }

    pub fn draw(
        mut self,
        ctx: &egui::Context,
        sheet_matcher: &FuzzyMatcher,
        sheet_list: &[&str],
    ) -> Result<Option<GoToMatch>, Self> {
        let mut ret = None;
        Modal::default_area("goto-modal".into())
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                Frame::window(ui.style()).show(ui, |ui| {
                    ui.heading("Go To…");
                    ui.separator();

                    // Thank you to https://github.com/JakeHandsome/egui_autocomplete/blob/master/src/lib.rs
                    // for a lot of the reference material.

                    let up_pressed =
                        ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::ArrowUp));
                    let down_pressed =
                        ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::ArrowDown));
                    let tab_pressed =
                        ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Tab));
                    let enter_pressed =
                        ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
                    let esc_pressed =
                        ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));

                    let output = TextEdit::singleline(&mut self.string_buffer)
                        .hint_text(&self.hint)
                        .return_key(None)
                        .lock_focus(true)
                        .show(ui);

                    self.string_buffer = self.string_buffer.replace('\t', "");

                    // not sure the best way to say "we want to focus when we open!"
                    if !self.requested_focused {
                        output.response.request_focus();
                        self.requested_focused = true;
                    }

                    if esc_pressed {
                        ret = Some(None);
                    }

                    const MAX_SUGGESTIONS: usize = 10;

                    let match_string = self.string_buffer.clone();
                    let match_results = LazyCell::new(|| {
                        Self::match_string(&match_string, sheet_matcher, sheet_list)
                    });
                    let match_sheets = LazyCell::new(|| {
                        if let Ok(EitherOrBoth::Left(sheets) | EitherOrBoth::Both(sheets, _)) =
                            &*match_results
                        {
                            Some(sheets)
                        } else {
                            None
                        }
                    });
                    let match_location = LazyCell::new(|| {
                        if let Ok(EitherOrBoth::Right(loc) | EitherOrBoth::Both(_, loc)) =
                            &*match_results
                        {
                            Some(loc)
                        } else {
                            None
                        }
                    });

                    let match_sheets_len = match_sheets
                        .as_ref()
                        .map_or(0, |s| s.len())
                        .min(MAX_SUGGESTIONS);
                    self.selected_index = match self.selected_index {
                        Some(_) if match_sheets_len == 0 => None,
                        // Handle down arrow
                        Some(index) if down_pressed => {
                            if index + 1 < match_sheets_len {
                                Some(index + 1)
                            } else {
                                Some(0)
                            }
                        }
                        // Handle up arrow
                        Some(index) if up_pressed => {
                            if index > 0 {
                                Some(index.saturating_sub(1))
                            } else {
                                Some(match_sheets_len - 1)
                            }
                        }
                        // Handle down from no selection to first item
                        None if down_pressed && match_sheets.is_some_and(|s| !s.is_empty()) => {
                            Some(0)
                        }
                        // Handle up from no selection to last item
                        None if up_pressed => match_sheets_len.checked_sub(1),
                        // Clamp out-of-bounds index
                        Some(index) if match_sheets.is_some_and(|s| s.len() <= index) => {
                            Some(match_sheets_len - 1)
                        }
                        // Default to first item if we have a selection but no index
                        None if match_sheets.is_some_and(|s| !s.is_empty()) => Some(0),
                        // Default case
                        other => other,
                    };

                    let popup = Popup::from_response(&output.response)
                        .layout(Layout::top_down_justified(egui::Align::LEFT))
                        .close_behavior(PopupCloseBehavior::IgnoreClicks)
                        .align(RectAlign::BOTTOM_START)
                        .width(output.response.rect.width())
                        .open(true);

                    let mut suggestion_clicked = false;
                    popup.show(|ui| {
                        ui.set_min_width(ui.available_width());

                        if let Some((row_id, subrow_id)) = match_location.as_ref() {
                            ui.label(
                                RichText::new(format!(
                                    "Row {row_id}{}",
                                    if let Some(subrow_id) = subrow_id {
                                        format!(", Subrow {subrow_id}")
                                    } else {
                                        String::new()
                                    }
                                ))
                                .strong(),
                            );
                        }

                        if let Some(sheets) = match_sheets.as_ref() {
                            if sheets.is_empty() {
                                ui.label(RichText::new("No matching sheets").weak());
                            } else {
                                for (i, sheet_name) in
                                    sheets.iter().take(MAX_SUGGESTIONS).enumerate()
                                {
                                    let mut selected = if let Some(x) = self.selected_index {
                                        x == i
                                    } else {
                                        false
                                    };

                                    let toggle = ui.toggle_value(&mut selected, *sheet_name);
                                    if toggle.hovered() {
                                        self.selected_index = Some(i);
                                    }
                                    if toggle.clicked() {
                                        suggestion_clicked = true;
                                        self.set_sheet_name(sheet_name, ctx, &output);
                                    }
                                }
                            }
                        }
                        if let Err(err) = match_results.as_ref() {
                            ui.label(err.to_string());
                        }
                    });

                    if tab_pressed
                        && let Some(sheets) = match_sheets.as_ref()
                        && !sheets.is_empty()
                    {
                        let sheet_name = sheets.get(self.selected_index.unwrap_or_default());
                        if let Some(sheet_name) = sheet_name {
                            self.set_sheet_name(sheet_name, ctx, &output);
                        }
                    } else if tab_pressed || enter_pressed || suggestion_clicked {
                        let index = self.selected_index.unwrap_or_default();
                        let r = match_results
                            .as_ref()
                            .map(|r| r.as_ref().map_left(|s| s.get(index).copied()))
                            .ok();
                        ret = Some(match r {
                            None | Some(EitherOrBoth::Left(None)) => None,
                            Some(EitherOrBoth::Left(Some(s))) => {
                                Some(EitherOrBoth::Left(s.to_string()))
                            }
                            Some(EitherOrBoth::Right(loc) | EitherOrBoth::Both(None, loc)) => {
                                Some(EitherOrBoth::Right(*loc))
                            }
                            Some(EitherOrBoth::Both(Some(s), loc)) => {
                                Some(EitherOrBoth::Both(s.to_string(), *loc))
                            }
                        });
                    }
                })
            });

        ret.ok_or(self)
    }

    fn set_sheet_name(&mut self, sheet_name: &str, ctx: &egui::Context, output: &TextEditOutput) {
        self.string_buffer = self
            .string_buffer
            .split_once(':')
            .map(|(_, row_part)| row_part)
            .map_or_else(
                || sheet_name.to_string(),
                |row_part| format!("{sheet_name}:{row_part}"),
            );
        self.selected_index = None;
        set_cursor_position(ctx, output, sheet_name.len());
    }

    /// Parses a string that may represent either a autocompleted sheet list or a row/subrow.
    /// Returns `Left` for a sheet list, and/or `Right` for a row/subrow tuple.
    /// Errors with a human readable string if the input is invalid.
    fn match_string<'a>(
        pattern: &str,
        sheet_matcher: &FuzzyMatcher,
        sheet_list: &'a [&'a str],
    ) -> anyhow::Result<PatternMatch<'a>> {
        if let Some((sheet_pattern, row_pattern)) = pattern.split_once(':') {
            if !sheet_pattern.is_empty() {
                let sheets = Self::match_sheet(sheet_pattern, sheet_matcher, sheet_list);
                let location = Self::match_location(row_pattern)
                    .ok_or_else(|| anyhow::anyhow!("Invalid row"))?;
                Ok(EitherOrBoth::Both(sheets, location))
            } else {
                let location = Self::match_location(row_pattern)
                    .ok_or_else(|| anyhow::anyhow!("Invalid row"))?;
                Ok(EitherOrBoth::Right(location))
            }
        } else {
            let location = Self::match_location(pattern);
            if let Some(location) = location {
                Ok(EitherOrBoth::Right(location))
            } else {
                let result = Self::match_sheet(pattern, sheet_matcher, sheet_list);
                Ok(EitherOrBoth::Left(result))
            }
        }
    }

    fn match_sheet<'a>(
        pattern: &str,
        sheet_matcher: &FuzzyMatcher,
        sheet_list: &'a [&'a str],
    ) -> Vec<&'a str> {
        sheet_matcher.match_list(Some(pattern), sheet_list)
    }

    fn match_location(string_buffer: &str) -> Option<(u32, Option<u16>)> {
        if string_buffer.contains('.') {
            // subrow case
            let (row_id_text, subrow_id_text) = string_buffer.split_once('.')?;

            Some((row_id_text.parse().ok()?, subrow_id_text.parse().ok()))
        } else {
            // normal row case
            Some((string_buffer.parse().ok()?, None))
        }
    }
}

#[cfg(test)]
mod test {
    use crate::goto::GoToWindow;

    #[test]
    fn match_location() {
        // Empty
        assert_eq!(GoToWindow::match_location(""), None);

        // Row
        assert_eq!(GoToWindow::match_location("5"), Some((5, None)));

        // Invalid Row
        assert_eq!(GoToWindow::match_location("5a"), None);

        // Subrow
        assert_eq!(GoToWindow::match_location("5.6"), Some((5, Some(6))));

        // Invalid Subrow
        assert_eq!(GoToWindow::match_location("5.a"), Some((5, None)));
    }
}
