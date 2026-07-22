//! Compose the trading toolbar: size, leverage, stop loss, TP/S slots, and Live controls.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSegment, MoonButtonSize, MoonButtonVariant,
    MoonInputState, MoonLabel, MoonPalette, h_flex,
};

use moon_core::session::CoreId;

use super::metric::{metric_button, sl_toggle};
use super::strips::{self, sell_strip, size_strip};
use super::{TradeMetric, fmt_field2, fmt_field2_signed};
use crate::shell::Shell;
use crate::{Backend, design};

/// Caption size for a preset group — one step below the strip's own cells, which render their
/// labels at 11, so 10 reads as a label NAMING the group rather than as another value in it.
const CAPTION_SIZE: f32 = 10.0;

/// Caption for a preset group, muted and one step below the strip's own cells.
///
/// A SIBLING of the strip, never a child: `strips::strip_with_overlay` lays its interaction layer
/// over the strip's own box, so anything inserted INSIDE the strip would shift every cell out from
/// under its hit target.
///
/// `MoonLabel` rather than a hand-rolled text div: it applies the theme's mono family and runs the
/// size through `tokens.font()` itself, so the caption follows the Font slider like every other
/// MoonUI text. Pass the BASE size — a pre-scaled `design::t_*` value would be scaled twice.
///
/// The text is a literal, not `t!`: `Size`/`Sell` are on the deliberately-untranslated list
/// (`locales/README.md`), as are the neighbouring `Lev`/`SL`/`TP`. The tooltip is translated, the
/// caption is not. The account's base currency appended to `Size` is a ticker, likewise untranslated.
fn strip_caption(text: impl Into<SharedString>, p: MoonPalette) -> impl IntoElement {
    div().flex_none().child(
        MoonLabel::new(text)
            .mono(true)
            .color(p.text_muted)
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
/// Raw width of each icon-only singleton-window button.
const ICON_BTN_W: f32 = 30.0;
/// Base width of the Settings button while its label is visible.
const SETTINGS_BTN_W: f32 = 92.0;
/// Caption of the sell group — unlike `Size` it carries no unit, the cells already show percents.
const SELL_CAPTION: &str = "Sell";

/// Length ceiling for the base-currency code in the caption. The toolbar expects a short asset code;
/// this bound prevents arbitrary server text from expanding the row.
const BASE_CCY_MAX_CHARS: usize = 8;

/// Which of the row's optional LABELS fit a window of width `chrome_width`.
///
/// The row's controls do not shrink, so at some width the labels are all that is left to give. This
/// is the one place that decides which ones go, and it resolves them to the values the row renders
/// so nothing downstream can reach a different conclusion.
///
/// The thresholds nest by construction: each rung adds one optional label to the unsheddable row
/// budget. Four direct comparisons therefore decide the four-rung ladder without enumerating
/// combinations of visible labels.
///
/// Yield order, most expendable first. Every rung sheds a LABEL; no control ever leaves the row.
/// 1. **the `Sell` caption** — its strip stands against the `TP` button, which names the same
///    concept one control away;
/// 2. **the Settings button's label** — it is the only label among four otherwise identical window
///    launchers, so dropping it makes that cluster more uniform, not less legible, and the gear
///    glyph keeps its tooltip;
/// 3. **the `Size, ` noun** — six numeric presets at the head of a trading toolbar are recognisable
///    without being named;
/// 4. **the unit** — last, because it is the one fact the digits cannot carry themselves. Even
///    then the cell tooltip still spells it out.
///
/// Measured from the REAL cell widths, which depend on the preset values and the font size, rather
/// than from constant thresholds: a fixed threshold cannot model a width that varies. The budget
/// includes the TRAILING CLUSTER — comparing against the window width alone would keep labels
/// visible while the window buttons are already pushed off the right edge.
fn row_fit(
    cx: &App,
    chrome_width: f32,
    size: &strips::FittedCells,
    sell: &strips::FittedCells,
    base_ccy: Option<&str>,
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
        + ICON_BTN_W * 4.0;
    // Five 1px rules — the hairline is deliberately NOT font-scaled (see `design::vline`). Pinned
    // against the row itself by `toolbar_row_budget_counts_every_rule_it_draws` in
    // `tests/theme_contract.rs`: adding a section here without updating this count is invisible
    // until the trailing cluster clips off the edge of some narrow window.
    let rules = 5.0;
    // Row gaps: 11 between the 12 root children (both sides of the zero-width spacer included)
    // plus 5 inside sections — one in Risk, one in Exit, three between the four window buttons.
    // Count them ALL: an undercount moves every threshold, so a label stays visible after the row's
    // fixed part has already outgrown the window — and the spacer cannot shrink past zero.
    let gaps = gap * 16.0;
    let base = design::ui_value(cx, design::HEADER_PAD_X) * 2.0 + controls + rules + gaps;
    // A caption costs its own width plus the gap separating it from its strip.
    let caption_w = |text: &str| design::ui_text_width(cx, text, CAPTION_SIZE, 400.0, true) + gap;
    // The label is what turns the icon-only button into the labelled one; only the difference is
    // optional, because the button itself is not.
    let settings_label_w = fw(SETTINGS_BTN_W) - ICON_BTN_W;

    let full_caption = size_caption_text(base_ccy);
    let full = base + caption_w(&full_caption);
    let with_settings = full + settings_label_w;

    let size_caption = if chrome_width >= full {
        Some(full_caption)
    } else {
        // Only the unit survives, and only if a core told us what it is. Measured lazily: at any
        // width that fits the full caption this string is never needed, and measuring costs an
        // uncached glyph layout per character on every frame.
        base_ccy
            .map(|ccy| SharedString::from(ccy.to_string()))
            .filter(|ccy| chrome_width >= base + caption_w(ccy))
    };
    RowFit {
        size_caption,
        sell_caption: (chrome_width >= with_settings + caption_w(SELL_CAPTION))
            .then(|| SharedString::from(SELL_CAPTION)),
        settings_width: (chrome_width >= with_settings).then(|| fw(SETTINGS_BTN_W)),
    }
}

/// The optional labels the row renders at the current window width, already resolved to the values
/// it renders — see [`row_fit`]. `None` means that label does not fit and is not drawn.
struct RowFit {
    size_caption: Option<SharedString>,
    sell_caption: Option<SharedString>,
    /// Fixed width of the settings button when it carries its label; `None` renders it icon-only.
    settings_width: Option<f32>,
}

/// Caption of the order-size group together with its unit.
///
/// The unit is the active core's account base currency, not a constant: a core on a BTC account
/// sizes its orders in BTC. No core means no currency, and the caption stays a bare `Size` — an
/// invented unit would state something about an account we do not know.
///
/// The unit lives on the CAPTION rather than in all six cells: it is one and the same fact for the
/// whole group, and cell width is the row's scarcest resource. The cost is that a collapsed caption
/// takes the unit with it; the cell tooltip, reachable at any width, covers that case.
fn size_caption_text(base_ccy: Option<&str>) -> SharedString {
    match base_ccy {
        Some(ccy) => SharedString::from(format!("Size, {ccy}")),
        None => SharedString::from("Size"),
    }
}

/// The toolbar strip: an ordinary `Shell` child between the header and the dock, not a dock panel.
/// It reads the active core's trading state from `backend`; clicks write back and notify.
///
/// `chrome_width` is the window width. The row's controls are all `flex_none`, so nothing shrinks:
/// its optional labels collapse against that width by an explicit priority ([`row_fit`]), because
/// the row would otherwise push the trailing window buttons off the edge.
#[allow(clippy::too_many_arguments)]
pub fn toolbar(
    backend: &Entity<Backend>,
    group: &str,
    size_edit: Option<(CoreId, usize)>,
    size_input: &Entity<MoonInputState>,
    sell_edit: Option<(CoreId, usize)>,
    sell_input: &Entity<MoonInputState>,
    shell: &Entity<Shell>,
    metric_popup: Option<(TradeMetric, AnyElement)>,
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
    let (
        follow,
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
        base_ccy,
    ) = {
        let b = backend.read(cx);
        // The active trade core is either the sticky header selection or the core of the open Main
        // chart. Every trading control in this row reads that same core. With no core, values render
        // as dashes and interactions are ignored.
        let focus_core = b.active_trade_core(group);
        let (size_values, size_sel) = match focus_core {
            Some(core) => {
                let (values, sel) = b.manual_order_size_state(core);
                (Some(values), Some(sel))
            }
            None => (None, None),
        };
        let core_data = focus_core.and_then(|c| b.session.store().core(c));
        let cs = core_data.and_then(|d| d.client_settings.as_ref());
        // The TP button always shows its own `take_profit_main_pct`, even while an S slot is engaged;
        // selecting a slot must not replace the value displayed by TP.
        // `None` is not merely "render a dash": it also means the popup has nothing to seed
        // from, so the button must be drawn DISABLED rather than swallowing a click.
        let tp_value = cs.map(|s| format!("{}%", fmt_field2(s.take_profit_main_pct as f32)));
        // TP is lit while fixed-sell is off. Local optimistic state overrides the core snapshot so
        // an S/TP click appears immediately instead of waiting for the ClientSettings echo.
        let tp_engaged = match (focus_core, cs) {
            (Some(core), Some(s)) => !b.fixed_sell_mode_with(core, s.fixed_sell_mode),
            _ => true,
        };
        // SL is signed: `+1.00%` / `-20.00%`, avoiding `--` from manually prefixing a negative value.
        let sl_value = cs.map(|s| format!("{}%", fmt_field2_signed(s.stop_loss_pct)));
        // The toggle beside SL controls `panic_if_price_drop`; while off, the SL button is disabled.
        let sl_on = cs.map(|s| s.panic_if_price_drop).unwrap_or(false);
        // Overlay the optimistic local cache on core values for a live sell display.
        let sell_pcts = focus_core.zip(cs).map(|(core, s)| {
            let arr: [f64; 6] =
                std::array::from_fn(|i| b.fixed_sell_pct_with(core, i, s.fixed_sell_pcts[i]));
            arr
        });
        // Highlight an S slot ONLY while fixed-sell is enabled; otherwise every S slot is dim.
        let sell_slot = match (focus_core, cs) {
            (Some(core), Some(s)) => {
                b.fixed_sell_slot_with(core, s.fixed_sell_mode.then_some(s.fixed_sell_slot))
            }
            _ => None,
        };
        // Leverage is the Main chart market's per-core, per-market value from assets.
        // `seed_value` rather than `current`: it carries the same "0 means not set" rule the
        // popup's own open guard uses, so the dash and the disabled button cannot disagree with it.
        let lev_value = TradeMetric::Lev
            .seed_value(b, group)
            .map(|l| format!("×{}", l as i32));
        // With the header's MS toggle on, the core sets sell/stop levels for new orders from the
        // manual strategy's fields. Dim toolbar TP/S slots/SL because their values do not apply.
        let manual_on = focus_core
            .map(|c| b.manual_strat_state(c).0)
            .unwrap_or(false);
        // Account base currency, for the unit in the size caption. The value comes FROM THE SERVER
        // with no guaranteed shape and lands in a fixed-height single-line surface, so normalize in
        // three steps: fold hard breaks (GPUI lays out one visual line per `\n` even under nowrap,
        // and an unfolded break would paint through neighbouring content), drop empty/blank instead
        // of rendering "Size, ", and cap an anomalously long token that would stretch the caption.
        // Cloned because `core_base` borrows from the backend read guard, which does not outlive
        // this block.
        let base_ccy = focus_core
            .and_then(|c| b.session.core_base(c))
            .map(crate::display_text::flatten_lines)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(BASE_CCY_MAX_CHARS).collect::<String>());
        (
            b.follow,
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
            base_ccy,
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
    // Having a VALUE is part of it. A metric whose value is a dash has nothing for its popup to
    // seed from, and `Shell` refuses to open one — so without this term the button would look live
    // and silently swallow the click, which is worse than being visibly unavailable.
    let has_core = focus_core.is_some();
    let enabled = |metric: TradeMetric, value: &Option<String>| {
        metric.available_with(has_core, sl_on, manual_on) && value.is_some()
    };
    let lev_available = enabled(TradeMetric::Lev, &lev_value);
    let tp_available = enabled(TradeMetric::Tp, &tp_value);
    let sl_available = enabled(TradeMetric::Sl, &sl_value);
    // Rendered form of the same three: a dash wherever there is no value.
    let dash = || "—".to_string();
    let (tp_str, sl_str, lev_str) = (
        tp_value.unwrap_or_else(dash),
        sl_value.unwrap_or_else(dash),
        lev_value.unwrap_or_else(dash),
    );

    // Cells are fitted BEFORE rendering: both the strip itself and the row budget that decides the
    // labels' fate read them. One computation, one source.
    let size_cells = strips::FittedCells::fit(cx, strips::size_labels(size_values));
    let sell_cells = strips::FittedCells::fit(cx, strips::sell_labels(sell_pcts));
    let fit = row_fit(
        cx,
        chrome_width,
        &size_cells,
        &sell_cells,
        base_ccy.as_deref(),
    );

    // A section carries the gap INSIDE it; the boundary between two sections is drawn by the RULE
    // standing between them, not by a wider gap. Shared with the header — see
    // `design::chrome_section`.
    let section = || design::chrome_section(cx);

    let mut row = h_flex()
        .id("toolbar")
        .w_full()
        .h(px(design::toolbar_height(cx)))
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
        // leverage scales it, the stop and the target exit it. Same principle as the header, which
        // leads with the active core because everything else is read through it.
        .child(
            section().child(captioned_strip(
                fit.size_caption,
                p,
                size_strip(
                    &size_cells,
                    size_sel,
                    // Show the editor only when the request belongs to the toolbar's focused core.
                    size_edit
                        .filter(|(c, _)| Some(*c) == focus_core)
                        .map(|(_, i)| i),
                    size_input,
                    backend.clone(),
                    focus_core,
                    base_ccy.as_deref(),
                ),
                cx,
            )),
        )
        // §2 LEVERAGE — its own section rather than an appendix to size: a leverage edit goes TO
        // THE EXCHANGE (`session.set_leverage`, behind an explicit Apply button in the popup),
        // whereas an order size is a local preset in the config. Different blast radius, different
        // group.
        .child(design::chrome_divider(cx, p))
        .child(section().child(metric_button(
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
        )))
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
                    sl_str,
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
                // Show the S editor only when its request belongs to the toolbar's focused core.
                sell_edit
                    .filter(|(c, _)| Some(*c) == focus_core && !manual_on)
                    .map(|(_, i)| i),
                sell_input,
                backend.clone(),
                // Manual strategy mode disables all click, wheel, and double-click interaction.
                focus_core.filter(|_| !manual_on),
            );
            // Dim ONLY the sell block: manual-strategy mode stops its slots from being applied,
            // while the TP button beside it stays live.
            let sell_block = captioned_strip(fit.sell_caption, p, strip, cx);
            let sell_block = if manual_on {
                div().opacity(0.55).child(sell_block).into_any_element()
            } else {
                sell_block.into_any_element()
            };
            section()
                .child(metric_button(
                    TradeMetric::Tp,
                    tp_str,
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
    // Trailing edge: the buttons that open singleton windows. All four do the same kind of thing,
    // so they share one section behind one rule — fenced off from Live and from the trading
    // parameters, which are not launchers at all.
    row.child(div().flex_1())
        .child(design::chrome_divider(cx, p))
        .child(
            section()
                .child(open_window_button(
                    "toolbar-strategies",
                    t!("toolbar.strategies").to_string(),
                    "icons/bot.svg",
                    None,
                    backend.clone(),
                    crate::strategies::open,
                    p,
                ))
                .child(open_window_button(
                    "toolbar-screener",
                    t!("toolbar.screener").to_string(),
                    "icons/chart-pie.svg",
                    None,
                    backend.clone(),
                    crate::screener::open,
                    p,
                ))
                .child(open_window_button(
                    "toolbar-analytics",
                    t!("toolbar.analytics").to_string(),
                    "icons/layout-dashboard.svg",
                    None,
                    backend.clone(),
                    crate::analytics::open,
                    p,
                ))
                .child(open_window_button(
                    "toolbar-settings",
                    t!("shell.settings_btn").to_string(),
                    "icons/settings.svg",
                    // Labelled while the row has room for it ([`row_fit`]), icon-only once it does
                    // not. A fixed width rather than padding: `MoonButton` has pad_x = 0
                    // (FORK_BUGS), and centres its content inside the width it is given.
                    fit.settings_width,
                    backend.clone(),
                    crate::settings::open,
                    p,
                )),
        )
}

/// A toolbar button that opens a singleton window (Strategies/Screener/Settings), styled like Live
/// (Soft/ToolbarCompact). `labeled_width = None` renders the icon alone with its name as a tooltip;
/// `Some(w)` renders icon + label inside the fixed width `w`. `open` is `strategies::open` /
/// `screener::open` / `settings::open` — one shared signature, each deduplicating and focusing
/// its own window.
#[allow(clippy::too_many_arguments)]
fn open_window_button(
    id: &'static str,
    label: String,
    icon: &'static str,
    labeled_width: Option<f32>,
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
        btn.text_segment(label, p.text, 500.0)
    } else {
        btn.tooltip(label)
    };
    btn.on_click(move |_, window, cx| {
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
