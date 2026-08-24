//! Chart-tab layout popup with mode (Fit or Scroll) and a size field ONLY for the active
//! mode. Settings are per-tab. Rendering is shared by the main-window tab strip and detached-window
//! header; the caller provides handlers that apply to the correct stack and persist the result.
//!
//! Semantics: `Fit=0` stretches slots to share the window; `Fit>=20` selects COMPRESS with a fixed
//! size and no scrolling; Scroll uses a fixed slot size with scrolling. Nonzero sizes are limited
//! to `[MIN_H, MAX_H]`; zero remains valid only for Fit stretch.

use gpui::*;
use moon_ui::{
    MoonAccent, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize,
    MoonInput, MoonInputState, MoonPalette, MoonSegmentItem, MoonSegmentedControl, h_flex, v_flex,
};
use rust_i18n::t;

use crate::design;
use crate::panels::{
    popup_apply_all_button, popup_close_button, popup_group, popup_group_inset_px, popup_title,
};
use crate::persistence::chart_persist::{
    ChartBtnPos, PriceAxisPos, StackLayoutMode, StackOrientation,
};

/// Mode order in the popup's two-position segmented control.
pub(super) const POPUP_MODES: [StackLayoutMode; 2] =
    [StackLayoutMode::Fit, StackLayoutMode::Scroll];

/// Action-button positions: dash hides, L is left, C is center, and R is right.
const BTN_POSITIONS: [ChartBtnPos; 4] = [
    ChartBtnPos::Hide,
    ChartBtnPos::Left,
    ChartBtnPos::Center,
    ChartBtnPos::Right,
];

fn pos_label(p: ChartBtnPos) -> &'static str {
    match p {
        ChartBtnPos::Hide => "—",
        ChartBtnPos::Left => "L",
        ChartBtnPos::Center => "C",
        ChartBtnPos::Right => "R",
    }
}

/// Build an action-button position row with a left caption and `[dash L C R]` segmented control.
fn pos_selector_row(
    id: String,
    caption: &str,
    current: ChartBtnPos,
    p: MoonPalette,
    cx: &App,
    on_pick: impl Fn(ChartBtnPos, &mut App) + 'static,
) -> impl IntoElement {
    let sel = BTN_POSITIONS
        .iter()
        .position(|x| *x == current)
        .unwrap_or(3);
    let items: Vec<MoonSegmentItem> = BTN_POSITIONS
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let mut it = MoonSegmentItem::new("", pos_label(*x)).width(30.0);
            if i == sel {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new(id)
        .accent(MoonAccent::Blue)
        .items(items)
        .on_click(move |ix, _, _, cx| {
            if let Some(x) = BTN_POSITIONS.get(ix) {
                on_pick(*x, cx);
            }
        })
        .render();
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .flex_1()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(caption.to_string()),
        )
        .child(seg)
}

/// Price-axis positions: dash hides, L is left, and R is right beyond the order book.
const AXIS_POSITIONS: [PriceAxisPos; 3] =
    [PriceAxisPos::Hide, PriceAxisPos::Left, PriceAxisPos::Right];

fn axis_label(p: PriceAxisPos) -> &'static str {
    match p {
        PriceAxisPos::Hide => "—",
        PriceAxisPos::Left => "L",
        PriceAxisPos::Right => "R",
    }
}

/// Build a price-axis position row with a left caption and `[dash L R]` segmented control.
fn axis_selector_row(
    id: String,
    caption: String,
    current: PriceAxisPos,
    p: MoonPalette,
    cx: &App,
    on_pick: impl Fn(PriceAxisPos, &mut App) + 'static,
) -> impl IntoElement {
    let sel = AXIS_POSITIONS
        .iter()
        .position(|x| *x == current)
        .unwrap_or(1);
    let items: Vec<MoonSegmentItem> = AXIS_POSITIONS
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let mut it = MoonSegmentItem::new("", axis_label(*x)).width(30.0);
            if i == sel {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new(id)
        .accent(MoonAccent::Blue)
        .items(items)
        .on_click(move |ix, _, _, cx| {
            if let Some(x) = AXIS_POSITIONS.get(ix) {
                on_pick(*x, cx);
            }
        })
        .render();
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .flex_1()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(caption),
        )
        .child(seg)
}

/// Slot-size bounds in pixels; values below MIN or above MAX are invalid except Fit zero for stretch.
pub(super) const MIN_H: u16 = 20;
pub(super) const MAX_H: u16 = 4000;

/// Upper bound on the detect cap. Zero remains valid and means uncapped; the ceiling is here to
/// keep a stray keystroke from asking for a stack of ten thousand charts, each of which is a real
/// GPU canvas.
pub(super) const MAX_CHARTS_MAX: u16 = 999;

/// The arrival/detect controls, or `None` on a tab that draws no arrival flash at all.
///
/// Bundled rather than passed as four more parameters: this function already carries fifteen
/// callbacks, and these belong to one frame that appears or stays away as a unit.
pub(super) struct DetectFlow<'a> {
    /// The cap controls, or `None` on a tab detects never reach. The flash is NOT part of this: it
    /// fires on any arrival, a hand-picked coin included, so a tab can want it off while having no
    /// detect feed to cap.
    pub cap: Option<DetectCap<'a>>,
    /// Whether an arriving chart flashes its border. The checkbox states the negative — "do not
    /// flash" is the thing a reader goes looking for — and inverts on the way in and out.
    pub flash: bool,
    pub on_toggle_flash: DetectFlowToggle,
}

/// The detect cap and what happens at it, shown only where detects arrive.
pub(super) struct DetectCap<'a> {
    /// Cap field state. Committed by the host on the way out, like the two size fields.
    pub max_input: &'a Entity<MoonInputState>,
    /// Whether a detect at the cap replaces the stalest chart instead of going unshown.
    pub evict: bool,
    pub on_toggle_evict: DetectFlowToggle,
}

/// One detect-flow checkbox handler, named so the fields above stay readable.
type DetectFlowToggle = Box<dyn Fn(bool, &mut App)>;

/// Popup CONTENT width in rendered pixels: the two 110-unit FIT/SCROLL segments plus the group
/// frame around them. `MoonPopover` adds its own padding and border outside this.
pub(super) fn content_width(cx: &App) -> Pixels {
    px(2.0 * 110.0 + 20.0 + popup_group_inset_px(cx))
}

fn mode_label(m: StackLayoutMode) -> &'static str {
    match m {
        StackLayoutMode::Fit => "FIT",
        StackLayoutMode::Scroll => "SCROLL",
    }
}

/// Render the compact layout settings panel, showing a size field ONLY for the current mode.
///
/// `height_fit_input` and `height_scroll_input` are separate fields whose Blur/Enter subscription
/// belongs to the caller. `on_pick_mode` runs on mode selection. `MoonPopover` positions the panel.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_layout_popup<F, G, H, I, J, K, L, M, N, O, P2, Q2, R2, S2>(
    id: &str,
    current: StackLayoutMode,
    orientation: StackOrientation,
    rename_input: Option<&Entity<MoonInputState>>,
    height_fit_input: &Entity<MoonInputState>,
    height_scroll_input: &Entity<MoonInputState>,
    orderbook_enabled: bool,
    liquidations_enabled: bool,
    show_zone: bool,
    auto_pin: bool,
    cancel_buy_pos: ChartBtnPos,
    panic_sell_pos: ChartBtnPos,
    price_axis_pos: PriceAxisPos,
    time_axis_visible: bool,
    line_labels: bool,
    cursor_labels: bool,
    detect_flow: Option<DetectFlow<'_>>,
    p: MoonPalette,
    cx: &App,
    on_pick_mode: F,
    apply_all_label: String,
    on_apply_all: G,
    on_toggle_orderbook: H,
    on_toggle_liquidations: R2,
    on_toggle_show_zone: I,
    on_toggle_auto_pin: J,
    on_toggle_orientation: K,
    on_pick_cancel_pos: L,
    on_pick_panic_pos: M,
    on_pick_price_axis: N,
    on_toggle_time_axis: O,
    on_toggle_line_labels: P2,
    on_toggle_cursor_labels: Q2,
    on_close: S2,
) -> AnyElement
where
    S2: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    F: Fn(StackLayoutMode, &mut App) + 'static,
    G: Fn(&mut App) + 'static,
    H: Fn(bool, &mut App) + 'static,
    I: Fn(bool, &mut App) + 'static,
    J: Fn(bool, &mut App) + 'static,
    K: Fn(&mut App) + 'static,
    L: Fn(ChartBtnPos, &mut App) + 'static,
    M: Fn(ChartBtnPos, &mut App) + 'static,
    N: Fn(PriceAxisPos, &mut App) + 'static,
    O: Fn(bool, &mut App) + 'static,
    P2: Fn(bool, &mut App) + 'static,
    Q2: Fn(bool, &mut App) + 'static,
    R2: Fn(bool, &mut App) + 'static,
{
    let horizontal = orientation.is_horizontal();
    let sel = POPUP_MODES.iter().position(|m| *m == current).unwrap_or(0);
    let items: Vec<MoonSegmentItem> = POPUP_MODES
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut it = MoonSegmentItem::new("", mode_label(*m)).width(110.0);
            if i == sel {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new(format!("{id}-mode"))
        .accent(MoonAccent::Blue)
        .items(items)
        .on_click(move |ix, _, _, cx| {
            if let Some(m) = POPUP_MODES.get(ix) {
                on_pick_mode(*m, cx);
            }
        })
        .render();

    // Show the field and note only for the active mode. In horizontal orientation the value is the
    // slot WIDTH, so labels and hints use the `*width*` keys with the same `20..4000` range.
    let (input, label, hint) = match (current, horizontal) {
        (StackLayoutMode::Fit, false) => (
            height_fit_input,
            t!("chart.layout.height_fit").to_string(),
            t!("chart.layout.height_fit_hint").to_string(),
        ),
        (StackLayoutMode::Fit, true) => (
            height_fit_input,
            t!("chart.layout.width_fit").to_string(),
            t!("chart.layout.width_fit_hint").to_string(),
        ),
        (StackLayoutMode::Scroll, false) => (
            height_scroll_input,
            t!("chart.layout.height_scroll").to_string(),
            t!("chart.layout.height_scroll_hint").to_string(),
        ),
        (StackLayoutMode::Scroll, true) => (
            height_scroll_input,
            t!("chart.layout.width_scroll").to_string(),
            t!("chart.layout.width_scroll_hint").to_string(),
        ),
    };
    // Orientation-dependent "Height X  [field]  px" or "Width X  [field]  px" row.
    let height_line = h_flex()
        .gap(design::ui_px(cx, 6.0))
        .items_center()
        .child(div().text_color(rgb(p.text)).child(label))
        .child(
            div().w(px(64.0)).child(
                MoonInput::new(SharedString::from(format!("{id}-input")))
                    .state(input)
                    .small(),
            ),
        )
        .child(div().text_color(rgb(p.text_muted)).child("px"));
    // Note below the field, split into multiple lines on `\n`.
    let hint_block = v_flex().children(hint.split('\n').map(|line| {
        div()
            .text_size(design::t_caption(cx))
            .text_color(rgb(p.text_muted))
            .child(line.to_string())
    }));

    // "Order book" toggles the order book on this tab's charts.
    let orderbook_cb = MoonCheckbox::new(SharedString::from(format!("{id}-orderbook")))
        .label(t!("chart.layout.orderbook").to_string())
        .checked(orderbook_enabled)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_orderbook(*ch, app));

    // "Liquidations" toggles liquidation-trade crosses on this tab's charts.
    let liquidations_cb = MoonCheckbox::new(SharedString::from(format!("{id}-liquidations")))
        .label(t!("chart.layout.liquidations").to_string())
        .checked(liquidations_enabled)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_liquidations(*ch, app));

    // "Show control zone" toggles the dim order-zone fill while the order book is hidden.
    let show_zone_cb = MoonCheckbox::new(SharedString::from(format!("{id}-show-zone")))
        .label(t!("chart.layout.show_zone").to_string())
        .checked(show_zone)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_show_zone(*ch, app));

    // "Auto-pin on order" pins a chart when placing a long or short order.
    let auto_pin_cb = MoonCheckbox::new(SharedString::from(format!("{id}-auto-pin")))
        .label(t!("chart.layout.auto_pin").to_string())
        .checked(auto_pin)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_auto_pin(*ch, app));

    // "Time axis" toggles bottom time labels on this tab's charts.
    let time_axis_cb = MoonCheckbox::new(SharedString::from(format!("{id}-time-axis")))
        .label(t!("chart.layout.time_axis").to_string())
        .checked(time_axis_visible)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_time_axis(*ch, app));

    // "Line labels" toggles values beside order lines, including size, percentage, and stop.
    let line_labels_cb = MoonCheckbox::new(SharedString::from(format!("{id}-line-labels")))
        .label(t!("chart.layout.line_labels").to_string())
        .checked(line_labels)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_line_labels(*ch, app));

    // "Crosshair label" toggles the cursor readout for time, price, percentage, volume, and size.
    let cursor_labels_cb = MoonCheckbox::new(SharedString::from(format!("{id}-cursor-labels")))
        .label(t!("chart.layout.cursor_labels").to_string())
        .checked(cursor_labels)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_cursor_labels(*ch, app));

    // "Detect flow" frame: how many charts detects may open on this tab, what a detect does once
    // that number is reached, and whether an arrival announces itself. Built only for tabs detects
    // actually reach; elsewhere the whole frame stays away rather than showing dead controls.
    let detect_group = detect_flow.map(|flow| {
        let cap_rows = flow.cap.map(|cap| {
            let cap_line = h_flex()
                .gap(design::ui_px(cx, 6.0))
                .items_center()
                .child(
                    div()
                        .text_color(rgb(p.text))
                        .child(t!("chart.layout.max_charts").to_string()),
                )
                .child(
                    div().w(px(64.0)).child(
                        MoonInput::new(SharedString::from(format!("{id}-max-charts")))
                            .state(cap.max_input)
                            .small(),
                    ),
                );
            let cap_hint = div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("chart.layout.max_charts_hint").to_string());
            let evict_cb = MoonCheckbox::new(SharedString::from(format!("{id}-max-charts-evict")))
                .label(t!("chart.layout.max_charts_evict").to_string())
                .checked(cap.evict)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |ch: &bool, _w, app| (cap.on_toggle_evict)(*ch, app));
            v_flex()
                .gap(design::ui_px(cx, 6.0))
                .child(cap_line)
                .child(cap_hint)
                .child(evict_cb)
        });
        // Stated as the negative: ticked means "do not flash", so the stored value is the inverse
        // of the box on both directions.
        let flash_cb = MoonCheckbox::new(SharedString::from(format!("{id}-no-arrival-flash")))
            .label(t!("chart.layout.no_arrival_flash").to_string())
            .checked(!flow.flash)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| (flow.on_toggle_flash)(!*ch, app));
        popup_group("frame-detect-flow", t!("chart.layout.frame_detect_flow")).child(
            v_flex()
                .gap(design::ui_px(cx, 6.0))
                .children(cap_rows)
                .child(flash_cb),
        )
    });

    // Position selectors for Cancel Buy and Panic Sell in the chart zone (dash, L, C, R). Their
    // names are Moonbot brand terms and deliberately remain untranslated.
    let cancel_pos_row = pos_selector_row(
        format!("{id}-cancelbuy-pos"),
        "Cancel Buy",
        cancel_buy_pos,
        p,
        cx,
        on_pick_cancel_pos,
    );
    let panic_pos_row = pos_selector_row(
        format!("{id}-panicsell-pos"),
        "Panic Sell",
        panic_sell_pos,
        p,
        cx,
        on_pick_panic_pos,
    );
    // Price-axis selector (dash, L, R): hidden, left, or right beyond the order book.
    let price_axis_row = axis_selector_row(
        format!("{id}-price-axis-pos"),
        t!("chart.layout.price_axis").to_string(),
        price_axis_pos,
        p,
        cx,
        on_pick_price_axis,
    );

    // Stack-orientation toggle beside "apply to all": ↕ is a vertical stack and ↔ is horizontal
    // columns. Clicking rebuilds the active tab's current presentation.
    let orientation_btn = MoonButton::new(SharedString::from(format!("{id}-orientation")))
        .label(if horizontal { "↔" } else { "↕" })
        .tooltip(t!("chart.layout.orientation_tip").to_string())
        .size(MoonButtonSize::Micro)
        .variant(if horizontal {
            MoonButtonVariant::Blue
        } else {
            MoonButtonVariant::Ghost
        })
        .selected(horizontal)
        .on_click(move |_, _w, app| on_toggle_orientation(app))
        .render();

    // Symbol-only "apply to all" icon with tooltip at the right of the header row. The scope text
    // distinguishes all windows from charts only, which is why the label is passed in rather than
    // read from one key like the candle and graphics popups do.
    let apply_all_btn = popup_apply_all_button(
        SharedString::from(format!("{id}-apply-all")),
        apply_all_label,
        move |_, _w, app: &mut App| on_apply_all(app),
    );

    // Name field only for custom tabs (`rename_input = Some`). The caller owns the input subscription
    // that commits on Blur or Enter.
    let rename_row = rename_input.map(|input| {
        h_flex()
            .gap(design::ui_px(cx, 6.0))
            .items_center()
            .child(
                div()
                    .text_color(rgb(p.text_muted))
                    .child(t!("chart.tab.rename").to_string()),
            )
            .child(
                div().flex_1().child(
                    MoonInput::new(SharedString::from(format!("{id}-name")))
                        .state(input)
                        .small(),
                ),
            )
    });

    // Chrome is MoonPopover's; see `popover_contents_do_not_paint_a_second_surface`. The caller
    // declares the width through `content_width`; height is content-driven, which avoids manual
    // height sums and empty space below.
    v_flex()
        .id(SharedString::from(format!("{id}-popup")))
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .child(
            // Left-aligned title with the actions pinned to the popup's right edge.
            h_flex()
                .w_full()
                .items_center()
                .child(popup_title(t!("chart.layout.title"), p, cx))
                .child(orientation_btn)
                .child(apply_all_btn)
                .child(popup_close_button(
                    SharedString::from(format!("{id}-close")),
                    on_close,
                )),
        )
        .children(rename_row)
        // "View" frame: FIT/SCROLL mode, active-mode size field, and its description.
        .child(
            // Group ids are `&'static str`: they only need to be unique among their siblings, and
            // the enclosing root already carries the per-host prefix.
            popup_group("frame-view", t!("chart.layout.frame_view")).child(
                v_flex()
                    .gap(design::ui_px(cx, 6.0))
                    .child(seg)
                    .child(height_line)
                    .child(hint_block),
            ),
        )
        // "Display" frame: order book, liquidations, control zone, time axis, line labels, and
        // crosshair labels.
        .child(
            popup_group("frame-display", t!("chart.layout.frame_display")).child(
                v_flex()
                    .gap(design::ui_px(cx, 6.0))
                    .child(orderbook_cb)
                    .child(liquidations_cb)
                    .child(show_zone_cb)
                    .child(time_axis_cb)
                    .child(line_labels_cb)
                    .child(cursor_labels_cb),
            ),
        )
        .children(detect_group)
        // Remaining controls: auto-pin, button positions, and price axis.
        .child(auto_pin_cb)
        .child(cancel_pos_row)
        .child(panic_pos_row)
        .child(price_axis_row)
        .into_any_element()
}

/// Clamp an entered slot size: Fit permits zero for stretch, otherwise `[MIN_H, MAX_H]`; Scroll
/// always uses `[MIN_H, MAX_H]`.
pub(super) fn clamp_height(mode: StackLayoutMode, raw: u16) -> u16 {
    match mode {
        StackLayoutMode::Fit if raw == 0 => 0,
        _ => raw.clamp(MIN_H, MAX_H),
    }
}
