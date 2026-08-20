//! The "Chart graphics" popup (the palette button beside the candlestick one) configures how the
//! chart DRAWS: the size of the closed-trade history arrows, the thickness of their entry-to-exit
//! connector, which closed TRADES appear at all, whether a closed order keeps its sell line, the
//! size of the live trade marks, and the bottom volume band.
//!
//! The last two groups used to live in Settings -> Interface, keyed to the THEME. They describe a
//! chart tab, not a colour scheme, so they moved here and became per tab like everything else in
//! this popup; `moon_core::config::theme_legacy` carries an existing user's values across.
//!
//! Like the layout and candle popups beside it, these settings are PER TAB: the target is the tab
//! strip's active tab or the detached window's panel. The tab spec persists them to `charts.json`
//! through `ChartTabSpec::chart_graphics`, and a tab without an override follows the global
//! `layout.chart_graphics` default. The ⧉ button distributes this target's settings to all
//! Add/Custom tabs and detached windows and updates that default; it includes Main only when Main is
//! the source, exactly as the candle popup's does.
//!
//! All controls are stateless: they are re-derived from the stored config on every render, which is
//! what lets the popup live in a chart host that repaints constantly.

use gpui::*;
use moon_core::config::ChartGraphicsCfg;
use moon_ui::{
    MoonCheckbox, MoonCheckboxSize, MoonPalette, MoonPopover, MoonPopoverPlacement, h_flex, v_flex,
};
use rust_i18n::t;

use super::common::{LayoutPopupHost, StackSetting, seg_row};
use crate::design;
use crate::panels::{
    popup_apply_all_button, popup_close_button, popup_group, popup_group_inset_px, popup_title,
};

/// Selectable arrow-size multipliers, inside `moon_chart::trade_marks`'s clamp range.
///
/// Steps rather than a slider: `MoonSlider` needs a state entity held on the host, and every other
/// control in these chart popups is stateless on purpose (see the module docs).
const ARROW_SCALES: [f32; 6] = [0.6, 0.8, 1.0, 1.3, 1.6, 2.0];

/// Selectable connector thicknesses, in logical px.
const CONNECTOR_PX: [f32; 4] = [1.0, 2.0, 3.0, 4.0];

/// Selectable trade-marker size multipliers.
///
/// Both `0.7` and `1.0` are steps on purpose. `0.7` is the shipped default; `1.0` was the default
/// while this value lived in `theme.toml`, so anyone who preferred the old size gets it in one
/// click rather than by hand-editing a file.
const MARKER_SCALES: [f32; 6] = [0.5, 0.7, 1.0, 1.5, 2.0, 3.0];

/// Selectable opacities for the per-TRADE volume bars. `0.34` is the shipped default.
const TRADE_VOLUME_ALPHAS: [f32; 6] = [0.0, 0.15, 0.34, 0.5, 0.75, 1.0];

/// Selectable bottom-volume band heights, as a fraction of the plot height.
///
/// The lower end sits above `moon_chart::volume_bars`'s drawable minimum, while the upper end is
/// its maximum. These are offered sizes, not the clamp, and the clamp stays that module's business
/// alone.
const CANDLE_VOLUME_HEIGHTS: [f32; 6] = [0.05, 0.10, 0.18, 0.25, 0.35, 0.45];

/// Selectable bottom-volume opacities.
///
/// Carries BOTH `0.30` and `0.22`: the first is the shipped default, the second is what the LIGHT
/// theme used to force before this value became per tab. A migrated light-mode user therefore lands
/// on an exact segment instead of the nearest one.
const CANDLE_VOLUME_ALPHAS: [f32; 6] = [0.0, 0.15, 0.22, 0.30, 0.50, 1.0];

/// Selectable bottom-volume styles, in display order.
const VOLUME_STYLES: [u8; 3] = [
    moon_core::market::candles::VOLUME_STYLE_HILLS,
    moon_core::market::candles::VOLUME_STYLE_BARS,
    moon_core::market::candles::VOLUME_STYLE_OFF,
];

/// Offered colours for the volume scale's reference lines, sRGB.
///
/// A grey ramp rather than a wheel: the lines annotate a volume band, and what matters is how far
/// they sit from the background. `110` and `170` are the two values the dark and light themes used
/// to set, so every migrated user finds their own colour on the ramp.
///
/// A stored colour that is NOT on this ramp still draws, and the row shows it as an extra trailing
/// cell — see [`swatch_row`]. This replaced a free colour wheel in Settings; `MoonColorPickerState`
/// could not be reused because it exposes no public setter, so its swatch could not be re-pointed
/// when the popup switches to another tab.
const VOLUME_SCALE_COLORS: [[u8; 3]; 6] = [
    [70, 70, 70],
    [110, 110, 110],
    [150, 150, 150],
    [170, 170, 170],
    [200, 200, 200],
    [235, 235, 235],
];

/// Popup CONTENT width in rendered pixels. `MoonPopover` adds its own padding and border outside it.
///
/// Sized on the widest row, of which there are now several: six segments at 42 units each, which
/// the three 84-unit style segments tie exactly. The checkbox labels wrap rather than widen, the
/// colour cells are sized to fit seven in that same width, and the localized ES strings are the
/// longest of the three.
pub(super) fn content_width(cx: &App) -> Pixels {
    px(6.0 * 42.0 + popup_group_inset_px(cx))
}

/// Index of the step nearest a stored value.
///
/// Nearest rather than exact: `layout.toml` is hand-editable, and a value between two steps must
/// still light one segment instead of leaving the row blank.
///
/// Args:
///     steps: Selectable values, in display order.
///     value: The stored value.
///
/// Returns:
///     Index into `steps` of the closest value; zero when the stored value is not finite.
fn nearest(steps: &[f32], value: f32) -> usize {
    if !value.is_finite() {
        return 0;
    }
    let mut best = 0usize;
    let mut best_gap = f32::INFINITY;
    for (index, step) in steps.iter().enumerate() {
        let gap = (step - value).abs();
        if gap < best_gap {
            best_gap = gap;
            best = index;
        }
    }
    best
}

/// Build one popup setting as a caption with a row of clickable colour cells below it.
///
/// Hand-built rather than taken from MoonUI, and the gap is real rather than an oversight:
/// `MoonColorPicker` is STATEFUL — it needs an `Entity<MoonColorPickerState>` held by the view, and
/// that state exposes no public setter, so it cannot be re-pointed when this popup switches to
/// another chart tab. `MoonSegmentedControl::replace_item` cannot carry the cells either: its own
/// documentation says a replaced cell keeps its width and underline but loses click handling, and a
/// colour the user cannot click is not a control. What the stack lacks is a STATELESS controlled
/// swatch strip, so this builds one from primitives and design tokens rather than duplicating an
/// existing widget.
///
/// Args:
///     id: Element identity prefix for the cells.
///     caption: Localized label drawn above them.
///     swatches: One `(sRGB, selected)` pair per cell, in display order.
///     p: Active palette, for the caption and the cell borders.
///     cx: App context, for the caption text size and cell sizing.
///     on_pick: Receives the picked cell index.
///
/// Returns:
///     The caption and its colour cells as one column.
fn swatch_row(
    id: String,
    caption: String,
    swatches: Vec<([u8; 3], bool)>,
    p: MoonPalette,
    cx: &App,
    on_pick: impl Fn(usize, &mut App) + Clone + 'static,
) -> impl IntoElement {
    // Sized so SEVEN cells still fit the popup's content width: the row grows by one when the
    // stored colour is off-ramp (see `VOLUME_SCALE_COLORS`), and that case must not overflow.
    let cell = design::ui_px(cx, 32.0);
    let mut cells = h_flex().w_full().gap(design::ui_px(cx, 4.0));
    for (index, (color, selected)) in swatches.into_iter().enumerate() {
        let on_pick = on_pick.clone();
        cells = cells.child(
            div()
                .id(SharedString::from(format!("{id}-{index}")))
                .w(cell)
                .h(design::ui_px(cx, 18.0))
                .rounded(design::ui_px(cx, 3.0))
                .bg(rgb(design::rgb_to_u32(color)))
                .border_1()
                // The selected cell is ringed in the accent the segmented rows above use, so the
                // two kinds of row read as the same control at a glance.
                .border_color(rgb(if selected { p.blue } else { p.border }))
                .cursor_pointer()
                .hover(|s| s.border_color(rgb(p.border_hover)))
                .on_click(move |_, _w, app| on_pick(index, app)),
        );
    }
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 2.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(caption),
        )
        .child(cells)
}

/// Label a 0..1 fraction as whole percent.
///
/// Needs no dictionary entry: the digits and `%` read the same in all three languages, which is
/// what keeps four rows of them out of the locale files.
fn percent_label(v: f32) -> String {
    format!("{}%", (v * 100.0).round())
}

/// Localized name of a bottom-volume style id.
///
/// An unknown id falls back to the "off" label rather than panicking: the value is a `u8` in a
/// hand-editable config, so a number outside the set is reachable by typing.
fn volume_style_label(style: u8) -> String {
    use moon_core::market::candles::{VOLUME_STYLE_BARS, VOLUME_STYLE_HILLS};
    if style == VOLUME_STYLE_HILLS {
        t!("chart.graphics.volume_style_hills").to_string()
    } else if style == VOLUME_STYLE_BARS {
        t!("chart.graphics.volume_style_bars").to_string()
    } else {
        t!("chart.graphics.volume_style_off").to_string()
    }
}

/// Edit the target's config by loading its current value, mutating it, and applying it to the tab.
fn write_cfg<T: GraphicsPopupHost>(
    entity: &Entity<T>,
    app: &mut App,
    f: impl FnOnce(&mut ChartGraphicsCfg),
) {
    entity.update(app, |this, cx| {
        let mut cfg = this.graphics_cfg(cx);
        f(&mut cfg);
        this.apply_graphics(cfg, cx);
    });
}

/// Render popup content by reading the stored values on every render for the stateless controls.
fn render_graphics_popup<T: GraphicsPopupHost>(
    id: &str,
    entity: Entity<T>,
    cfg: ChartGraphicsCfg,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    // --- Trade history frame: arrow size and connector thickness. ---
    let arrow_row = {
        let entity = entity.clone();
        let current = nearest(&ARROW_SCALES, cfg.trade_arrow_scale);
        seg_row(
            format!("{id}-arrow"),
            t!("chart.graphics.arrow_size").to_string(),
            ARROW_SCALES
                .iter()
                .enumerate()
                // Labelled as multipliers ("1x"), which needs no dictionary entry and stays
                // readable when the base sizes are retuned.
                .map(|(index, v)| (format!("{v}x"), index == current))
                .collect(),
            42.0,
            p,
            cx,
            move |ix, app| {
                if let Some(v) = ARROW_SCALES.get(ix) {
                    let v = *v;
                    write_cfg(&entity, app, |c| c.trade_arrow_scale = v);
                }
            },
        )
    };
    let connector_row = {
        let entity = entity.clone();
        let current = nearest(&CONNECTOR_PX, cfg.connector_thickness_px);
        seg_row(
            format!("{id}-connector"),
            t!("chart.graphics.connector").to_string(),
            CONNECTOR_PX
                .iter()
                .enumerate()
                .map(|(index, v)| (format!("{v}"), index == current))
                .collect(),
            34.0,
            p,
            cx,
            move |ix, app| {
                if let Some(v) = CONNECTOR_PX.get(ix) {
                    let v = *v;
                    write_cfg(&entity, app, |c| c.connector_thickness_px = v);
                }
            },
        )
    };

    // --- Which closed trades the history layer draws, plus the closed order's sell line. ---
    let real_cb = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-real")))
            .label(t!("chart.graphics.real_trades").to_string())
            .checked(cfg.show_real_trades)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| {
                let v = *ch;
                write_cfg(&entity, app, |c| c.show_real_trades = v);
            })
    };
    let emulator_cb = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-emulator")))
            .label(t!("chart.graphics.emulator_trades").to_string())
            .checked(cfg.show_emulator_trades)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| {
                let v = *ch;
                write_cfg(&entity, app, |c| c.show_emulator_trades = v);
            })
    };
    let hide_sell_cb = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-hide-closed-sell")))
            .label(t!("chart.graphics.hide_closed_sell").to_string())
            .checked(cfg.hide_closed_sell_line)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| {
                let v = *ch;
                write_cfg(&entity, app, |c| c.hide_closed_sell_line = v);
            })
    };
    let hide_sell_hint = div()
        .text_size(design::t_caption(cx))
        .text_color(rgb(p.text_muted))
        .child(t!("chart.graphics.hide_closed_sell_hint").to_string());

    // --- Trade marks: the live trade crosses and their per-trade volume bars. ---
    let marker_scale_row = {
        let entity = entity.clone();
        let current = nearest(&MARKER_SCALES, cfg.marker_scale);
        seg_row(
            format!("{id}-marker-scale"),
            t!("chart.graphics.marker_scale").to_string(),
            MARKER_SCALES
                .iter()
                .enumerate()
                .map(|(index, v)| (format!("{v}x"), index == current))
                .collect(),
            42.0,
            p,
            cx,
            move |ix, app| {
                if let Some(v) = MARKER_SCALES.get(ix) {
                    let v = *v;
                    write_cfg(&entity, app, |c| c.marker_scale = v);
                }
            },
        )
    };
    let trade_volume_alpha_row = {
        let entity = entity.clone();
        let current = nearest(&TRADE_VOLUME_ALPHAS, cfg.trade_volume_alpha);
        seg_row(
            format!("{id}-trade-volume-alpha"),
            t!("chart.graphics.trade_volume_alpha").to_string(),
            TRADE_VOLUME_ALPHAS
                .iter()
                .enumerate()
                .map(|(index, v)| (percent_label(*v), index == current))
                .collect(),
            42.0,
            p,
            cx,
            move |ix, app| {
                if let Some(v) = TRADE_VOLUME_ALPHAS.get(ix) {
                    let v = *v;
                    write_cfg(&entity, app, |c| c.trade_volume_alpha = v);
                }
            },
        )
    };

    // --- Bottom volumes: the per-CANDLE band drawn beneath the trade bars above. ---
    let volume_style_row = {
        let entity = entity.clone();
        seg_row(
            format!("{id}-volume-style"),
            t!("chart.graphics.volume_style").to_string(),
            VOLUME_STYLES
                .iter()
                // Exact equality, not `nearest`: a style is an identity, and snapping an unknown
                // id to the closest NUMBER would light a style the chart is not drawing.
                .map(|v| (volume_style_label(*v), *v == cfg.candle_volume_style))
                .collect(),
            84.0,
            p,
            cx,
            move |ix, app| {
                if let Some(v) = VOLUME_STYLES.get(ix) {
                    let v = *v;
                    write_cfg(&entity, app, |c| c.candle_volume_style = v);
                }
            },
        )
    };
    let volume_height_row = {
        let entity = entity.clone();
        let current = nearest(&CANDLE_VOLUME_HEIGHTS, cfg.candle_volume_height);
        seg_row(
            format!("{id}-volume-height"),
            t!("chart.graphics.candle_volume_height").to_string(),
            CANDLE_VOLUME_HEIGHTS
                .iter()
                .enumerate()
                .map(|(index, v)| (percent_label(*v), index == current))
                .collect(),
            42.0,
            p,
            cx,
            move |ix, app| {
                if let Some(v) = CANDLE_VOLUME_HEIGHTS.get(ix) {
                    let v = *v;
                    write_cfg(&entity, app, |c| c.candle_volume_height = v);
                }
            },
        )
    };
    let volume_alpha_row = {
        let entity = entity.clone();
        let current = nearest(&CANDLE_VOLUME_ALPHAS, cfg.candle_volume_alpha);
        seg_row(
            format!("{id}-volume-alpha"),
            t!("chart.graphics.candle_volume_alpha").to_string(),
            CANDLE_VOLUME_ALPHAS
                .iter()
                .enumerate()
                .map(|(index, v)| (percent_label(*v), index == current))
                .collect(),
            42.0,
            p,
            cx,
            move |ix, app| {
                if let Some(v) = CANDLE_VOLUME_ALPHAS.get(ix) {
                    let v = *v;
                    write_cfg(&entity, app, |c| c.candle_volume_alpha = v);
                }
            },
        )
    };
    let volume_scale_row = {
        let entity = entity.clone();
        let stored = cfg.candle_volume_scale;
        let on_ramp = VOLUME_SCALE_COLORS.contains(&stored);
        let mut swatches: Vec<([u8; 3], bool)> = VOLUME_SCALE_COLORS
            .iter()
            .map(|c| (*c, on_ramp && *c == stored))
            .collect();
        // A colour set before this row existed — the old Settings page had a full wheel — is kept
        // as a trailing cell rather than silently dropped. Without it the row would show nothing
        // selected while the chart draws that very colour.
        if !on_ramp {
            swatches.push((stored, true));
        }
        swatch_row(
            format!("{id}-volume-scale"),
            t!("chart.graphics.candle_volume_scale").to_string(),
            swatches,
            p,
            cx,
            move |ix, app| {
                // The trailing cell is the value already stored, so picking it is a no-op and
                // falls out of this bound check by itself.
                if let Some(v) = VOLUME_SCALE_COLORS.get(ix) {
                    let v = *v;
                    write_cfg(&entity, app, |c| c.candle_volume_scale = v);
                }
            },
        )
    };

    // The ⧉ "apply to all" icon mirrors the candle popup beside it: distribute THIS target's
    // settings to all non-Main tabs and windows, include Main only when it is the source, then
    // update the global default inherited by new tabs.
    let apply_all_btn = {
        let entity = entity.clone();
        popup_apply_all_button(
            SharedString::from(format!("{id}-apply-all")),
            t!("chart.apply_all_tabs_windows").to_string(),
            move |_, _w, app: &mut App| {
                entity.update(app, |this, cx| {
                    let cfg = this.graphics_cfg(cx);
                    this.apply_graphics_all(cfg, cx);
                });
            },
        )
    };

    // Chrome is MoonPopover's; see `popover_contents_do_not_paint_a_second_surface`.
    v_flex()
        .id(SharedString::from(format!("{id}-popup")))
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .child(popup_title(t!("chart.graphics.title"), p, cx))
                .child(apply_all_btn)
                .child(popup_close_button(
                    SharedString::from(format!("{id}-close")),
                    {
                        let entity = entity.clone();
                        move |_, _w, app: &mut App| {
                            entity.update(app, |this, cx| this.close_graphics_popup(cx));
                        }
                    },
                )),
        )
        .child(
            // The two trade-kind checkboxes belong HERE, beside the arrow size and the connector they
            // now share a subject with. They used to sit in the order-lines group because that is
            // what they filtered; the order-lines group keeps only the closed sell line, which
            // genuinely is about an order line.
            popup_group("frame-history", t!("chart.graphics.frame_history")).child(
                v_flex()
                    .gap(design::ui_px(cx, 6.0))
                    .child(arrow_row)
                    .child(connector_row)
                    .child(real_cb)
                    .child(emulator_cb),
            ),
        )
        .child(
            popup_group("frame-orders", t!("chart.graphics.frame_orders")).child(
                v_flex()
                    .gap(design::ui_px(cx, 6.0))
                    .child(hide_sell_cb)
                    .child(hide_sell_hint),
            ),
        )
        .child(
            popup_group("frame-trade-marks", t!("chart.graphics.frame_trade_marks")).child(
                v_flex()
                    .gap(design::ui_px(cx, 6.0))
                    .child(marker_scale_row)
                    .child(trade_volume_alpha_row),
            ),
        )
        .child(
            popup_group(
                "frame-bottom-volumes",
                t!("chart.graphics.frame_bottom_volumes"),
            )
            .child(
                v_flex()
                    .gap(design::ui_px(cx, 6.0))
                    .child(volume_style_row)
                    .child(volume_height_row)
                    .child(volume_alpha_row)
                    .child(volume_scale_row),
            ),
        )
        .into_any_element()
}

/// Host for the graphics popup in either the tab strip or a detached-window header.
///
/// The target is the strip's active tab or the window panel, resolved by the host. Applying and
/// persisting go through [`LayoutPopupHost::apply_tab_setting`]; each host implements its own
/// "apply to all", exactly as [`super::candle_popup::CandlePopupHost`] does.
pub(super) trait GraphicsPopupHost: LayoutPopupHost {
    fn graphics_popup_open(&self) -> bool;
    fn set_graphics_popup_open(&mut self, open: bool);
    /// Return the target's per-tab override, or `None` to follow the global default.
    fn graphics_override(&self, cx: &App) -> Option<ChartGraphicsCfg>;
    /// Apply settings to all non-Main tabs and windows and update the global default. Include Main
    /// only when the host's source is Main; Add, Custom, and detached sources leave it unchanged.
    fn apply_graphics_all(&mut self, cfg: ChartGraphicsCfg, cx: &mut Context<Self>);

    /// Read the target's effective settings, NORMALIZED to what the chart actually draws.
    ///
    /// The engine normalizes before it stores, so a hand-edited `layout.toml` value outside the
    /// drawable range is rendered as its clamp. Reading the raw value here would light a segment
    /// the chart is not using — and, because a write starts from this value, would also persist
    /// the out-of-range number back untouched.
    fn graphics_cfg(&self, cx: &App) -> ChartGraphicsCfg {
        let effective = self
            .graphics_override(cx)
            .unwrap_or(self.backend().read(cx).layout.chart_graphics);
        moon_chart::normalize_chart_graphics(effective)
    }

    /// Apply settings to the target stacks and persist them in the tab spec.
    fn apply_graphics(&mut self, cfg: ChartGraphicsCfg, cx: &mut Context<Self>) {
        self.apply_tab_setting(StackSetting::Graphics(cfg), cx);
    }

    /// Close the popup.
    ///
    /// The already-closed guard is load-bearing: clicking the button while the popup is open makes
    /// `Popover` fire `on_open_change(false)` twice (outside-click handler, then the trigger
    /// re-arming).
    fn close_graphics_popup(&mut self, cx: &mut Context<Self>) {
        if !self.graphics_popup_open() {
            return;
        }
        self.set_graphics_popup_open(false);
        cx.notify();
    }
}

/// Build the chart-graphics popup: a `MoonPopover` anchored to the button that opens it.
///
/// The content is built ONLY while open — `MoonPopover` takes it eagerly, and this sits in a chart
/// host that repaints constantly.
///
/// Args:
///     this: The popup's host.
///     id_prefix: Per-host element identity prefix.
///     trigger: The button the popover anchors to.
///     cx: Host context.
///
/// Returns:
///     The trigger with its anchored popover.
pub(super) fn graphics_popup_host<T: GraphicsPopupHost>(
    this: &T,
    id_prefix: &'static str,
    trigger: impl IntoElement,
    cx: &mut Context<T>,
) -> MoonPopover {
    let open_entity = cx.entity();
    let mut popover = MoonPopover::new(SharedString::from(format!("{id_prefix}-popover")))
        // Anchored bottom-right of the button, as the candle popup beside it is: growing left
        // keeps the popup inside the window rather than running off its right edge.
        .placement(MoonPopoverPlacement::BottomEnd)
        .content_width(f32::from(content_width(cx)))
        .close_on_content_click(false)
        .open(this.graphics_popup_open())
        .on_open_change(move |open, _window, app| {
            open_entity.update(app, |this, cx| {
                this.set_graphics_popup_open(open);
                cx.notify();
            });
        })
        .trigger(trigger);
    if !this.graphics_popup_open() {
        return popover;
    }
    let p = MoonPalette::active(cx);
    let cfg = this.graphics_cfg(cx);
    let entity = cx.entity();
    popover = popover.content(render_graphics_popup(id_prefix, entity, cfg, p, cx));
    popover
}
