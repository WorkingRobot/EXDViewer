use std::cell::LazyCell;

use egui::{
    Frame, Key, Layout, Modal, Modifiers, Popup, PopupCloseBehavior, RectAlign, RichText, TextEdit,
    text::{CCursor, CCursorRange},
    text_edit::TextEditOutput,
};
use itertools::EitherOrBoth;

use crate::utils::FuzzyMatcher;

type PatternMatch<'a> = EitherOrBoth<Vec<&'a str>, (u32, Option<u16>)>;
type GoToMatch = EitherOrBoth<String, (u32, Option<u16>)>;

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
                                        "".to_string()
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
                            .map(|r| r.as_ref().map_left(|s| s.get(index).cloned()))
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
            .split_once(":")
            .map(|(_, row_part)| row_part)
            .map(|row_part| format!("{}:{}", sheet_name, row_part))
            .unwrap_or_else(|| sheet_name.to_string());
        self.selected_index = None;
        Self::set_cursor_position(ctx, output, sheet_name.len());
    }

    fn set_cursor_position(ctx: &egui::Context, output: &TextEditOutput, position: usize) {
        let mut state = output.state.clone();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(position))));
        state.store(ctx, output.response.id);
    }

    /// Parses a string that may represent either a autocompleted sheet list or a row/subrow.
    /// Returns `Left` for a sheet list, and/or `Right` for a row/subrow tuple.
    /// Errors with a human readable string if the input is invalid.
    fn match_string<'a>(
        pattern: &str,
        sheet_matcher: &FuzzyMatcher,
        sheet_list: &'a [&'a str],
    ) -> anyhow::Result<PatternMatch<'a>> {
        if let Some((sheet_pattern, row_pattern)) = pattern.split_once(":") {
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
        if string_buffer.contains(".") {
            // subrow case
            let (row_id_text, subrow_id_text) = string_buffer.split_once(".")?;

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
