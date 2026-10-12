//! Row fitting: the measured widths the collapse level is picked from, and the pure decisions the
//! level drives.

use super::*;

/// The leverage reason that replaces the Leverage pod with the no-coin hint.
const NO_MARKET_REASON: &str = "toolbar.lev_unset_no_market";

/// Widest the no-coin hint is counted at from L2, before the font scale. It ellipsizes past this
/// and its tooltip carries the whole sentence, so a long locale cannot push a level that 760 px
/// holds. Below L2 — where the MAX readout it stands in for would still be drawn — it is counted
/// whole.
const NO_COIN_HINT_MAX_W: f32 = 170.0;

/// Weight of the Live chip's label.
const LIVE_WEIGHT: f32 = 500.0;
/// Size of the Live chip's dot glyph, before the UI scale.
const LIVE_DOT_SIZE: f32 = 9.0;
/// Weight of the Live chip's dot glyph.
const LIVE_DOT_WEIGHT: f32 = 700.0;
/// The Live chip's dot.
pub(super) const LIVE_DOT: &str = "●";

/// Whether the leverage reason selects the no-coin hint in place of the Leverage pod.
///
/// Only a missing market does: with no coin on the chart there is neither a leverage nor an order
/// limit to state, and one sentence saying so beats two dashes. A missing core or an unreported
/// value keeps the pod and its explaining tooltip.
pub(crate) fn shows_no_coin_hint(lev_unset_key: Option<&str>) -> bool {
    lev_unset_key == Some(NO_MARKET_REASON)
}

/// Width the Strategy pod's controls may spend at `level`.
///
/// The pod is counted at its FULL inner ladder at every level; only once the toolbar is at its
/// narrowest level and still short does the pod's own ladder shed names and slots, by exactly the
/// shortfall. Everywhere else the budget is unbounded.
pub(crate) fn strat_budget(widths: &ToolbarWidths, level: ToolbarLevel, available: f32) -> f32 {
    let short = widths.width_at(level) - available;
    if level.0 == ToolbarLevel::MAX && short > 0.0 {
        (widths.strat_controls - short).max(0.0)
    } else {
        f32::INFINITY
    }
}

/// Whether the trailing cluster pins itself over the row's right edge: the row at its level still
/// outgrows the window, so the end of the flow — where "More" and Live stand — is past it.
pub(crate) fn trailing_pinned(widths: &ToolbarWidths, level: ToolbarLevel, available: f32) -> bool {
    widths.width_at(level) > available
}

/// Rendered width of the Live chip carrying `label`: the dot and its segment gap in front of a
/// labelled chrome button at the launcher insets.
pub(super) fn live_width(cx: &App, label: &str) -> f32 {
    design::ui_text_width_zoomed(cx, LIVE_DOT, LIVE_DOT_SIZE, LIVE_DOT_WEIGHT, false)
        + design::ui_value(cx, design::CONTROL_TIER.control_metrics().gap)
        + crate::controls::launch::labeled_button_width(cx, label, LIVE_WEIGHT, false)
}

/// The no-coin hint's whole sentence as laid out: its measured text, rounded up to the whole pixel
/// GPUI boxes a text run at.
pub(super) fn no_coin_hint_full_width(cx: &App, text: &str) -> f32 {
    design::ui_caption_text_width(cx, text, 400.0).ceil()
}

/// Width the no-coin hint is drawn at: whole below L2, capped from L2 on.
pub(super) fn no_coin_hint_width(cx: &App, text: &str, level: ToolbarLevel) -> f32 {
    let full = no_coin_hint_full_width(cx, text);
    if level.max_readout() {
        full
    } else {
        full.min(design::font_w(cx, NO_COIN_HINT_MAX_W))
    }
}

/// Width the order pods (Strategy .. Sell) occupy at `level` in a row `available` wide: the row
/// less its insets, the trailing cluster and the two gaps around the spacer. The lock overlay
/// covers exactly this span.
pub(crate) fn order_pods_width(
    widths: &ToolbarWidths,
    level: ToolbarLevel,
    available: f32,
    cx: &App,
) -> f32 {
    let gap = design::ui_value(cx, design::CHROME_GAP);
    let pad = design::ui_value(cx, design::TOOLBAR_PAD_X);
    (widths.width_at(level).min(available) - widths.trailing_at(level) - pad * 2.0 - gap * 2.0)
        .max(0.0)
}

/// Measure every toolbar element group, in rendered pixels, with the arithmetic the row renders by.
///
/// The order pods (and the no-coin hint) stand side by side in one span, then the flexible spacer
/// and the trailing cluster; every pod's width carries one gap, so a pod folding into "More" takes
/// its gap with it. A pod costs its chrome; its caption, counted in `captions`, costs the caption
/// and the pod gap after it.
///
/// Args:
///     model: The resolved toolbar state.
///     cx: Application context supplying theme-aware scale and text measurements.
///
/// Returns:
///     The widths [`ToolbarWidths::level`] picks the collapse level from.
pub(crate) fn toolbar_widths(model: &ToolbarModel, cx: &App) -> ToolbarWidths {
    let gap = design::ui_value(cx, design::CHROME_GAP);
    let pod_gap = design::ui_value(cx, design::POD_GAP);
    let pod = design::pod_chrome_width(cx) + gap;
    let caption = |word: &str, unit: Option<&str>| pod_label_width(cx, word, unit) + pod_gap;

    // The insets, and the gap between the spacer and the trailing cluster.
    let mut fixed = design::ui_value(cx, design::TOOLBAR_PAD_X) * 2.0 + gap;
    let mut captions = 0.0;
    let mut strat_controls = 0.0;
    let mut ms_summary = 0.0;
    if let Some(strat) = model.strat.as_ref() {
        fixed += pod;
        captions += caption(&model.captions.strategy, None);
        strat_controls = strat.controls_width(cx);
        ms_summary = strat.summary_width();
    }

    // Size: the strip or its collapsed form, and the own-set icon while it is on the row.
    captions += caption(&model.captions.size, Some(SIZE_POD_UNIT));
    let size = strips::strip_geometry(&model.size_cells, cx);
    let own_icon = if model.display_core.is_some() {
        pod_gap + design::glyph_btn_w(cx)
    } else {
        0.0
    };

    // Leverage with the exchange's limits, or the one no-coin hint standing for both: capped from
    // L2 on, its uncapped rest counted with the MAX readout it stands in for.
    let max_readout = if model.no_coin {
        let full = no_coin_hint_full_width(cx, &model.no_coin_text);
        let capped = no_coin_hint_width(cx, &model.no_coin_text, ToolbarLevel(ToolbarLevel::MAX));
        fixed += capped + gap;
        full - capped
    } else {
        fixed += pod + metric_value_width(cx, &model.lev_value);
        captions += caption(&model.captions.lev, None);
        pod_gap + max_readout_width(model, cx)
    };

    // Stop: the mini switch and the value, until L10 folds them into "More".
    captions += caption(&model.captions.sl, None);

    // Take and Sell: their captions go at L5, long before the pods fold into "More".
    captions += caption(&model.captions.tp, None) + caption(&model.captions.sell, None);
    let sell = strips::strip_geometry(&model.sell_cells, cx);
    let glyph = design::glyph_btn_w(cx);

    ToolbarWidths {
        fixed,
        strat_controls,
        sell_strip: sell.full(),
        sell_drop: sell.collapsed(),
        max_readout,
        ms_summary,
        size_strip: size.full(),
        size_drop: size.collapsed(),
        captions,
        chart_tools: glyph * 2.0 + gap,
        overflow: glyph,
        own_icon,
        tp_pod: pod + metric_value_width(cx, &model.tp_value),
        sell_pod: pod,
        size_pod: pod,
        sl_pod: pod + sl_switch_width(cx) + pod_gap + metric_value_width(cx, &model.sl_value),
        live: gap + live_width(cx, &model.live_label),
    }
}

/// Rendered width of the Leverage pod's limits read-out: `of ×max ·`, `max order`, and the value,
/// each text run at its whole-pixel box.
pub(super) fn max_readout_width(model: &ToolbarModel, cx: &App) -> f32 {
    use crate::chrome::terminal_chrome::text_box;
    let pod_gap = design::ui_value(cx, design::POD_GAP);
    let of = model.lev_of.as_deref().map_or(0.0, |text| {
        text_box(design::ui_caption_text_width(cx, text, 400.0)) + pod_gap
    });
    of + text_box(design::ui_caption_text_width(
        cx,
        &model.max_order_inline,
        400.0,
    )) + pod_gap
        + text_box(design::mono_body_text_width(
            cx,
            &model.max_order_value,
            400.0,
        ))
}
