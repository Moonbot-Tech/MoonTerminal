//! The inner switch shared by the Telegram tab and the Interface and General groups.
//!
//! The control is the same one the Telegram tab already draws: a blue `MoonSegmentedControl`
//! whose cells fit between 110 and the width left in the Settings viewport.

use gpui::{App, ClickEvent, Window};
use moon_ui::{MoonAccent, MoonSegmentItem, MoonSegmentedControl};

use crate::design;

/// Minimum cell width, in unscaled pixels, matching the Telegram tab's segments.
const SEGMENT_MIN_W: f32 = 110.0;
/// Largest cell width the Telegram tab allows, in font-scaled pixels.
const SEGMENT_MAX_W: f32 = 220.0;

/// Build the Settings inner switch.
///
/// Args:
///     id: Stable element id. The Telegram tab keeps `telegram-segments`.
///     labels: Localized captions, in switch order. Each cell's tooltip is that same caption.
///     selected: Index of the selected caption. An index past the end selects nothing.
///     width: Viewport width used to cap each cell the way the Telegram tab does.
///     on_select: Called with the clicked index.
///     cx: Application context used to fit the cells.
///
/// Returns:
///     The segmented control.
pub(in crate::settings) fn settings_segment(
    id: &'static str,
    labels: &[String],
    selected: usize,
    width: f32,
    on_select: impl Fn(usize, &ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl gpui::IntoElement {
    let cell_max =
        ((width - design::ui_value(cx, 36.0)) / design::font_w(cx, 2.0)).min(SEGMENT_MAX_W);
    let items = labels.iter().enumerate().map(|(index, label)| {
        MoonSegmentItem::new("", label.clone())
            .fit_width(cx, SEGMENT_MIN_W, cell_max)
            .tooltip(label.clone())
            .selected(index == selected)
    });
    MoonSegmentedControl::new(id)
        .accent(MoonAccent::Blue)
        .items(items)
        .on_click(on_select)
        .render()
}
