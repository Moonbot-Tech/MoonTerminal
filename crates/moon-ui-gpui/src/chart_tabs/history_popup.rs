//! The "Trade history" popup (the book button beside the palette one) configures how the chart
//! draws the CLOSED trades: as arrows or as Moonbot's order lines, the size of the arrows, the
//! thickness of their entry-to-exit connector, which closed trades appear at all — real, the
//! emulator's — and whether a closed order keeps its sell line while it is still in the live
//! store.
//!
//! These rows were the first two frames of the graphics popup. They are a subject of their own —
//! what happened, as opposed to what the live picture draws — and a popup of their own reads as
//! one; the graphics popup keeps the lines and the live marks.
//!
//! The closed sell line belongs here rather than with the other lines because of WHEN it acts: only
//! on a closed order, and only in the arrows style — in the lines style the exit IS the picture and
//! the switch is lifted (`chartdx::data_state::orders`). Beside the style row a reader sees why the
//! box does nothing after switching to lines; among the live lines they would not.
//!
//! Everything here is a field of [`ChartGraphicsCfg`], so the graphics popup's host — the target's
//! override, its normalised effective value, the apply path, the ⧉ press — serves this popup
//! unchanged, as it does the volumes popup; only the slot it occupies is its own.

use gpui::*;
use moon_core::config::{ChartGraphicsCfg, TradeHistoryStyle};
use moon_ui::{MoonPalette, MoonPopover, MoonPopoverPlacement, h_flex, v_flex};
use rust_i18n::t;

use super::common::{StackSetting, seg_row};
use super::graphics_popup::{GraphicsPopupHost, ROW_W, flag_cb, nearest, write_cfg};
use super::popup_slot::ChartPopup;
use crate::design;
use crate::panels::{
    popup_apply_all_button, popup_close_button, popup_group, popup_group_inset_px, popup_title,
};

/// Selectable arrow-size multipliers, inside `moon_chart::trade_marks`'s clamp range.
///
/// Steps rather than a slider: `MoonSlider` needs a state entity held on the host, and every other
/// control in these popups is stateless and reads the config on each render.
pub(crate) const ARROW_SCALES: [f32; 6] = [0.6, 0.8, 1.0, 1.3, 1.6, 2.0];

/// Selectable connector thicknesses, in logical px.
pub(crate) const CONNECTOR_PX: [f32; 4] = [1.0, 2.0, 3.0, 4.0];

const SEG_W2: f32 = ROW_W / 2.0;
const SEG_W6: f32 = ROW_W / 6.0;

/// Popup CONTENT width in rendered pixels. `MoonPopover` adds its own padding and border outside it.
pub(super) fn content_width(cx: &App) -> Pixels {
    px(ROW_W + popup_group_inset_px(cx))
}

/// Render popup content by reading the stored values on every render for the stateless controls.
fn render_history_popup<T: HistoryPopupHost>(
    id: &str,
    entity: Entity<T>,
    cfg: ChartGraphicsCfg,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    // Arrows or Moonbot's order lines for the closed trades: a two-way row, because the two are
    // pictures of the same thing at the same place and never coexist.
    let trade_style_row = {
        let entity = entity.clone();
        let lines = cfg.trade_history_style == TradeHistoryStyle::MoonbotLines;
        seg_row(
            format!("{id}-trade-style"),
            t!("chart.graphics.trade_style").to_string(),
            vec![
                (t!("chart.graphics.trade_style_marks").to_string(), !lines),
                (t!("chart.graphics.trade_style_lines").to_string(), lines),
            ],
            SEG_W2,
            p,
            cx,
            move |ix, app| {
                let style = match ix {
                    1 => TradeHistoryStyle::MoonbotLines,
                    _ => TradeHistoryStyle::Marks,
                };
                write_cfg(&entity, app, |c| c.trade_history_style = style);
            },
        )
    };
    // The arrow rows stay in the lines style too: an entry the archive holds no line for keeps
    // its arrow there.
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
            SEG_W6,
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
    // Which closed trades the history layer draws at all.
    let real_cb = flag_cb(
        &entity,
        id,
        "real",
        "chart.graphics.real_trades",
        cfg.show_real_trades,
        |c, v| c.show_real_trades = v,
    );
    let emulator_cb = flag_cb(
        &entity,
        id,
        "emulator",
        "chart.graphics.emulator_trades",
        cfg.show_emulator_trades,
        |c, v| c.show_emulator_trades = v,
    );
    let hide_sell_cb = flag_cb(
        &entity,
        id,
        "hide-closed-sell",
        "chart.graphics.hide_closed_sell",
        cfg.hide_closed_sell_line,
        |c, v| c.hide_closed_sell_line = v,
    )
    .description(t!("chart.graphics.hide_closed_sell_hint").to_string());

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
                .child(popup_title(t!("chart.history.title"), p, cx))
                .child(apply_all_btn)
                .child(popup_close_button(
                    SharedString::from(format!("{id}-close")),
                    {
                        let entity = entity.clone();
                        move |_, _w, app: &mut App| {
                            entity.update(app, |this, cx| this.close_history_popup(cx));
                        }
                    },
                )),
        )
        .child(
            popup_group("frame-history", t!("chart.graphics.frame_history")).child(
                v_flex()
                    .w(px(ROW_W))
                    .gap(design::ui_px(cx, 6.0))
                    .child(trade_style_row)
                    .child(arrow_row)
                    .child(connector_row)
                    .child(real_cb)
                    .child(emulator_cb),
            ),
        )
        .child(
            popup_group(
                "frame-closed-orders",
                t!("chart.history.frame_closed_orders"),
            )
            .child(
                v_flex()
                    .w(px(ROW_W))
                    .gap(design::ui_px(cx, 6.0))
                    .child(hide_sell_cb),
            ),
        )
        .into_any_element()
}

/// Host for the history popup in either the tab strip or a detached-window header.
///
/// See the module doc: the graphics host does everything but the slot.
pub(super) trait HistoryPopupHost: GraphicsPopupHost {
    /// Close the popup. Ownership is checked by the slot; see `GraphicsPopupHost`.
    fn close_history_popup(&mut self, cx: &mut Context<Self>) {
        self.close_chart_popup(ChartPopup::History, cx);
    }
}

impl<T: GraphicsPopupHost> HistoryPopupHost for T {}

/// Build the trade-history popup: a `MoonPopover` anchored to the button that opens it.
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
pub(super) fn history_popup_host<T: HistoryPopupHost>(
    this: &T,
    id_prefix: &'static str,
    trigger: impl IntoElement,
    cx: &mut Context<T>,
) -> MoonPopover {
    let open_entity = cx.entity();
    let mut popover = MoonPopover::new(SharedString::from(format!("{id_prefix}-popover")))
        .placement(MoonPopoverPlacement::BottomEnd)
        .content_width(f32::from(content_width(cx)))
        .close_on_content_click(false)
        .open(this.popup_shows(ChartPopup::History))
        .on_open_change(move |open, _window, app| {
            open_entity.update(app, |this, cx| {
                this.report_chart_popup(ChartPopup::History, open, cx)
            });
        })
        .trigger(trigger);
    if !this.popup_shows(ChartPopup::History) {
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
            .child(render_history_popup(id_prefix, entity, cfg, p, cx)),
    );
    popover
}
