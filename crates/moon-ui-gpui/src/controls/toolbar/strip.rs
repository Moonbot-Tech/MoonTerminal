//! The row's text recipes: pod captions and read-outs.

use super::*;

/// Gap between a pod caption's word and its unit, before the UI scale.
const POD_UNIT_GAP: f32 = 3.0;
/// Weight of a pod caption's word.
const POD_WORD_WEIGHT: f32 = 500.0;

/// A pod's caption: its word in the UI face, emphasized, and an optional literal unit after it,
/// muted (`Size USDT`). Measured by [`pod_label_width`].
pub(super) fn pod_label(
    word: impl Into<SharedString>,
    unit: Option<&'static str>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, POD_UNIT_GAP))
        .child(
            design::pod_caption(word, p, cx)
                .text_color(rgb(p.text_soft))
                .font_weight(FontWeight(POD_WORD_WEIGHT)),
        )
        .children(unit.map(|unit| design::pod_caption(unit, p, cx)))
}

/// Rendered width of [`pod_label`] for `word` and `unit`, each text run at its whole-pixel box.
pub(super) fn pod_label_width(cx: &App, word: &str, unit: Option<&str>) -> f32 {
    use crate::chrome::terminal_chrome::text_box;
    text_box(design::ui_caption_text_width(cx, word, POD_WORD_WEIGHT))
        + unit.map_or(0.0, |unit| {
            design::ui_value(cx, POD_UNIT_GAP)
                + text_box(design::ui_caption_text_width(cx, unit, 400.0))
        })
}

/// The row's shared read-out recipe: a mono figure at the control-tier body size.
///
/// One home for the MoonUI call so a second read-out cannot drift to a different family, size or
/// casing while looking the same in the source. `color` is the only thing that legitimately varies.
///
/// Args:
///     text: Read-out text rendered at the toolbar's shared text scale.
///     color: Active palette color for the text's semantic role.
///     cx: Application context used to resolve control-tier text metrics.
///
/// Returns:
///     A non-shrinking monospaced toolbar text element.
pub(super) fn strip_text(text: impl Into<SharedString>, color: u32, cx: &App) -> impl IntoElement {
    div().flex_none().child(
        MoonText::new(text)
            .mono(true)
            .color(color)
            .uppercase(false)
            .rendered_metrics(design::text_metrics(cx, 0.0, 11.0))
            .render(),
    )
}
