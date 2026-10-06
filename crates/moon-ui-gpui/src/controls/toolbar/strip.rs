//! Captioned strips: the preset-group captions and their text.

use super::*;

/// Caption for a preset group, muted, sharing the control-tier body size with the chips beside it.
///
/// `MoonText` with [`design::text_metrics`] rather than a hand-rolled text div: it applies
/// the theme's mono family and already-scaled metrics, so the caption lands on the same size and
/// baseline as the neighbouring control-tier controls. Colour is the remaining caption cue.
///
/// The text is a literal, not `t!`: `Size`/`Sell` are on the deliberately-untranslated list
/// (`locales/README.md`), as are the neighbouring `Lev`/`SL`/`TP`. The tooltip is translated, the
/// caption is not. The appended `USDT eq.` is a deliberately-untranslated technical unit.
pub(super) fn strip_caption(
    text: impl Into<SharedString>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    strip_text(text, p.text_muted, cx)
}

/// The row's shared text recipe, used by every label and readout on it.
///
/// One home for the MoonUI call so a second text cell cannot drift to a different family, size or
/// casing while looking the same in the source. `color` is the only thing that legitimately varies:
/// a caption NAMES something and is muted, while a readout STATES something and is not.
///
/// Args:
///     text: Caption or readout text rendered at the toolbar's shared text scale.
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

/// A preset strip with its caption in front, as one flex group. `caption = None` collapses it.
///
/// Both preset groups are laid out identically and only differ in caption text and the strip
/// itself; keeping the grouping in one place is what stops a gap or ordering tweak from being
/// applied to Size and forgotten on Sell, a drift that only shows up once a window is narrow enough
/// to render the two differently.
///
/// `tip`, when present, attaches a tooltip to the whole group WITHOUT affecting its measured
/// width — the freshness/rejection marker for a core-sourced manual block goes here rather than
/// into the caption text itself, which `row_fit` has already measured and budgeted by the time
/// this runs. `id` gives the group the stable element identity a tooltip requires.
pub(super) fn captioned_strip(
    id: &'static str,
    caption: Option<SharedString>,
    p: MoonPalette,
    strip: impl IntoElement,
    tip: Option<SharedString>,
    caption_gap: f32,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .id(id)
        .flex_none()
        .gap(design::ui_px(cx, caption_gap))
        .children(caption.map(|text| strip_caption(text, p, cx)))
        .child(strip)
        .when_some(tip, |el, tip| el.tooltip(text_tooltip(tip)))
}
