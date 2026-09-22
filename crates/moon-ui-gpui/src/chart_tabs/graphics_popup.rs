//! The "Chart graphics" popup (the palette button beside the candlestick one) configures what the
//! chart draws OVER the candles right now: the lines — the two core price lines, a live order's
//! repricing trail, the MoonShot corridor — and the live trade marks, their per-trade volume bars
//! and the liquidation crosses among them.
//!
//! It used to hold the closed-trade history and the bottom volume band as well, four frames in
//! two columns. Those are subjects of their own and moved to popups of their own: the history to
//! [`super::history_popup`], the band to [`super::volumes_popup`] beside the horizontal volumes it
//! shares its colours with. What stayed is what a reader looking at the LIVE picture adjusts.
//!
//! Like the layout and candle popups beside it, these settings are PER TAB: the target is the tab
//! strip's active tab or the detached window's panel. The tab spec persists them to `charts.json`
//! through `ChartTabSpec::chart_graphics`, and a tab without an override follows the default of its
//! KIND of tab (`chart_tabs::apply_all`). The ⧉ button opens the row that names which kinds a press
//! addresses and stores these settings as their default. Every switch here is a field of
//! [`ChartGraphicsCfg`] — the price lines and the corridor moved in from `CandleViewCfg`, the
//! liquidations from the tab spec — because the press copies the struct whole, and a switch shown
//! here but stored elsewhere would either not travel with it or drag its own struct along.
//!
//! Controls read stored config on every render and are stateless on purpose: `MoonSlider` needs a
//! state entity held on the host, and every other control in these popups reads its value straight
//! from the config, so the sizes are offered as steps.

use gpui::*;
use moon_core::config::ChartGraphicsCfg;
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonVariant, MoonCheckbox, MoonPalette, MoonPopover,
    MoonPopoverPlacement, MoonSize, h_flex, v_flex,
};
use rust_i18n::t;

use super::common::{LayoutPopupHost, StackSetting, seg_row};
use super::popup_slot::ChartPopup;
use crate::design;
use crate::panels::{
    popup_apply_all_button, popup_close_button, popup_group, popup_group_inset_px, popup_title,
};

/// Selectable trade-marker size multipliers.
///
/// Both `0.7` and `1.0` are steps on purpose. `0.7` is the shipped default; `1.0` was the default
/// while this value lived in `theme.toml`, so anyone who preferred the old size gets it in one
/// click rather than by hand-editing a file.
const MARKER_SCALES: [f32; 6] = [0.5, 0.7, 1.0, 1.5, 2.0, 3.0];

/// Selectable opacities for the per-TRADE volume bars. `0.34` is the shipped default.
const TRADE_VOLUME_ALPHAS: [f32; 6] = [0.0, 0.15, 0.34, 0.5, 0.75, 1.0];

/// Row width in rendered pixels, and the segment width of its six-step rows.
///
/// Kept at the width the two-column popup's rows had, so the checkbox labels wrap where they did;
/// the localized ES strings are the longest of the three.
pub(crate) const ROW_W: f32 = 7.0 * 42.0;
const SEG_W6: f32 = ROW_W / 6.0;

/// Popup CONTENT width in rendered pixels. `MoonPopover` adds its own padding and border outside it.
pub(super) fn content_width(cx: &App) -> Pixels {
    px(ROW_W + popup_group_inset_px(cx))
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
pub(crate) fn nearest(steps: &[f32], value: f32) -> usize {
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

/// Label a 0..1 fraction as whole percent.
///
/// Needs no dictionary entry: the digits and `%` read the same in all three languages, which is
/// what keeps rows of them out of the locale files.
pub(super) fn percent_label(v: f32) -> String {
    format!("{}%", (v * 100.0).round())
}

/// Edit the target's config by loading its current value, mutating it, and applying it to the tab.
///
/// Shared by the three popups that edit [`ChartGraphicsCfg`]: this one, the history and the
/// volumes. Each starts from the target's NORMALIZED value, so a hand-edited out-of-range number is
/// not persisted back untouched by an unrelated click.
pub(super) fn write_cfg<T: GraphicsPopupHost>(
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

/// Build one checkbox bound to a single [`ChartGraphicsCfg`] flag.
///
/// Args:
///     entity: Popup host, updated on toggle.
///     id: Per-host element identity prefix.
///     suffix: Element id suffix, unique within this popup.
///     label_key: Locale key for the label.
///     checked: Current value, read fresh on every render.
///     set: Writes the new value into the target's config.
///
/// Returns:
///     The checkbox.
pub(super) fn flag_cb<T: GraphicsPopupHost>(
    entity: &Entity<T>,
    id: &str,
    suffix: &str,
    label_key: &str,
    checked: bool,
    set: fn(&mut ChartGraphicsCfg, bool),
) -> MoonCheckbox {
    let entity = entity.clone();
    MoonCheckbox::new(SharedString::from(format!("{id}-{suffix}")))
        .label(t!(label_key).to_string())
        .checked(checked)
        .on_change(move |ch: &bool, _w, app| {
            let v = *ch;
            write_cfg(&entity, app, |c| set(c, v));
        })
}

/// Build the Auto Overview toggle that widens closed-trade history to every same-exchange core.
///
/// Host-generic so the docked strip and a detached window share one button, the way
/// [`flag_cb`] shares one checkbox. The element id stays per host: the two toolbars are on
/// screen together and a repeated id is a real GPUI collision.
///
/// Args:
///     entity: Popup host whose graphics config the click writes.
///     id: Element id, unique to this host.
///     active: Whether the stored flag is currently on.
///
/// Returns:
///     The icon-only button, not yet rendered.
pub(super) fn all_cores_toggle_button<T: GraphicsPopupHost>(
    entity: &Entity<T>,
    id: &str,
    active: bool,
) -> MoonButton {
    let entity = entity.clone();
    MoonButton::new(SharedString::from(id.to_string()))
        .leading_icon(MoonButtonIconSlot::new("icons/network.svg"))
        .tooltip(t!("chart.history.all_cores.tip").to_string())
        .size(MoonSize::Xs)
        .variant(if active {
            MoonButtonVariant::Blue
        } else {
            MoonButtonVariant::Ghost
        })
        .selected(active)
        .on_click(move |_, _w, app| {
            write_cfg(&entity, app, |c| {
                c.history_all_cores = !c.history_all_cores;
            });
        })
}

/// Render popup content by reading the stored values on every render for the stateless controls.
fn render_graphics_popup<T: GraphicsPopupHost>(
    id: &str,
    entity: Entity<T>,
    cfg: ChartGraphicsCfg,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    // --- Lines frame: the two core price lines, the live order trail, the MoonShot corridor. ---
    // Each price line has a toggle of its own: the orange LastPrice and the blue MarkPrice. A
    // market whose provider reports no mark price draws none regardless of the flag.
    let last_line_cb = flag_cb(
        &entity,
        id,
        "last-price-line",
        "chart.graphics.last_price_line",
        cfg.last_price_line,
        |c, v| c.last_price_line = v,
    );
    let mark_line_cb = flag_cb(
        &entity,
        id,
        "mark-price-line",
        "chart.graphics.mark_price_line",
        cfg.mark_price_line,
        |c, v| c.mark_price_line = v,
    );
    let hide_move_cb = flag_cb(
        &entity,
        id,
        "hide-move-history",
        "chart.graphics.hide_move_history",
        cfg.hide_order_move_history,
        |c, v| c.hide_order_move_history = v,
    );
    // The MoonShot order's own corridor fill, NOT the layout popup's "zone" (that one shades the
    // trading control strip). It spans the full pane width, so it is the one order area worth a
    // switch of its own.
    let moonshot_cb = flag_cb(
        &entity,
        id,
        "moonshot-zone",
        "chart.graphics.moonshot_zone",
        cfg.moonshot_zone,
        |c, v| c.moonshot_zone = v,
    );

    // --- Trade marks: the live trade crosses, their per-trade volume bars, and the liquidation
    // crosses appended to the same ring. ---
    let marker_scale_row = {
        let entity = entity.clone();
        let current = nearest(&MARKER_SCALES, cfg.marker_scale);
        seg_row(
            format!("{id}-marker-scale"),
            t!("chart.graphics.marker_scale").to_string(),
            MARKER_SCALES
                .iter()
                .enumerate()
                // Labelled as multipliers ("1x"), which needs no dictionary entry and stays
                // readable when the base sizes are retuned.
                .map(|(index, v)| (format!("{v}x"), index == current))
                .collect(),
            SEG_W6,
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
    // What the row governs is easy to miss on a busy chart — thin bars along the plot's floor, one
    // per cross in the trade zone (`crosses.hlsl`, `volume_vertex`) — so a caption under it says
    // where to look.
    let trade_volume_alpha_hint = div()
        .text_size(design::t_caption(cx))
        .text_color(rgb(p.text_muted))
        .child(t!("chart.graphics.trade_volume_alpha_hint").to_string());
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
            SEG_W6,
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
    let liquidations_cb = flag_cb(
        &entity,
        id,
        "liquidations",
        "chart.graphics.liquidations",
        cfg.liquidations,
        |c, v| c.liquidations = v,
    );

    // The ⧉ "apply to all" icon mirrors the candle popup beside it: distribute THIS target's
    // settings to all non-Main tabs and windows, include Main only when it is the source, then
    // update the global default inherited by new tabs.
    let apply_all_btn = {
        let entity = entity.clone();
        popup_apply_all_button(
            SharedString::from(format!("{id}-apply-all")),
            t!("chart.apply_all_tabs_windows").to_string(),
            move |_, _w, app: &mut App| {
                entity.update(app, |this, cx| this.arm_apply_press(cx));
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
            // Group ids are `&'static str`: they only need to be unique among their siblings, and
            // the enclosing root already carries the per-host prefix.
            popup_group("frame-lines", t!("chart.graphics.frame_lines")).child(
                v_flex()
                    .w(px(ROW_W))
                    .gap(design::ui_px(cx, 6.0))
                    .child(last_line_cb)
                    .child(mark_line_cb)
                    .child(hide_move_cb)
                    .child(moonshot_cb),
            ),
        )
        .child(
            popup_group("frame-trade-marks", t!("chart.graphics.frame_trade_marks")).child(
                v_flex()
                    .w(px(ROW_W))
                    .gap(design::ui_px(cx, 6.0))
                    .child(marker_scale_row)
                    .child(trade_volume_alpha_row)
                    .child(trade_volume_alpha_hint)
                    .child(liquidations_cb),
            ),
        )
        .into_any_element()
}

/// Host for the graphics popup in either the tab strip or a detached-window header.
///
/// The target is the strip's active tab or the window panel, resolved by the host. Applying and
/// persisting go through [`LayoutPopupHost::apply_tab_setting`]; each host implements its own
/// "apply to all", exactly as [`super::candle_popup::CandlePopupHost`] does. The history and
/// volumes popups edit the same struct and ride on this trait; see their modules.
pub(super) trait GraphicsPopupHost: LayoutPopupHost {
    /// Return the target's per-tab override, or `None` to follow the global default.
    fn graphics_override(&self, cx: &App) -> Option<ChartGraphicsCfg>;

    /// Read the target's effective settings, NORMALIZED to what the chart actually draws.
    ///
    /// The engine normalizes before it stores, so a hand-edited `layout.toml` value outside the
    /// drawable range is rendered as its clamp. Reading the raw value here would light a segment
    /// the chart is not using — and, because a write starts from this value, would also persist
    /// the out-of-range number back untouched.
    fn graphics_cfg(&self, cx: &App) -> ChartGraphicsCfg {
        // By KIND: a write starts from this value, so the Main default read here would be persisted
        // as a torn-off window's own settings by its very first edit.
        let kind = self.source_kind(cx);
        let effective = self
            .graphics_override(cx)
            .unwrap_or_else(|| self.backend().read(cx).layout.chart_graphics_for(kind));
        moon_chart::normalize_chart_graphics(effective)
    }

    /// Apply settings to the target stacks and persist them in the tab spec.
    fn apply_graphics(&mut self, cfg: ChartGraphicsCfg, cx: &mut Context<Self>) {
        self.apply_tab_setting(StackSetting::Graphics(cfg), cx);
    }

    /// Close the popup.
    ///
    /// Ownership is checked by the slot, which is load-bearing: clicking the button while the popup
    /// is open makes `Popover` fire `on_open_change(false)` twice (outside-click handler, then the
    /// trigger re-arming).
    fn close_graphics_popup(&mut self, cx: &mut Context<Self>) {
        self.close_chart_popup(ChartPopup::Graphics, cx);
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
        .open(this.popup_shows(ChartPopup::Graphics))
        .on_open_change(move |open, _window, app| {
            open_entity.update(app, |this, cx| {
                this.report_chart_popup(ChartPopup::Graphics, open, cx)
            });
        })
        .trigger(trigger);
    if !this.popup_shows(ChartPopup::Graphics) {
        return popover;
    }
    let p = MoonPalette::active(cx);
    let cfg = this.graphics_cfg(cx);
    let entity = cx.entity();
    let row = crate::chart_tabs::apply_row::render_apply_row(
        this,
        id_prefix,
        vec![StackSetting::Graphics(cfg)],
        None,
        p,
        cx,
    );
    popover = popover.content(
        v_flex()
            .gap_2()
            .children(row)
            .child(render_graphics_popup(id_prefix, entity, cfg, p, cx)),
    );
    popover
}
