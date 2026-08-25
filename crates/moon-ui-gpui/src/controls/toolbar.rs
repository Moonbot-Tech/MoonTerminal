//! Compose the trading toolbar: size, leverage with an exchange max-order readout, stop loss, TP/S
//! slots, and Live controls.

use gpui::*;
use moon_core::session::CoreId;
use rust_i18n::t;

use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSegment, MoonButtonSize, MoonButtonVariant,
    MoonInputState, MoonLabel, MoonPalette, h_flex,
};

use super::metric::{metric_button, sl_toggle};
use super::strips::{self, sell_strip, size_strip};
use super::{MaxOrderReadout, TradeMetric, fmt_field2, fmt_field2_signed};
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
fn captioned_strip(
    caption: Option<SharedString>,
    p: MoonPalette,
    strip: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .flex_none()
        .gap(design::ui_px(cx, design::CHROME_GAP))
        .children(caption.map(|text| strip_caption(text, p)))
        .child(strip)
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
    // Row gaps: 15 between the 16 root children (both sides of the zero-width spacer included)
    // plus 5 inside sections — one in Leverage, one in Risk, one in Exit, one between Profit
    // Monitor and Screener, and one between Analytics and Strategies. Settings is a one-child
    // section and adds none.
    // Count them ALL: an undercount moves every threshold, so a label stays visible after the row's
    // fixed part has already outgrown the window — and the spacer cannot shrink past zero.
    //
    // LEVERAGE earned its in-section gap when the permanent max-order value joined the metric
    // button there. That gap is counted HERE rather than on the ladder because the value never
    // sheds; the max-order CAPTION does shed, and its own preceding gap travels inside its ladder
    // width (see `caption_w`), so counting it again here would double it.
    let gaps = gap * 20.0;
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
) -> impl IntoElement {
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
    let (
        follow,
        overview,
        focus_core,
        size_values,
        size_sel,
        tp_value,
        tp_engaged,
        sl_value,
        sl_on,
        lev_value,
        sell_pcts,
        sell_slot,
        manual_on,
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
        let (size_values, size_sel) = b.manual_order_size_state(group);
        let exit = b.group_exit_settings(group);
        // The TP button always shows its own `take_profit_pct`, even while an S slot is engaged;
        // selecting a slot must not replace the value displayed by TP.
        let tp_value = format!("{}%", fmt_field2(exit.take_profit_pct as f32));
        let tp_engaged = exit.fixed_sell_slot.is_none();
        // SL is signed: `+1.00%` / `-20.00%`, avoiding `--` from manually prefixing a negative value.
        let sl_value = format!("{}%", fmt_field2_signed(exit.stop_loss_pct));
        // The toggle beside SL controls `panic_if_price_drop`; while off, the SL button is disabled.
        let sl_on = exit.stop_loss_enabled;
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
        let manual_on = manual_core
            .map(|c| b.manual_strat_state(c).0)
            .unwrap_or(false);
        (
            b.follow,
            overview,
            focus_core,
            size_values,
            size_sel,
            tp_value,
            tp_engaged,
            sl_value,
            sl_on,
            lev_value,
            sell_pcts,
            sell_slot,
            manual_on,
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
    let lev_available =
        TradeMetric::Lev.available_with(has_core, sl_on, manual_on) && lev_value.is_some();
    let tp_available = TradeMetric::Tp.available_with(has_core, sl_on, manual_on);
    let sl_available = TradeMetric::Sl.available_with(has_core, sl_on, manual_on);
    let lev_str = lev_value.unwrap_or_else(|| "—".to_string());

    // Cells are fitted BEFORE rendering: both the strip itself and the row budget that decides the
    // labels' fate read them. One computation, one source.
    let size_cells = strips::FittedCells::fit(cx, strips::size_labels(size_values));
    let sell_cells = strips::FittedCells::fit(cx, strips::sell_labels(sell_pcts));
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
        // §1 ORDER SIZE. Leads the row: it is the quantity the other three sections modify —
        // leverage scales it, the stop bounds it, and TP/S define its target exit.
        .child(
            section().child(captioned_strip(
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
                    group.to_string(),
                    SIZE_UNIT,
                ),
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
                .child(sl_toggle(
                    sl_on,
                    manual_on,
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
                // Manual strategy mode does not apply these slots, so none is lit.
                sell_slot.filter(|_| !manual_on),
                // Show the S editor only when the request belongs to this toolbar's group.
                sell_edit
                    .filter(|(edit_group, _)| edit_group == group && !manual_on)
                    .map(|(_, i)| i),
                sell_input,
                backend.clone(),
                // Manual strategy mode disables all click, wheel, and double-click interaction.
                (!manual_on).then(|| group.to_string()),
            );
            // MoonUI dims disabled cells once. An outer opacity would multiply that alpha in
            // manual-strategy mode and make the strip substantially darker than the disabled TP
            // button beside it.
            let sell_block = captioned_strip(fit.sell_caption, p, strip, cx);
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
    // Trailing edge: Profit Monitor + Screener, then Strategies + Analytics, then Settings.
    row.child(div().flex_1())
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
                    "icons/bot.svg",
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
        )
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
                backend.focus_auto_workspace(group, backend_cx);
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
