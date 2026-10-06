//! Row fitting: launcher widths, the label ladder and the overflow fold.

use super::*;

/// Measure one complete localized launcher button at control-tier geometry.
///
/// The label is measured AND rendered in [`design::ui_font`]: it is a control caption, and the two
/// sections that host labeled launchers set that family so `MoonButton`'s text segment inherits it
/// (the segment pins a family only when its own `mono` flag is set). Measuring in the other family
/// would budget the whole trailing cluster wrongly at every `row_fit` shedding threshold. MoonUI
/// gives this size zero native padding so icon-only targets stay square; labeled launchers add
/// [`TOOLBAR_LAUNCHER_PAD_X`] on each side via `MoonButton::padding_x`. The reserved width is the
/// leading icon plus its control-tier gap ([`design::action_icon_reservation`]), both insets, and
/// the two border pixels. The button is never allowed to become narrower than
/// [`design::glyph_btn_w`].
///
/// Args:
///     cx: Application context supplying active font and UI scales.
///     label: Localized launcher label rendered by the button.
///
/// Returns:
///     Full icon-plus-label width in logical pixels.
pub(super) fn launcher_label_width(cx: &App, label: &str) -> f32 {
    // The UI family, not the monospaced one: a launcher label is a control caption and its
    // buttons render it in `design::ui_font()` (the two sections that host them set it). This
    // measurement sizes the button the label is drawn in, so the two must name the same family --
    // measure mono, draw proportional, and the whole trailing cluster is budgeted too wide.
    let text = design::ui_text_width_zoomed(
        cx,
        label,
        design::BODY_TEXT,
        TOOLBAR_LAUNCHER_TEXT_WEIGHT,
        false,
    );
    let chrome = design::action_icon_reservation(cx)
        + design::ui_value(cx, TOOLBAR_LAUNCHER_PAD_X) * 2.0
        + TOOLBAR_LAUNCHER_BORDER_W;
    (text + chrome).max(design::glyph_btn_w(cx))
}

/// Width of the SL toggle (track + gap + "SL" label) at the density-default tier.
///
/// The label is measured in the mono family because `sl_toggle` sets no `.mono()` and `MoonToggle`
/// defaults `mono: true`.
fn sl_toggle_width(cx: &App) -> f32 {
    let m = MoonToggleSize::density_default(&MoonTheme::active_tokens(cx)).reference_metrics();
    design::ui_value(cx, m.track_width + m.gap)
        + design::ui_text_width_zoomed(cx, "SL", m.font_size, m.label_weight, true)
}

/// Width of the unlabeled own-trade toggle track at the density-default tier.
fn own_trade_toggle_width(cx: &App) -> f32 {
    let m = MoonToggleSize::density_default(&MoonTheme::active_tokens(cx)).reference_metrics();
    design::ui_value(cx, m.track_width)
}

/// Resolve the cumulative label ladder without any rendering or theme dependency.
///
/// Each threshold adds exactly the next label that survives when the toolbar grows. Inclusive
/// comparisons make a label visible at the exact pixel where its complete width first fits.
///
/// Args:
///     available: Toolbar width available to the complete row.
///     widths: Icon-only base and incremental optional-label widths.
///
/// Returns:
///     Visibility flags for the seven ordered ladder rungs.
pub(super) fn label_ladder(available: f32, widths: LabelWidths) -> LabelLadder {
    let size_unit = widths.icon_only + widths.size_unit;
    let size_noun = size_unit + widths.size_noun;
    let settings = size_noun + widths.settings;
    let strategies = settings + widths.strategies;
    let analytics = strategies + widths.analytics;
    let sell = analytics + widths.sell;
    let max_order_caption = sell + widths.max_order_caption;

    LabelLadder {
        size_unit: available >= size_unit,
        size_noun: available >= size_noun,
        settings: available >= settings,
        strategies: available >= strategies,
        analytics: available >= analytics,
        sell: available >= sell,
        max_order_caption: available >= max_order_caption,
    }
}

/// Resolve the overflow fold without any rendering or theme dependency.
///
/// The label ladder runs first and sheds labels only; once even the icon-only row outgrows
/// `available`, launchers fold one by one in [`LAUNCHER_FOLD_ORDER`] until the row, now carrying
/// the always-visible overflow button, fits. Nothing folds while the icon-only row fits, so the
/// overflow button exists only when something is in it. If every launcher is folded and the row
/// still does not fit, the fold stops there — the rest of the row is trading controls, which never
/// leave it — and the overflow button is `pinned` to the row's right edge instead.
///
/// Args:
///     available: Toolbar width available to the complete row.
///     widths: Icon-only row width and per-button launcher and overflow costs.
///
/// Returns:
///     The folded launcher count, the resulting budgeted row width and whether the overflow
///     button must pin itself to the right edge.
pub(super) fn launcher_fold(available: f32, widths: LauncherFoldWidths) -> LauncherFold {
    if available >= widths.icon_only {
        return LauncherFold {
            folded: 0,
            width: widths.icon_only,
            pinned: false,
        };
    }
    let width_at =
        |folded: usize| widths.icon_only - widths.launcher * folded as f32 + widths.overflow;
    let folded = (1..=LAUNCHER_FOLD_ORDER.len())
        .find(|&folded| available >= width_at(folded))
        .unwrap_or(LAUNCHER_FOLD_ORDER.len());
    let width = width_at(folded);
    LauncherFold {
        folded,
        width,
        pinned: available < width,
    }
}

/// Which of the row's optional LABELS fit a window of width `chrome_width`.
///
/// The row's controls do not shrink, so at some width the labels are all that is left to give. This
/// is the one place that decides which ones go, and it resolves them to the values the row renders
/// so nothing downstream can reach a different conclusion.
///
/// The thresholds nest by construction: each rung adds one optional label to the unsheddable row
/// budget. Seven direct comparisons therefore decide the seven-rung ladder without enumerating
/// combinations of visible labels.
///
/// Yield order, most expendable first. Every rung sheds a LABEL; no control ever leaves the row.
/// The exchange max-order VALUE is deliberately absent from this ladder: it is a permanent readout,
/// so it sits in the unsheddable budget and never leaves the row. Only the word naming it yields —
/// and it yields FIRST, because the value it labels keeps a tooltip that says what the figure is.
///
/// 1. **the max-order caption** — the value stays, and its tooltip already names it;
/// 2. **the `Sell` caption** — its strip stands against the `TP` button, which names the same
///    concept one control away;
/// 3. **the Analytics button's label** — its dashboard glyph keeps the full tooltip;
/// 4. **the Strategies button's label** — its bot glyph keeps the full tooltip;
/// 5. **the Settings button's label** — the gear glyph keeps the full tooltip;
/// 6. **the `Size, ` noun** — six numeric presets at the head of a trading toolbar are recognisable
///    without being named;
/// 7. **the unit** — last, because it is the one fact the digits cannot carry themselves. Even
///    then the cell tooltip still spells it out.
///
/// Measured from the REAL cell widths, which depend on the preset values and the font size, rather
/// than from constant thresholds: a fixed threshold cannot model a width that varies. The budget
/// includes the TRAILING CLUSTER — comparing against the window width alone would keep labels
/// visible while the window buttons are already pushed off the right edge.
///
/// Args:
///     cx: Application context supplying theme-aware scale and text measurements.
///     chrome_width: Available toolbar width in logical pixels.
///     size: Pre-fitted manual-size cells.
///     sell: Pre-fitted sell-percentage cells.
///     launchers: Localized labels of the three trailing singleton-window launchers.
///     max_order_caption: Localized caption naming the exchange max-order readout.
///     max_order_value: The max-order figure as it will actually be rendered, measured verbatim.
///
/// Returns:
///     Optional captions and complete launcher widths for the current row.
pub(super) fn row_fit(
    cx: &App,
    chrome_width: f32,
    size: &strips::FittedCells,
    sell: &strips::FittedCells,
    launchers: LauncherLabels<'_>,
    max_order_caption: &str,
    max_order_value: &str,
) -> RowFit {
    let gap = design::ui_value(cx, design::CHROME_GAP);
    let fw = |v: f32| design::font_w(cx, v);
    // Everything the row cannot shed, with the settings button at its icon-only width. The SL
    // toggle is the one entry the row does not render from a width — the widget sizes itself, so
    // the budget reads the live density-default metrics via [`sl_toggle_width`].
    let controls = size.total_width()
        + sell.total_width()
        + fw(LEV_W)
        + fw(SL_W)
        + fw(TP_W)
        + sl_toggle_width(cx)
        // Budgeted unconditionally even though it is drawn only for an addressed core: a budget
        // that shrank with the switch would let the row fit at a width it cannot hold the moment a
        // chart is addressed, and re-widen only after the clipping had already happened.
        + own_trade_toggle_width(cx)
        + fw(LIVE_W)
        + design::glyph_btn_w(cx) * 5.0
        // The exchange max-order VALUE is permanent — outcome 4 asks for a readout that is always
        // on the row — so it belongs in the unsheddable budget rather than on the ladder. Measured
        // from the REAL rendered string: a coin's cap runs from three digits to nine.
        + design::ui_text_width_zoomed(cx, max_order_value, design::BODY_TEXT, 400.0, true);
    // Seven 1px rules — the hairline is deliberately NOT font-scaled (see `design::vline`). Pinned
    // against the row itself by `toolbar_row_budget_counts_every_rule_it_draws` in
    // `tests/theme_contract/shell.rs`: adding a section here without updating this count is invisible
    // until the trailing cluster clips off the edge of some narrow window.
    let rules = 7.0;
    // Row gaps: 16 between the 17 root children (the leading per-core switch and both sides of the
    // zero-width spacer included) plus 5 inside sections — one in Leverage, one in Risk, one in
    // Exit, one between Profit Monitor and Screener, and one between Analytics and Strategies.
    // Settings is a one-child section and adds none.
    // Count them ALL: an undercount moves every threshold, so a label stays visible after the row's
    // fixed part has already outgrown the window — and the spacer cannot shrink past zero.
    //
    // LEVERAGE earned its in-section gap when the permanent max-order value joined the metric
    // button there. That gap is counted HERE rather than on the ladder because the value never
    // sheds; the max-order CAPTION does shed, and its own preceding gap travels inside its ladder
    // width (see `caption_w`), so counting it again here would double it.
    let gaps = gap * 21.0;
    let base = design::ui_value(cx, design::HEADER_PAD_X) * 2.0 + controls + rules + gaps;
    // A caption costs its own width plus the gap separating it from its strip.
    let caption_w =
        |text: &str| design::ui_text_width_zoomed(cx, text, design::BODY_TEXT, 400.0, true) + gap;
    let full_caption = size_caption_text();
    let unit_caption_width = caption_w(SIZE_UNIT);
    let full_caption_width = caption_w(&full_caption);
    let analytics_width = launcher_label_width(cx, launchers.analytics);
    let strategies_width = launcher_label_width(cx, launchers.strategies);
    let settings_width = launcher_label_width(cx, launchers.settings);
    let ladder = label_ladder(
        chrome_width,
        LabelWidths {
            icon_only: base,
            size_unit: unit_caption_width,
            size_noun: (full_caption_width - unit_caption_width).max(0.0),
            settings: settings_width - design::glyph_btn_w(cx),
            strategies: strategies_width - design::glyph_btn_w(cx),
            analytics: analytics_width - design::glyph_btn_w(cx),
            sell: caption_w(SELL_CAPTION),
            // Measured from the REAL rendered strings, not a constant: the digit count of a max
            // order differs by orders of magnitude between coins, and the caption is localized.
            max_order_caption: caption_w(max_order_caption),
        },
    );

    let size_caption = if ladder.size_noun {
        Some(full_caption)
    } else {
        ladder.size_unit.then(|| SharedString::from(SIZE_UNIT))
    };
    // Past the last label rung the launchers themselves fold into the overflow button. Each one
    // costs its glyph and the gap in front of it; a section a fold empties also frees its rule and
    // root gap, which this deliberately does not credit — the budget errs wide, never short.
    let launcher_w = design::glyph_btn_w(cx) + gap;
    let launchers = launcher_fold(
        chrome_width,
        LauncherFoldWidths {
            icon_only: base,
            launcher: launcher_w,
            overflow: launcher_w,
        },
    );
    RowFit {
        launchers,
        size_caption,
        sell_caption: ladder.sell.then(|| SharedString::from(SELL_CAPTION)),
        analytics_width: ladder.analytics.then_some(analytics_width),
        strategies_width: ladder.strategies.then_some(strategies_width),
        settings_width: ladder.settings.then_some(settings_width),
        max_order_caption: ladder
            .max_order_caption
            .then(|| SharedString::from(max_order_caption.to_string())),
    }
}

/// Caption of the order-size group together with its unit.
///
/// Manual sizes are displayed as one USDT equivalent for the whole group and converted only when
/// an order targets a particular core.
///
/// The unit lives on the caption rather than in all six cells. Narrow layouts retain the compact
/// `USDT eq.` caption, and every cell tooltip repeats the same unit.
fn size_caption_text() -> SharedString {
    SharedString::from("Size, USDT eq.")
}
