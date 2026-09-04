//! Compose the trading toolbar: size, leverage with an exchange max-order readout, stop loss, TP/S
//! slots, and Live controls.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::feed::{CoreConfigArea, CoreConfigRejection};
use moon_core::session::CoreId;
use rust_i18n::t;

use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSegment, MoonButtonSize, MoonButtonVariant,
    MoonInputState, MoonLabel, MoonPalette, MoonToggle, MoonToggleSize, h_flex,
};

use super::DASH;
use super::metric::{metric_button, sl_toggle};
use super::strips::{self, sell_strip, size_strip};
use super::{MaxOrderReadout, TradeMetric, fmt_field2, fmt_field2_signed};
use crate::backend::{ManualSource, ManualStop};
use crate::panels::common::text_tooltip;
use crate::shell::Shell;
use crate::{Backend, design};
use moon_core::util::fmt;

#[cfg(test)]
mod tests;

/// Caption size for a preset group — one step below the strip's own cells, which render their
/// labels at 11, so 10 reads as a label NAMING the group rather than as another value in it.
const CAPTION_SIZE: f32 = 10.0;

/// Caption for a preset group, muted and one step below the strip's own cells.
///
/// `MoonLabel` rather than a hand-rolled text div: it applies the theme's mono family and runs the
/// size through `tokens.font()` itself, so the caption follows the Font slider like every other
/// MoonUI text. Pass the BASE size — a pre-scaled `design::t_*` value would be scaled twice.
///
/// The text is a literal, not `t!`: `Size`/`Sell` are on the deliberately-untranslated list
/// (`locales/README.md`), as are the neighbouring `Lev`/`SL`/`TP`. The tooltip is translated, the
/// caption is not. The appended `USDT eq.` is a deliberately-untranslated technical unit.
fn strip_caption(text: impl Into<SharedString>, p: MoonPalette) -> impl IntoElement {
    strip_text(text, p.text_muted)
}

/// The row's caption-scale text recipe, shared by every label and readout on it.
///
/// One home for the MoonUI call so a second text cell cannot drift to a different family, size or
/// casing while looking the same in the source. `color` is the only thing that legitimately varies:
/// a caption NAMES something and is muted, while a readout STATES something and is not.
///
/// Args:
///     text: Caption or readout text rendered at the toolbar's shared text scale.
///     color: Active palette color for the text's semantic role.
///
/// Returns:
///     A non-shrinking monospaced toolbar text element.
fn strip_text(text: impl Into<SharedString>, color: u32) -> impl IntoElement {
    div().flex_none().child(
        MoonLabel::new(text)
            .mono(true)
            .color(color)
            .font_size(CAPTION_SIZE)
            .uppercase(false)
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
fn captioned_strip(
    id: &'static str,
    caption: Option<SharedString>,
    p: MoonPalette,
    strip: impl IntoElement,
    tip: Option<SharedString>,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .id(id)
        .flex_none()
        .gap(design::ui_px(cx, design::CHROME_GAP))
        .children(caption.map(|text| strip_caption(text, p)))
        .child(strip)
        .when_some(tip, |el, tip| el.tooltip(text_tooltip(tip)))
}

/// Button widths for this row — the ONE home of these numbers.
///
/// The text-bearing ones are passed through `design::font_w` at the point of use: their consumer
/// (`MoonButton::width`) puts the value into `px(..)` verbatim, so a raw width would squeeze a
/// label that grows with the Font slider — the same ailment the preset cells had.
///
/// [`ICON_BTN_W`] is the exception and stays RAW. An icon-only button holds no text: its glyph is
/// capped at `clamp(font_size + 1, 10, 14)` while its height comes from a fit formula, so a
/// linearly scaled width makes the button LESS square the larger the font — and spends row width on
/// nothing, in the one place on this row where nothing needs the room.
const LEV_W: f32 = 61.6;
/// Base width of the stop-loss metric button.
const SL_W: f32 = 58.0;
/// Base width of the take-profit metric button.
const TP_W: f32 = 74.6;
/// The SL toggle's width, for the row budget ONLY — `MoonToggle` sizes itself and takes no width,
/// so unlike its neighbours nothing renders from these two numbers.
///
/// Split because the widget's halves follow DIFFERENT scales: MoonUI puts the Compact track (28)
/// and its label gap (7) through `tokens.ui()`, while the "SL" label follows the font. Scaling the
/// sum by either one alone drifts the budget on the other slider.
const SL_TOGGLE_TRACK_W: f32 = 35.0;
/// Base width of the SL text beside the toggle, used only by the row budget.
const SL_TOGGLE_LABEL_W: f32 = 13.0;
/// The per-core order-size switch's width, for the row budget ONLY, on the same terms as
/// [`SL_TOGGLE_TRACK_W`]: `MoonToggle` sizes itself and nothing renders from this number. It is the
/// bare Compact track without [`SL_TOGGLE_TRACK_W`]'s label gap, because this switch carries no
/// label — its meaning is in the tooltip.
const OWN_TRADE_TOGGLE_W: f32 = 28.0;
/// Base width of the Live/Pause button.
const LIVE_W: f32 = 62.0;
/// Raw width of each icon-only singleton-window button. Shared so the Report panel's trash
/// button (`panels::report::controls`) matches the toolbar launchers from one source.
pub(crate) const ICON_BTN_W: f32 = 30.0;
/// Base font size of a ToolbarCompact text segment in MoonUI.
const TOOLBAR_LAUNCHER_TEXT_SIZE: f32 = 10.0;
/// Font weight used by the toolbar launchers' localized text segments.
const TOOLBAR_LAUNCHER_TEXT_WEIGHT: f32 = 500.0;
/// Base font size from which MoonUI derives a ToolbarCompact leading-icon size.
const TOOLBAR_LAUNCHER_ICON_FONT_SIZE: f32 = 10.5;
/// UI-scaled gap between a ToolbarCompact leading icon and its label.
const TOOLBAR_LAUNCHER_ICON_GAP: f32 = 6.0;
/// Two raw one-pixel borders enclosing a labeled Soft button's horizontal content.
const TOOLBAR_LAUNCHER_BORDER_W: f32 = 2.0;
/// Horizontal inset on each side of a labeled launcher. Action/ToolbarCompact ship with
/// `pad_x = 0` so icon-only targets stay square; labeled buttons must opt into the same
/// 7-unit inset used by other Action labels (`core_settings_popup`, connections tab).
const TOOLBAR_LAUNCHER_PAD_X: f32 = 7.0;
/// Caption of the sell group — unlike `Size` it carries no unit, the cells already show percents.
const SELL_CAPTION: &str = "Sell";
/// Stable unit for group-local manual order-size equivalents.
const SIZE_UNIT: &str = "USDT eq.";

/// Measure one complete localized launcher button at ToolbarCompact geometry.
///
/// The Shell root supplies the monospaced family inherited by the text segment. MoonUI gives this
/// size zero native padding so icon-only targets stay square; labeled launchers add
/// [`TOOLBAR_LAUNCHER_PAD_X`] on each side via `MoonButton::padding_x`. The reserved width is the
/// leading icon, its UI-scaled gap, both insets, and the two border pixels. The button is never
/// allowed to become narrower than its stable icon-only target.
///
/// Args:
///     cx: Application context supplying active font and UI scales.
///     label: Localized launcher label rendered by the button.
///
/// Returns:
///     Full icon-plus-label width in logical pixels.
fn launcher_label_width(cx: &App, label: &str) -> f32 {
    let text = design::ui_text_width(
        cx,
        label,
        TOOLBAR_LAUNCHER_TEXT_SIZE,
        TOOLBAR_LAUNCHER_TEXT_WEIGHT,
        true,
    );
    let icon = (design::font_value(cx, TOOLBAR_LAUNCHER_ICON_FONT_SIZE) + 1.0).clamp(10.0, 14.0);
    let chrome = icon
        + design::ui_value(cx, TOOLBAR_LAUNCHER_ICON_GAP)
        + design::ui_value(cx, TOOLBAR_LAUNCHER_PAD_X) * 2.0
        + TOOLBAR_LAUNCHER_BORDER_W;
    (text + chrome).max(ICON_BTN_W)
}

/// The localized labels of the three singleton-window launchers at the row's trailing edge.
///
/// Grouped because they are ONE fact — the trailing cluster's text — and the budget reads all
/// three or none of them. Passing them separately also pushed [`row_fit`] past the argument count
/// where a reader stops tracking which string is which.
struct LauncherLabels<'a> {
    analytics: &'a str,
    strategies: &'a str,
    settings: &'a str,
}

/// Incremental widths of the optional labels above the icon-only row.
#[derive(Clone, Copy, Debug)]
struct LabelWidths {
    /// Complete row width with every optional label removed.
    icon_only: f32,
    /// Width of the compact size unit caption.
    size_unit: f32,
    /// Extra width that expands the unit caption to `Size, USDT eq.`.
    size_noun: f32,
    /// Extra width of the Settings launcher label.
    settings: f32,
    /// Extra width of the Strategies launcher label.
    strategies: f32,
    /// Extra width of the Analytics launcher label.
    analytics: f32,
    /// Width of the Sell caption.
    sell: f32,
    /// Extra width of the caption naming the exchange max-order value.
    ///
    /// The VALUE itself is not on this ladder: it is permanently visible and therefore part of the
    /// unsheddable `controls` budget instead — only the word naming it may go.
    max_order_caption: f32,
}

/// Visibility of every optional label at one available row width.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct LabelLadder {
    size_unit: bool,
    size_noun: bool,
    settings: bool,
    strategies: bool,
    analytics: bool,
    sell: bool,
    max_order_caption: bool,
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
fn label_ladder(available: f32, widths: LabelWidths) -> LabelLadder {
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
fn row_fit(
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
    // toggle is the one entry the row does not render from a width — the widget sizes itself, and
    // its two halves follow different scales (see [`SL_TOGGLE_TRACK_W`]).
    let controls = size.total_width()
        + sell.total_width()
        + fw(LEV_W)
        + fw(SL_W)
        + fw(TP_W)
        + design::ui_value(cx, SL_TOGGLE_TRACK_W)
        + fw(SL_TOGGLE_LABEL_W)
        // Budgeted unconditionally even though it is drawn only for an addressed core: a budget
        // that shrank with the switch would let the row fit at a width it cannot hold the moment a
        // chart is addressed, and re-widen only after the clipping had already happened.
        + design::ui_value(cx, OWN_TRADE_TOGGLE_W)
        + fw(LIVE_W)
        + ICON_BTN_W * 5.0
        // The exchange max-order VALUE is permanent — outcome 4 asks for a readout that is always
        // on the row — so it belongs in the unsheddable budget rather than on the ladder. Measured
        // from the REAL rendered string: a coin's cap runs from three digits to nine.
        + design::ui_text_width(cx, max_order_value, CAPTION_SIZE, 400.0, true);
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
    let caption_w = |text: &str| design::ui_text_width(cx, text, CAPTION_SIZE, 400.0, true) + gap;
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
            settings: settings_width - ICON_BTN_W,
            strategies: strategies_width - ICON_BTN_W,
            analytics: analytics_width - ICON_BTN_W,
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
    RowFit {
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

/// The optional labels the row renders at the current window width, already resolved to the values
/// it renders — see [`row_fit`]. `None` means that label does not fit and is not drawn.
struct RowFit {
    size_caption: Option<SharedString>,
    sell_caption: Option<SharedString>,
    /// Complete Analytics-button width when its label fits; `None` renders it icon-only.
    analytics_width: Option<f32>,
    /// Complete Strategies-button width when its label fits; `None` renders it icon-only.
    strategies_width: Option<f32>,
    /// Complete Settings-button width when its label fits; `None` renders it icon-only.
    settings_width: Option<f32>,
    /// The caption naming the permanent max-order figure; `None` leaves the value identified by its tooltip.
    max_order_caption: Option<SharedString>,
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

/// Prefer the hovered chart's core when deciding whether its core-owned manual strategy applies.
pub(crate) fn manual_strategy_core(
    active_core: Option<CoreId>,
    hovered_core: Option<CoreId>,
    hovered_belongs_to_group: bool,
) -> Option<CoreId> {
    hovered_core
        .filter(|_| hovered_belongs_to_group)
        .or(active_core)
}

/// Resolve the chart core whose manual-trading config governs the toolbar's size/exit block —
/// deliberately NOT [`crate::Backend::active_trade_core`], which never answers `None` for a group
/// with live cores (it falls through to that group's first core) and so cannot express "no chart
/// is addressed right now". With no chart in front of the user this answers `None`, and the
/// toolbar then shows group-local values with no core marker — the honest answer.
///
/// This is deliberately NOT the core an order would route to (that is
/// [`effective_manual_strategy_core`]) — goal A had two different "which core" rules in this
/// area and merging them was the bug that doc comment exists to prevent.
///
/// Priority: the hovered chart in this group, then the group's Main chart target, then the
/// remembered Classic selection. Pure and arg-taking, the same shape as [`manual_strategy_core`]
/// and for the same reason its own test exists.
pub(crate) fn chart_display_core(
    hovered: Option<CoreId>,
    hovered_in_group: bool,
    main_target: Option<CoreId>,
    remembered: Option<CoreId>,
) -> Option<CoreId> {
    hovered
        .filter(|_| hovered_in_group)
        .or(main_target)
        .or(remembered)
}

/// Resolve [`chart_display_core`] against live backend state.
///
/// Reads `hovered_chart` through [`crate::panels::ChartPanel::active_target`] rather than
/// `target_at_cursor`: the latter answers `None` the instant the pointer leaves the hovered pane,
/// even while `hovered_chart` itself is still set, which would make the gate flicker away and back
/// on ordinary pane-to-pane pointer movement. `active_target` stays with the panel regardless of
/// pane-level hover, which is also what makes a DETACHED window keep naming its core: the `Pane` is
/// owned by the `ChartEngine` inside the `ChartPanel`, and a detached window re-hosts the same
/// entity, so nothing about the toolbar's group window needs to know the chart left it.
pub(crate) fn effective_chart_display_core(
    backend: &Entity<Backend>,
    group: &str,
    cx: &App,
) -> Option<CoreId> {
    let hovered_core = backend
        .read(cx)
        .hovered_chart
        .clone()
        .and_then(|weak| weak.upgrade())
        .and_then(|chart| chart.read(cx).active_target())
        .map(|(core, _)| core);
    let b = backend.read(cx);
    chart_display_core(
        hovered_core,
        hovered_core.is_some_and(|core| b.core_belongs_to_group(group, core)),
        b.main_chart_target(group).map(|(core, _)| core),
        b.layout
            .active_trade_core_by_group
            .get(group)
            .copied()
            .filter(|&core| b.core_belongs_to_group(group, core)),
    )
}

/// Per-core "keep your own manual-trading set" switch, drawn immediately right of the order-size
/// strip.
///
/// `None` when no chart core is addressed: the switch names ONE core's generation, so a row with
/// no core to name must not draw it. It addresses the same `display_core` the strip beside it was
/// rendered from, so the switch and the numbers it governs can never describe different cores.
///
/// Flipping it on moves this core off its group's shared generation and onto its own, seeded from
/// the group so the numbers do not move under the trader's hands; flipping it off returns the row
/// to the group's, keeping the core's own set for the next time it is switched back on.
fn own_trade_toggle(
    core: Option<CoreId>,
    on: bool,
    backend: &Entity<Backend>,
) -> Option<AnyElement> {
    let core = core?;
    let toggle_backend = backend.clone();
    let tip = if on {
        "toolbar.own_trade_on"
    } else {
        "toolbar.own_trade_off"
    };
    Some(
        h_flex()
            .id("toolbar-own-trade")
            .flex_none()
            .items_center()
            .child(
                MoonToggle::new("toolbar-own-trade-toggle")
                    .checked(on)
                    .size(MoonToggleSize::Compact)
                    .on_change(move |checked: &bool, _w, app| {
                        let on = *checked;
                        toggle_backend.update(app, |b, cx| {
                            b.set_core_own_trade(core, on);
                            cx.notify();
                        });
                    }),
            )
            .tooltip(text_tooltip(SharedString::from(t!(tip).to_string())))
            .into_any_element(),
    )
}

/// Caption naming one coarse [`CoreConfigArea`] the core rejected. `moon-core` cannot localize, so
/// the mapping lives here like every other caption of a `moon-core` enum in this module.
fn area_caption(area: CoreConfigArea) -> String {
    let key = match area {
        CoreConfigArea::AutoBuy => "toolbar.core_config_area_auto_buy",
        CoreConfigArea::AutoStart => "toolbar.core_config_area_auto_start",
        CoreConfigArea::BtcBlink => "toolbar.core_config_area_btc_blink",
        CoreConfigArea::General => "toolbar.core_config_area_general",
        CoreConfigArea::Interface => "toolbar.core_config_area_interface",
        CoreConfigArea::Leverage => "toolbar.core_config_area_leverage",
        CoreConfigArea::Manual => "toolbar.core_config_area_manual",
        CoreConfigArea::Signals => "toolbar.core_config_area_signals",
        CoreConfigArea::Special => "toolbar.core_config_area_special",
        CoreConfigArea::Telegram => "toolbar.core_config_area_telegram",
    };
    t!(key).to_string()
}

/// Whole-block tooltip naming a [`CoreConfigRejection::Areas`] the display core's one retained edit
/// carries, or `None` while nothing is rejected.
///
/// This is the ONLY reader of `CoreData::core_config_edit`. The gear popup still writes AutoStart,
/// BtcBlink, General and Leverage through the shared-config sequence, and a core that refuses one
/// of them resolves the edit as `NotApplied` — without this the popup would close exactly as it
/// does on success and the refusal would never reach the screen.
fn manual_area_rejection_tip(mismatches: Option<&CoreConfigRejection>) -> Option<SharedString> {
    let Some(CoreConfigRejection::Areas(areas)) = mismatches else {
        return None;
    };
    if areas.is_empty() {
        return None;
    }
    let areas = areas
        .iter()
        .map(|&area| area_caption(area))
        .collect::<Vec<_>>()
        .join(", ");
    Some(SharedString::from(
        t!("toolbar.core_config_areas_rejected", areas = areas).to_string(),
    ))
}

/// Resolve the core whose manual-strategy state governs the toolbar and any open metric popup.
pub(crate) fn effective_manual_strategy_core(
    backend: &Entity<Backend>,
    group: &str,
    cx: &App,
) -> Option<CoreId> {
    let hovered_core = backend
        .read(cx)
        .hovered_chart
        .clone()
        .and_then(|weak| weak.upgrade())
        .and_then(|chart| chart.read(cx).target_at_cursor())
        .map(|(core, _)| core);
    let backend = backend.read(cx);
    manual_strategy_core(
        backend.active_trade_core(group),
        hovered_core,
        hovered_core.is_some_and(|core| backend.core_belongs_to_group(group, core)),
    )
}

/// The toolbar strip: an ordinary `Shell` child between the header and the dock, not a dock panel.
/// It reads size and exit state from the window group, plus leverage and manual strategy from the
/// active core. While a chart is hovered, manual-strategy applicability follows that chart's core
/// so the row describes the order target under the pointer.
///
/// `chrome_width` is the window width. The row's controls are all `flex_none`, so nothing shrinks:
/// its optional labels collapse against that width by an explicit priority ([`row_fit`]), because
/// the row would otherwise push the trailing window buttons off the edge.
///
/// Args:
///     backend: Shared terminal state and singleton-window registry.
///     group: Main-window group whose trading controls are rendered.
///     size_edit: Active manual-size editor text and cell index, when any.
///     size_input: Shared input state for the active size editor.
///     sell_edit: Active sell-percentage editor text and cell index, when any.
///     sell_input: Shared input state for the active sell editor.
///     shell: Owning shell entity receiving toolbar actions.
///     settings_hint_at: When the first-run settings hint was armed, if it still is. Passed BY
///         VALUE rather than read off `shell`, because this row is built from inside the shell's
///         own render -- reading that entity here panics as a re-entrant borrow.
///     metric_popup: Active trade metric and its popup contents, when open.
///     max_order: Exchange maximum-order readout for the active leverage target.
///     quote: Quote token displayed beside a present maximum-order value.
///     chrome_width: Available toolbar width in logical pixels.
///     cx: Application context used for state reads and rendering.
///
/// Returns:
///     The complete responsive trading toolbar row.
// `use<>` on the return type: the row holds no input lifetime, and saying so is what lets a caller
// build it inside a scope narrower than the tree it is added to — the diagnostic timer around this
// call in `shell::render` is exactly that. Without it Rust 2024 captures every argument lifetime.
#[allow(clippy::too_many_arguments)]
pub fn toolbar(
    backend: &Entity<Backend>,
    group: &str,
    size_edit: Option<(String, usize)>,
    size_input: &Entity<MoonInputState>,
    sell_edit: Option<(String, usize)>,
    sell_input: &Entity<MoonInputState>,
    shell: &Entity<Shell>,
    settings_hint_at: Option<std::time::Instant>,
    metric_popup: Option<(TradeMetric, AnyElement)>,
    max_order: MaxOrderReadout,
    quote: &str,
    chrome_width: f32,
    cx: &App,
) -> impl IntoElement + use<> {
    let phase_us = crate::diag::timer();
    // Which metric is open and what its popup contains are ONE fact — `Shell` derives both from the
    // same field — so they arrive as one value: passed separately, a caller could name an open
    // metric with no content and the row would light a button over an empty popover. The content
    // does not clone, so it goes to exactly one button.
    let (lev_popup, sl_popup, tp_popup) = match metric_popup {
        Some((TradeMetric::Lev, content)) => (Some(content), None, None),
        Some((TradeMetric::Sl, content)) => (None, Some(content), None),
        Some((TradeMetric::Tp, content)) => (None, None, Some(content)),
        None => (None, None, None),
    };
    let manual_core = effective_manual_strategy_core(backend, group, cx);
    // The core whose manual config governs the toolbar's sizes and exits, independent of
    // manual-strategy applicability above: a chart can be addressed for one without being
    // addressed for the other, and neither gate may be built from the other's result — see
    // `chart_display_core`'s doc.
    let display_core = effective_chart_display_core(backend, group, cx);
    let (
        follow,
        overview,
        focus_core,
        write_matches_display,
        size_values,
        size_sel,
        size_source,
        core_config_edit,
        tp_value,
        tp_engaged,
        sl_value,
        sl_on,
        lev_value,
        sell_pcts,
        sell_slot,
        manual_on,
        sl_locked,
    ) = {
        let b = backend.read(cx);
        // Whether the header scope names one account at all. Read ONCE here so the leverage
        // button, the max-order readout and this row's own core cannot reach three different
        // conclusions about the same scope; the leverage ADDRESS is gated one level down, in
        // `TradeMetric`, so `Shell`'s open-popup guard shares the decision rather than copying it.
        let overview = b.is_auto_overview_scope(group);
        // Group-local size and exit controls do not move with this selection. Leverage reads a
        // core only when the visible scope names one; manual strategy still uses the active trade
        // core. In Overview, `active_trade_core` would answer with the group's first core and the
        // row would present that server's leverage as the group's.
        let focus_core = if overview {
            None
        } else {
            b.active_trade_core(group)
        };
        // Whether a click, double-click, or Ctrl+wheel on the size/sell strips right now would
        // write to the same source this row just displayed. `display_core` is hover-aware while
        // the write always targets `manual_write_core` (-> `active_trade_core`, never hover-aware),
        // so hovering a different chart in the same group can make the two disagree while the
        // strip still renders as live. `false` here disables both strips below (goal A2 FIX-3).
        let write_matches_display = b.manual_display_matches_write(group, display_core);
        // Sizes: [`Backend::effective_order_size_state`] is the one resolver every manual-trading
        // reader shares, so the row cannot reach a different "which core, which source" conclusion
        // than the choke point every write goes through. Both of its sources are local config this
        // terminal owns, so there is no freshness to report and nothing to wait for.
        let (size_values, size_sel, size_source) =
            b.effective_order_size_state(group, display_core);
        // Exits (TP/SL/sell presets): the exit twin of the sizes resolver above, resolved through
        // the same per-core-or-group choice so the two halves cannot disagree about the source.
        let (exit, _exit_source) = b.effective_group_exit(group, display_core);
        // The display core's one retained core-config write attempt, for the rejection notice
        // below. Read from the DISPLAY core so the tooltip describes the same core the row shows.
        let core_config_edit = display_core
            .and_then(|core| b.session.store().core(core))
            .and_then(|data| data.core_config_edit.clone());
        // Manual-strategy mode, and what it does to this row's exits — Moonbot's own arrangement:
        //
        // * the strategy owns the sell price and the stop, so both READOUTS come from it;
        // * the TP popup (slider and field) is closed in this mode: a free-form take profit has
        //   nowhere to go, since the strategy's sell price is a single value;
        // * the S presets are the ONE way to change that sell price, and only while Moonbot's
        //   "ignore the manual strategy's sell price" checkbox is on — with it off the strategy
        //   alone decides and the strip is disabled;
        // * the SL button and its toggle follow the same core's "Moonbot logic" switch: with it on
        //   — the default — the stop is the strategy's and both controls only report it; with it
        //   off they are editable and the visible stop is written to the order once it is placed.
        // Locked only while the core sells a manual order at the STRATEGY's own price: there the
        // terminal's TP and S presets reach nothing. With Moonbot's "ignore the manual strategy's
        // sell price" checkbox on they are ordinary controls again — their value rides along with
        // the order as `planned_sell_price` — so they stay live, slider and hotkeys included.
        let manual_on = manual_core
            .map(|c| {
                b.manual_strat_active(c).is_some() && !b.ignore_strat_sell_price(c).unwrap_or(false)
            })
            .unwrap_or(false);
        // The TP button always shows its own `take_profit_pct`, even while an S slot is engaged;
        // selecting a slot must not replace the value displayed by TP.
        // In manual-strategy mode both readouts come from the STRATEGY, because that is what the
        // core will use for the order; the group's own values return the moment MS goes off.
        // While a manual strategy owns the exits, BOTH readouts come from its overlay; with MS
        // off — or on another chart — they are the saved generation, untouched underneath.
        let manual_exit = manual_core.and_then(|c| b.manual_exit_overlay(c));
        let tp_value = format!(
            "{}%",
            fmt_field2(
                manual_exit
                    .and_then(|ms| ms.take_profit_pct)
                    .unwrap_or(exit.take_profit_pct) as f32
            )
        );
        let tp_engaged = exit.fixed_sell_slot.is_none() && !manual_on;
        // Who owns the stop, decided once and used for the value, the toggle and the lock alike —
        // three readings of one fact that must not diverge. Moonbot's own rule is on by default per
        // core; the switch is in the MS gear popup beside the toggle that turns the mode on.
        let manual_stop = manual_core
            .map(|c| b.manual_stop(c))
            .unwrap_or(ManualStop::Free);
        let sl_locked = manual_stop.locked();
        // SL is signed: `+1.00%` / `-20.00%`, avoiding `--` from manually prefixing a negative
        // value. The strategy stores its stop as a positive distance, so it is negated on the way
        // in and this reads the way the toolbar's own stop does. A DASH where the strategy owns the
        // stop and its value cannot be read: the saved generation would look like an answer here,
        // and it is not the number the order carries.
        let sl_on = manual_stop.stop_on(
            manual_exit
                .map(|ms| ms.stop_on)
                .unwrap_or(exit.stop_loss_enabled),
        );
        let sl_value = match manual_stop {
            ManualStop::Strategy { pct, .. } => format!("{}%", fmt_field2_signed(pct)),
            ManualStop::Unknown => DASH.to_string(),
            ManualStop::Free => format!(
                "{}%",
                fmt_field2_signed(
                    manual_exit
                        .map(|ms| ms.stop_pct)
                        .unwrap_or(exit.stop_loss_pct)
                )
            ),
        };
        let sell_pcts = exit.fixed_sell_pcts;
        let sell_slot = exit.fixed_sell_slot;
        // Leverage is the Main chart market's per-core, per-market value from assets.
        // `seed_value` rather than `current`: it carries the same "0 means not set" rule the
        // popup's own open guard uses, so the dash and the disabled button cannot disagree with it.
        let lev_value = TradeMetric::Lev
            .seed_value(b, group)
            .map(|l| format!("×{}", l as i32));
        // The target chart wins over the header selection while it is hovered: mouse and market
        // hotkeys address that chart's independent core, whose manual strategy can override the
        // visible group exit values.
        (
            b.follow,
            overview,
            focus_core,
            write_matches_display,
            size_values,
            size_sel,
            size_source,
            core_config_edit,
            tp_value,
            tp_engaged,
            sl_value,
            sl_on,
            lev_value,
            sell_pcts,
            sell_slot,
            manual_on,
            sl_locked,
        )
    };
    let p = MoonPalette::active(cx);
    // `p.blue` in both themes, not the light theme's `p.accent`: the light Blue button uses
    // `p.accent` as its opaque fill, so repeating that token for the TP segment would merge the
    // percentage into its selected background.
    let tp_color = p.blue;
    let sl_color = design::danger_color(p);

    // Whether each metric can be edited at all. Derived through `TradeMetric` from the state this
    // block already read, so the row and the `Shell` state that outlives an open popup cannot come
    // to different conclusions about the same metric.
    //
    // Leverage alone needs a live value. Group-owned TP and SL are complete even before a core
    // connects, so their availability depends only on manual-strategy and SL-toggle state.
    let has_core = focus_core.is_some();
    let lev_available = TradeMetric::Lev.available_with(has_core, sl_on, manual_on, sl_locked)
        && lev_value.is_some();
    let tp_available = TradeMetric::Tp.available_with(has_core, sl_on, manual_on, sl_locked);
    let sl_available = TradeMetric::Sl.available_with(has_core, sl_on, manual_on, sl_locked);
    let lev_str = lev_value.unwrap_or_else(|| "—".to_string());

    // Cells are fitted BEFORE rendering: both the strip itself and the row budget that decides the
    // labels' fate read them. One computation, one source.
    crate::diag::record_us(&crate::diag::TOOLBAR_DATA_US, phase_us);
    let phase_us = crate::diag::timer();
    let size_cells = strips::FittedCells::fit(cx, strips::size_labels(size_values));
    let sell_cells = strips::FittedCells::fit(cx, strips::sell_labels(sell_pcts));
    // Two whole-block notices, in priority order. The strips going non-interactive comes first: it
    // explains why nothing on this row can be clicked (goal A2 FIX-3). A core-config rejection is
    // next — the numbers on the row are local config with nothing to be stale about, but the gear
    // popup's write to this core can still have been refused, and this tooltip is where that fact
    // surfaces.
    let manual_block_tip = (!write_matches_display)
        .then(|| SharedString::from(t!("toolbar.core_manual_mismatch").to_string()))
        .or_else(|| {
            manual_area_rejection_tip(
                core_config_edit
                    .as_ref()
                    .and_then(|row| row.mismatches.as_ref()),
            )
        });
    // The exchange's own cap on a single order, kept permanently on the row rather than only inside
    // the leverage popover: it bounds every order the row above it composes, and a cap you have to
    // open a popup to see is a cap you check after sizing rather than before. Compact here, exact in
    // the popover — one value from one read, at two precisions.
    let max_order_caption = t!("toolbar.max_order_short").to_string();
    // A cap is one exchange account's rule. `Shell` resolved this readout through
    // `TradeMetric::Lev.target`, which already answers `None` in Overview — but `None` there means
    // `NoData`, whose hover text says the limits have not loaded yet. That is a false explanation
    // for a dash the scope caused, so the state is named explicitly instead: value and tooltip then
    // come from ONE value and cannot disagree about why the figure is absent.
    let max_order = if overview {
        MaxOrderReadout::OutOfScope
    } else {
        max_order
    };
    let max_order_value = max_order.format_compact(fmt::compact_si, quote);
    let max_order_tip = t!(max_order.tooltip_key()).to_string();
    let analytics_label = t!("toolbar.analytics").to_string();
    let strategies_label = t!("toolbar.strategies").to_string();
    let settings_label = t!("shell.settings_btn").to_string();
    let fit = row_fit(
        cx,
        chrome_width,
        &size_cells,
        &sell_cells,
        LauncherLabels {
            analytics: &analytics_label,
            strategies: &strategies_label,
            settings: &settings_label,
        },
        &max_order_caption,
        &max_order_value,
    );
    crate::diag::record_us(&crate::diag::TOOLBAR_FIT_US, phase_us);
    let phase_us = crate::diag::timer();

    // A section carries the gap INSIDE it; the boundary between two sections is drawn by the RULE
    // standing between them, not by a wider gap. Shared with the header — see
    // `design::chrome_section`.
    let section = || design::chrome_section(cx);

    let mut row = h_flex()
        .id("toolbar")
        .w_full()
        .h(px(design::toolbar_height(cx)))
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, design::CHROME_GAP))
        .px(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(rgb(p.shell_high))
        // The bottom border bounds the whole chrome block (header + toolbar share one background)
        // against the transparent dock region below — it is not a seam between the two rows, which
        // is why the header carries none.
        .border_b_1()
        .border_color(rgb(p.border));

    row = row
        // §0 SOURCE. Leftmost, before everything it governs: this switch decides WHICH generation
        // of sizes and exits the rest of the row shows and edits, so it reads as the row's subject
        // rather than as one more control inside the size group.
        .children(own_trade_toggle(
            display_core,
            size_source == ManualSource::CoreOwn,
            backend,
        ))
        // §1 ORDER SIZE. Follows the switch: it is the quantity the other three sections modify —
        // leverage scales it, the stop bounds it, and TP/S define its target exit.
        .child(
            section().child(captioned_strip(
                "toolbar-size-caption",
                fit.size_caption,
                p,
                size_strip(
                    &size_cells,
                    size_sel,
                    // Show the editor only when the request belongs to this toolbar's group.
                    size_edit
                        .filter(|(edit_group, _)| edit_group == group)
                        .map(|(_, i)| i),
                    size_input,
                    backend.clone(),
                    // `None` disables the strip when the displayed core and the write target
                    // disagree (goal A2 FIX-3) — a live control must not mutate a source other
                    // than the one this row just showed.
                    write_matches_display.then(|| group.to_string()),
                    SIZE_UNIT,
                ),
                manual_block_tip.clone(),
                cx,
            )),
        )
        // §2 LEVERAGE — its own section rather than an appendix to size: a leverage edit goes TO
        // THE EXCHANGE (`session.set_leverage`, behind an explicit Apply button in the popup),
        // whereas an order size is a local preset in the config. Different blast radius, different
        // group.
        .child(design::chrome_divider(cx, p))
        .child(
            section()
                .child(metric_button(
                    TradeMetric::Lev,
                    lev_str,
                    p.text,
                    design::font_w(cx, LEV_W),
                    lev_popup.is_some(),
                    false,
                    lev_available,
                    lev_popup,
                    shell.clone(),
                    p,
                    cx,
                ))
                // The exchange max order joins THIS section rather than opening one of its own: it
                // constrains the same "how large, at what leverage" decision the section already
                // owns, and a section of its own would need a rule plus a root gap — the rule is
                // pinned by `toolbar_row_budget_counts_every_rule_it_draws` and the gap count is
                // guarded by nothing at all.
                .children(fit.max_order_caption.map(|text| strip_caption(text, p)))
                // The VALUE is unconditional: this readout exists so the exchange's cap is on
                // screen while an order is being sized, and a figure that disappears on a narrow
                // window is not that. Only its caption yields to width — the tooltip below still
                // names the figure once the word is gone.
                .child(
                    div()
                        .id("toolbar-max-order")
                        .flex_none()
                        .child(strip_text(max_order_value, p.text))
                        .tooltip(crate::panels::common::text_tooltip(max_order_tip)),
                ),
        )
        // §3 RISK: the on/off toggle (`panic_if_price_drop`) plus the value button and its popup.
        .child(design::chrome_divider(cx, p))
        .child(
            section()
                // Disabled only where the stop is not this terminal's to move: with a manual
                // strategy selected and Moonbot's own rule in force, this reports the strategy's
                // `UseStopLoss`. Turn that rule off in the MS popup and it is editable again.
                .child(sl_toggle(
                    sl_on,
                    sl_locked,
                    backend.clone(),
                    group.to_string(),
                ))
                .child(metric_button(
                    TradeMetric::Sl,
                    sl_value,
                    sl_color,
                    design::font_w(cx, SL_W),
                    sl_popup.is_some(),
                    false,
                    sl_available,
                    sl_popup,
                    shell.clone(),
                    p,
                    cx,
                )),
        )
        // §4 EXIT: TP and the S-slot strip are one and the same sell target, and exactly one of
        // them is lit. Hence ONE section: a rule between them would claim they are separate things.
        .child(design::chrome_divider(cx, p))
        .child({
            let strip = sell_strip(
                &sell_cells,
                sell_slot.filter(|_| !manual_on),
                // Show the S editor only when the request belongs to this toolbar's group.
                sell_edit
                    .filter(|(edit_group, _)| edit_group == group && !manual_on)
                    .map(|(_, i)| i),
                sell_input,
                backend.clone(),
                // Disabled only where a click would reach nothing: a manual strategy owning the
                // sell price. A displayed core disagreeing with the write target disables it too
                // (goal A2 FIX-3).
                (!manual_on && write_matches_display).then(|| group.to_string()),
            );
            let sell_block = captioned_strip(
                "toolbar-sell-caption",
                fit.sell_caption,
                p,
                strip,
                manual_block_tip.clone(),
                cx,
            );
            section()
                .child(metric_button(
                    TradeMetric::Tp,
                    tp_value,
                    tp_color,
                    design::font_w(cx, TP_W),
                    tp_popup.is_some(),
                    tp_engaged && !manual_on,
                    tp_available,
                    tp_popup,
                    shell.clone(),
                    p,
                    cx,
                ))
                .child(sell_block)
        });
    // Scale is configured per tab in the chart-tab strip beside the settings button; see
    // controls::scale_dropdown_for_tabs / chart_tabs::ChartTabs::pick_active_scale.

    let live_tone = if follow {
        design::positive_color(p)
    } else {
        p.text_muted
    };
    let live_label = if follow {
        t!("toolbar.live").to_string()
    } else {
        t!("toolbar.pause").to_string()
    };
    let backend_live = backend.clone();
    // §5 SESSION — Live is fenced off from the trading parameters to its left: it governs whether
    // the chart follows the market, not anything about an order.
    row = row.child(design::chrome_divider(cx, p)).child(
        section().child(
            MoonButton::new("live")
                .width(design::font_w(cx, LIVE_W))
                .variant(MoonButtonVariant::Soft)
                .size(MoonButtonSize::ToolbarCompact)
                // Keep the localized interaction hint reachable without adding another row label.
                .tooltip(t!("toolbar.live_tip").to_string())
                .segment(
                    MoonButtonSegment::new("●")
                        .color(live_tone)
                        .font_size(9.0)
                        .weight(700.0),
                )
                .segment(
                    MoonButtonSegment::new(live_label)
                        .color(live_tone)
                        .weight(500.0),
                )
                .on_click(move |_, _, cx| {
                    backend_live.update(cx, |b, bcx| {
                        b.follow = !b.follow;
                        bcx.notify();
                    });
                })
                .render(),
        ),
    );
    crate::diag::record_us(&crate::diag::TOOLBAR_TRADE_US, phase_us);
    let phase_us = crate::diag::timer();
    // Trailing edge: Profit Monitor + Screener, then Strategies + Analytics, then Settings.
    let row = row
        .child(div().flex_1())
        .child(design::chrome_divider(cx, p))
        .child(
            section()
                .child(open_window_button(
                    "toolbar-profit-monitor",
                    t!("toolbar.profit_monitor").to_string(),
                    "icons/trending-up.svg",
                    None,
                    None,
                    backend.clone(),
                    crate::analytics::profit_monitor::open,
                    p,
                ))
                .child(open_window_button(
                    "toolbar-screener",
                    t!("toolbar.screener").to_string(),
                    "icons/chart-pie.svg",
                    None,
                    None,
                    backend.clone(),
                    crate::screener::open,
                    p,
                )),
        )
        .child(design::chrome_divider(cx, p))
        .child(
            section()
                .child(open_window_button(
                    "toolbar-strategies",
                    strategies_label,
                    super::STRATEGIES_ICON,
                    fit.strategies_width,
                    Some(group.to_string()),
                    backend.clone(),
                    crate::strategies::open,
                    p,
                ))
                .child(open_window_button(
                    "toolbar-analytics",
                    analytics_label,
                    "icons/layout-dashboard.svg",
                    fit.analytics_width,
                    Some(group.to_string()),
                    backend.clone(),
                    crate::analytics::open,
                    p,
                )),
        )
        .child(design::chrome_divider(cx, p))
        .child(
            section().child(
                // The first-run hint. TWO conditions, not one: the timer decides how long the ring
                // breathes, and the saved config decides whether it is still relevant at all -- so the
                // moment a core is saved the ring is gone on the NEXT FRAME rather than at the end of
                // its timer. Read from `backend.config`, never from the Settings draft: an unsaved row
                // the user is still typing into is not a configured core.
                div()
                    .relative()
                    .child(open_window_button(
                        "toolbar-settings",
                        settings_label,
                        "icons/settings.svg",
                        fit.settings_width,
                        None,
                        backend.clone(),
                        crate::settings::open,
                        p,
                    ))
                    // Declared AFTER the button so the ring paints on top of its chrome; it is a
                    // pointer-transparent overlay and takes no clicks from the control beneath.
                    .children(
                        settings_hint_at
                            .filter(|_| !backend.read(cx).config.core_ever_configured())
                            .and_then(|at| crate::pulse::attention_ring(p.accent, at)),
                    ),
            ),
        );
    crate::diag::record_us(&crate::diag::TOOLBAR_LAUNCH_US, phase_us);
    row
}

/// A toolbar button that opens a singleton window, styled like Live
/// (Soft/ToolbarCompact). `labeled_width = None` renders the icon alone with its name as a tooltip;
/// `Some(w)` renders icon + label inside the fixed width `w`. Every destination uses one shared
/// open signature and deduplicates or focuses its own singleton window.
///
/// Args:
///     id: Stable button element identity.
///     label: Localized visible label or icon tooltip.
///     icon: MoonUI asset path for the launcher glyph.
///     labeled_width: Fixed labeled width, or `None` for an icon-only button.
///     workspace_owner: Group to record before opening a workspace-scoped singleton.
///     backend: Shared terminal state passed to the destination.
///     open: Singleton-window entry point invoked by the click.
///     p: Active palette used for icon and text colors.
///
/// Returns:
///     One rendered compact launcher button.
#[allow(clippy::too_many_arguments)]
fn open_window_button(
    id: &'static str,
    label: String,
    icon: &'static str,
    labeled_width: Option<f32>,
    workspace_owner: Option<String>,
    backend: Entity<Backend>,
    open: fn(Entity<Backend>, Option<AnyWindowHandle>, Option<DisplayId>, &mut App),
    p: MoonPalette,
) -> impl IntoElement {
    let mut btn = MoonButton::new(id)
        // The icon-only width stays raw — see [`ICON_BTN_W`].
        .width(labeled_width.unwrap_or(ICON_BTN_W))
        .variant(MoonButtonVariant::Soft)
        .size(MoonButtonSize::ToolbarCompact)
        .leading_icon(MoonButtonIconSlot::new(icon).color(p.text_soft));
    btn = if labeled_width.is_some() {
        btn.padding_x(TOOLBAR_LAUNCHER_PAD_X)
            .text_segment(label, p.text, 500.0)
    } else {
        btn.tooltip(label)
    };
    btn.on_click(move |_, window, cx| {
        if let Some(group) = workspace_owner.as_deref() {
            backend.update(cx, |backend, backend_cx| {
                backend.focus_singleton_owner(group, backend_cx);
            });
        }
        let owner_display = window.display(cx).map(|d| d.id());
        open(
            backend.clone(),
            Some(window.window_handle()),
            owner_display,
            cx,
        );
    })
    .render()
}
