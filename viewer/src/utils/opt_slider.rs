use std::{num::NonZero, ops::RangeInclusive};

pub fn opt_slider(
    ui: &mut egui::Ui,
    value: Option<NonZero<u32>>,
    range: RangeInclusive<u32>,
    text: &str,
    none_text: &str,
    suffix: &str,
) -> egui::InnerResponse<Option<NonZero<u32>>> {
    let mut value = value;
    let fake_end = *range.end() + 1;
    let r = ui.add(
        egui::Slider::from_get_set(f64::from(*range.start())..=f64::from(fake_end), |val| {
            if let Some(val) = val
                && let Some(val) = NonZero::new(val.round() as u32)
            {
                value = (val.get() != fake_end).then_some(val);
                val.get().into()
            } else {
                value.map_or(fake_end, |w| w.get()).into()
            }
        })
        .integer()
        .text(text)
        .custom_formatter(|f, _| {
            if f > f64::from(*range.end()) {
                none_text.to_owned()
            } else {
                format!("{}{suffix}", f.round())
            }
        }),
    );
    egui::InnerResponse::new(value, r)
}
