//! The trade window's ONE settings popup: what every trade window draws.
//!
//! # Why one popup and not the chart's six
//!
//! The main chart splits its settings over six popups because a live chart has six subjects. A
//! frozen replay has fewer: the horizontal profile is off by construction (`chartdx::data_state`
//! draws none under a `trade_replay`), the core price lines need a live order (the MoonShot
//! corridor comes from the report row instead, under a switch of its own here), the order book and
//! the trade zone are switched off by the panel's constructor.
//! What is left — the window's own switches, how the closed trades and their lines are drawn,
//! how the candles look, and the volume band the replay serves from its own prints — fits one
//! popup, and a reader who came to look at ONE trade should not have to learn which of six
//! buttons holds the thing they want to change.
//!
//! # Where an edit goes
//!
//! Every row writes the trade-window KIND's stored set ([`ChartTabKind::Trade`]) at once — there
//! is one kind of trade window, so what it shows is one setting, remembered the way the scale and
//! the "other trades" toggle already are. No per-window override and no "make default" press: a
//! window is not a tab and has no spec to hold a value of its own, and a reader adjusting the
//! picture of one trade means every trade they will open next. Every open trade window follows
//! the stored set: the panel re-reads graphics and captions on the backend notification because
//! it holds no override for them, and the candles are re-pinned by
//! [`TradeWindowView::follow_candle_default`]. No reset either: the rows are few, and each shows
//! what it is set to.
//!
//! The write goes through `store_*`, not the ⧉ row's `set_*_default`: that press also SEPARATES
//! the tab kinds from Main, which is a deliberate statement with its own wording there, and a row
//! here makes none — the same reason the right-click caption menu stores through
//! `store_chart_labels`.
//!
//! # The two forces
//!
//! The window pins its candles to one minute and, until a tick series arrives, refuses candle
//! mode Off — a candle replay has no ticks to draw instead. Both forces live in
//! [`pinned_candle_view`] so the constructor and a mode picked here go through the
//! one rule, and the pinned timeframe never leaks into the stored set:
//! [`candles_for_default`] restores the stored timeframe before the write.

use gpui::*;
use moon_core::config::{ChartGraphicsCfg, ChartTabKind, TradeHistoryStyle};
use moon_core::market::candles::{
    CANDLE_MODE_FILLED, CANDLE_MODE_OFF, CANDLE_MODE_OUTLINE, CANDLE_MODE_OUTLINE_IN_ZONE,
    CandleViewCfg,
};
use moon_ui::{MoonCheckbox, MoonPalette, MoonPopover, MoonPopoverPlacement, h_flex, v_flex};
use rust_i18n::t;

use super::{TradeWindowState, TradeWindowView};
use crate::chart_tabs::{ARROW_SCALES, CANDLE_OUTLINES, CONNECTOR_PX, nearest, seg_row};
use crate::design;
use crate::panels::common::popup_gear_trigger_dense;
use crate::panels::{popup_close_button, popup_group, popup_group_inset_px, popup_title};

/// Row allowance — the chart popups' own, so this popup is as wide as theirs by construction:
/// the widest row here is the six arrow-size segments at a sixth of it each.
const ROW_W: f32 = crate::chart_tabs::POPUP_ROW_W;

/// The timeframe every trade window draws at, in minutes.
///
/// The replay is fetched at one minute and the panel is pinned to it; see `window.rs` for why a
/// coarser bucket under a caption naming minutes was the bug this pin removed.
pub(super) const PINNED_TF_MIN: u32 = 1;

/// The candle modes this window offers, in the main chart's order minus "In zone".
///
/// The trade ZONE — the last N candles before NOW, where the main chart draws trades over the
/// candles, hides wicks or neutral colour, and where "In zone" switches from filled to outlined
/// (`chartdx::data_state::market`, `trades_zone_rel`) — is anchored to the wall clock, not to the
/// picture. A frozen replay ends minutes to months before now, so the zone never reaches it: the
/// zone rows would change nothing here, and "In zone" draws exactly as "Filled". None of them are
/// offered, and a stored "In zone" lights the segment it actually draws as.
const TRADE_WINDOW_MODES: [u8; 3] = [CANDLE_MODE_OFF, CANDLE_MODE_FILLED, CANDLE_MODE_OUTLINE];

/// The segment a stored candle mode lights in this window's mode row.
///
/// Args:
///     mode: The stored mode, any of the four the main chart knows.
///
/// Returns:
///     One of [`TRADE_WINDOW_MODES`]: "In zone" answers "Filled", which is what it draws as on a
///     chart the zone never reaches; an unknown value answers "Off" as the main chart's row does.
pub(super) fn trade_window_mode_segment(mode: u8) -> u8 {
    match mode {
        CANDLE_MODE_OUTLINE_IN_ZONE => CANDLE_MODE_FILLED,
        other => other.min(CANDLE_MODE_OFF),
    }
}

/// Popup CONTENT width in rendered pixels; `MoonPopover` adds its own padding and border outside.
fn content_width(cx: &App) -> Pixels {
    px(ROW_W + popup_group_inset_px(cx))
}

/// The candle settings the panel is told to DRAW, from the user's own choice.
///
/// Two forces, both the window's: the timeframe is always [`PINNED_TF_MIN`], and candle mode Off
/// — a pure tick chart — is replaced by the shipped mode while no tick series is on screen, since
/// a candle replay has no ticks and would draw an empty pane under a caption naming candles. Once
/// ticks are on offer the second force's reason is gone and Off is honoured.
///
/// Args:
///     user: The user's own choice, as the popup shows it and the default stores it.
///     ticks_on_screen: Whether the panel currently draws a tick series.
///
/// Returns:
///     What the panel draws.
pub(super) fn pinned_candle_view(user: CandleViewCfg, ticks_on_screen: bool) -> CandleViewCfg {
    let mut view = user;
    view.tf_min = PINNED_TF_MIN;
    if view.mode == CANDLE_MODE_OFF && !ticks_on_screen {
        view.mode = CandleViewCfg::default().mode;
    }
    view
}

/// The candle settings a row here stores for the trade-window kind.
///
/// The pin is the WINDOW's rule, not the stored set's: the set keeps whatever timeframe it
/// already carried, so a row here never writes the pinned minute into `layout.toml`. Everything
/// else is what this window shows.
///
/// Args:
///     shown: The user's choice as this window holds it — pinned timeframe included.
///     stored_tf_min: The timeframe the kind's stored set carries.
///
/// Returns:
///     The set to store.
pub(super) fn candles_for_default(shown: CandleViewCfg, stored_tf_min: u32) -> CandleViewCfg {
    CandleViewCfg {
        tf_min: stored_tf_min,
        ..shown
    }
}

/// One checkbox bound to a single [`ChartGraphicsCfg`] flag on the window.
fn graphics_cb(
    entity: &Entity<TradeWindowView>,
    suffix: &str,
    label_key: &str,
    checked: bool,
    set: fn(&mut ChartGraphicsCfg, bool),
) -> MoonCheckbox {
    let entity = entity.clone();
    MoonCheckbox::new(SharedString::from(format!(
        "trade-window-settings-{suffix}"
    )))
    .label(t!(label_key).to_string())
    .checked(checked)
    .on_change(move |ch: &bool, _w, app| {
        let v = *ch;
        entity.update(app, |this, cx| this.write_graphics(cx, |c| set(c, v)));
    })
}

impl TradeWindowView {
    /// Whether the chart currently draws a tick series, which is what lets candle mode Off stand.
    fn ticks_on_screen(&self) -> bool {
        matches!(&self.state, TradeWindowState::Ready { source, .. } if source.is_ticks())
    }

    /// The chart-drawing settings as the chart draws them: the trade-window kind's stored set,
    /// normalised (the panel holds no override of its own for them).
    pub(super) fn graphics_cfg(&self, cx: &App) -> ChartGraphicsCfg {
        self.panel.read(cx).effective_chart_graphics(cx)
    }

    /// The candle settings as the USER chose them: the panel's effective set with the user's own
    /// mode in place of whatever the candle-stage force is currently drawing.
    pub(super) fn candle_cfg(&self, cx: &App) -> CandleViewCfg {
        let mut view = self.panel.read(cx).effective_candle_view(cx);
        view.mode = self.user_candle_mode;
        view
    }

    /// Edit the trade-window graphics, for every trade window at once.
    fn write_graphics(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut ChartGraphicsCfg)) {
        let mut cfg = self.graphics_cfg(cx);
        f(&mut cfg);
        self.backend.update(cx, |b, bcx| {
            // Normalised on the way in, as the ⧉ row's slot does: the value is COMPARED on every
            // notification, and an out-of-range number would read as a change for ever.
            if b.layout.store_chart_graphics(
                ChartTabKind::Trade,
                moon_chart::normalize_chart_graphics(cfg),
            ) {
                b.layout_dirty = true;
            }
            // Every trade window's panel re-reads the kind's graphics on this.
            bcx.notify();
        });
        cx.notify();
    }

    /// Edit the trade-window candles, for every trade window at once.
    ///
    /// The mode picked here is the user's choice, stored as such: the pin and the candle-stage
    /// force are applied on the way back by [`Self::follow_candle_default`], which this calls at
    /// once for this window and which the other windows reach through their observers.
    fn write_candles(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut CandleViewCfg)) {
        let mut cfg = self.candle_cfg(cx);
        f(&mut cfg);
        self.backend.update(cx, |b, bcx| {
            let kind = ChartTabKind::Trade;
            let stored = candles_for_default(cfg, b.layout.candle_view_for(kind).tf_min);
            if b.layout.store_candle_view(kind, stored) {
                b.layout_dirty = true;
            }
            bcx.notify();
        });
        self.follow_candle_default(cx);
        cx.notify();
    }

    /// Re-pin the candles from the trade-window kind's stored set when it moved.
    ///
    /// Run from the backend observer, so the common case — nothing moved — costs one compare.
    /// Graphics and captions need no counterpart: with no override the panel re-reads their
    /// stored set itself. The candles do, because their override is never absent (see
    /// [`TradeWindowView::candles_followed`]).
    pub(super) fn follow_candle_default(&mut self, cx: &mut Context<Self>) {
        let now = self
            .backend
            .read(cx)
            .layout
            .candle_view_for(ChartTabKind::Trade);
        if now == self.candles_followed {
            return;
        }
        self.candles_followed = now;
        self.user_candle_mode = now.mode;
        let view = pinned_candle_view(now, self.ticks_on_screen());
        self.panel
            .update(cx, |panel, pcx| panel.set_candle_view(Some(view), pcx));
        cx.notify();
    }

    /// Open or close the popup.
    pub(super) fn set_settings_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.settings_open == open {
            return;
        }
        self.settings_open = open;
        cx.notify();
    }

    /// The ⚙ trigger with its anchored popup.
    ///
    /// The content is built ONLY while open — `MoonPopover` takes it eagerly, and this sits in a
    /// chart host that repaints on every replay tick.
    pub(super) fn settings_popup_host(&self, cx: &mut Context<Self>) -> MoonPopover {
        let open_entity = cx.entity();
        let trigger = popup_gear_trigger_dense(
            "trade-window-settings",
            t!("trade_window.settings.tip").to_string(),
            self.settings_open,
        );
        let mut popover = MoonPopover::new(SharedString::from("trade-window-settings-popover"))
            // Anchored bottom-right of the button: growing left keeps the popup inside a window
            // whose ⚙ sits at the right edge of the header.
            .placement(MoonPopoverPlacement::BottomEnd)
            .content_width(f32::from(content_width(cx)))
            .close_on_content_click(false)
            .open(self.settings_open)
            .on_open_change(move |open, _window, app| {
                open_entity.update(app, |this, cx| this.set_settings_open(open, cx));
            })
            .trigger(trigger);
        if !self.settings_open {
            return popover;
        }
        let p = MoonPalette::active(cx);
        popover = popover.content(render_settings_popup(
            cx.entity(),
            WindowSwitches {
                show_other_trades: self.show_other_trades,
                fit_trade: self.fit_trade,
                hide_rail: self.hide_rail,
                show_labels: self.show_labels,
                load_ticks: self.load_ticks,
                show_corridor: self.show_corridor,
            },
            self.graphics_cfg(cx),
            self.candle_cfg(cx),
            p,
            cx,
        ));
        popover
    }
}

/// The popup's content, read from the panel on every render for the stateless controls.
///
/// Args:
///     entity: The window, for the rows' handlers.
///     show_other_trades: The window's own toggle, as it stands.
///     graphics: What the chart draws, normalised.
///     candles: The user's candle choice.
///     p: Active palette.
///     cx: App context.
///
/// Returns:
///     The content root.
/// The window's own switches, as the popup shows them: remembered across windows like the scale,
/// and not part of any chart setting.
struct WindowSwitches {
    show_other_trades: bool,
    fit_trade: bool,
    hide_rail: bool,
    show_labels: bool,
    load_ticks: bool,
    show_corridor: bool,
}

fn render_settings_popup(
    entity: Entity<TradeWindowView>,
    switches: WindowSwitches,
    graphics: ChartGraphicsCfg,
    candles: CandleViewCfg,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let id = "trade-window-settings";
    let gap = design::ui_px(cx, 6.0);

    // --- Trades and the window: the window's own switches. ---
    let other_trades_cb = {
        let entity = entity.clone();
        MoonCheckbox::new("trade-window-other-trades")
            .label(t!("trade_window.other_trades").to_string())
            .checked(switches.show_other_trades)
            .on_change(move |show: &bool, _w, app| {
                let show = *show;
                entity.update(app, |this, cx| this.set_other_trades(show, cx));
            })
    };
    let fit_cb = {
        let entity = entity.clone();
        MoonCheckbox::new("trade-window-fit")
            .label(t!("trade_window.settings.fit").to_string())
            .checked(switches.fit_trade)
            .on_change(move |fit: &bool, _w, app| {
                let fit = *fit;
                entity.update(app, |this, cx| this.set_fit_trade(fit, cx));
            })
    };
    let ticks_cb = {
        let entity = entity.clone();
        MoonCheckbox::new("trade-window-load-ticks")
            .label(t!("trade_window.settings.load_ticks").to_string())
            .checked(switches.load_ticks)
            .on_change(move |load: &bool, _w, app| {
                let load = *load;
                entity.update(app, |this, cx| this.set_load_ticks(load, cx));
            })
    };
    let corridor_cb = {
        let entity = entity.clone();
        MoonCheckbox::new("trade-window-moonshot-zone")
            .label(t!("trade_window.settings.moonshot_zone").to_string())
            .checked(switches.show_corridor)
            .on_change(move |show: &bool, _w, app| {
                let show = *show;
                entity.update(app, |this, cx| this.set_show_corridor(show, cx));
            })
    };
    let hide_rail_cb = {
        let entity = entity.clone();
        MoonCheckbox::new("trade-window-hide-rail")
            .label(t!("trade_window.settings.hide_rail").to_string())
            .checked(switches.hide_rail)
            .on_change(move |hide: &bool, _w, app| {
                let hide = *hide;
                entity.update(app, |this, cx| this.set_hide_rail(hide, cx));
            })
    };

    let labels_cb = {
        let entity = entity.clone();
        MoonCheckbox::new("trade-window-labels")
            .label(t!("trade_window.settings.show_labels").to_string())
            .checked(switches.show_labels)
            .on_change(move |show: &bool, _w, app| {
                let show = *show;
                entity.update(app, |this, cx| this.set_show_labels(show, cx));
            })
    };

    // --- Volumes: the band, ONE switch as on the main chart, written as the pair the rule maps
    // it to. The replay serves it from its own prints and bars; the per-trade bars the band
    // replaces are switched off by the band itself (`chartdx::data_state::market`). ---
    let volumes_on = moon_chart::volume_bars::volume_band_on(
        graphics.candle_volume_style,
        graphics.candle_volume_sides,
    );
    let volumes_cb = graphics_cb(
        &entity,
        "volumes",
        "chart.volumes.band_enabled",
        volumes_on,
        |c, v| {
            c.candle_volume_style = if v {
                moon_core::market::candles::VOLUME_STYLE_HILLS
            } else {
                moon_core::market::candles::VOLUME_STYLE_OFF
            };
            c.candle_volume_sides = v;
        },
    );

    // --- History: how the closed trades and their lines are drawn. ---
    let trade_style_row = {
        let entity = entity.clone();
        let lines = graphics.trade_history_style == TradeHistoryStyle::MoonbotLines;
        seg_row(
            format!("{id}-trade-style"),
            t!("chart.graphics.trade_style").to_string(),
            vec![
                (t!("chart.graphics.trade_style_marks").to_string(), !lines),
                (t!("chart.graphics.trade_style_lines").to_string(), lines),
            ],
            ROW_W / 2.0,
            p,
            cx,
            move |ix, app| {
                let style = match ix {
                    1 => TradeHistoryStyle::MoonbotLines,
                    _ => TradeHistoryStyle::Marks,
                };
                entity.update(app, |this, cx| {
                    this.write_graphics(cx, |c| c.trade_history_style = style)
                });
            },
        )
    };
    let arrow_row = {
        let entity = entity.clone();
        let current = nearest(&ARROW_SCALES, graphics.trade_arrow_scale);
        seg_row(
            format!("{id}-arrow"),
            t!("chart.graphics.arrow_size").to_string(),
            ARROW_SCALES
                .iter()
                .enumerate()
                .map(|(index, v)| (format!("{v}x"), index == current))
                .collect(),
            ROW_W / 6.0,
            p,
            cx,
            move |ix, app| {
                if let Some(v) = ARROW_SCALES.get(ix) {
                    let v = *v;
                    entity.update(app, |this, cx| {
                        this.write_graphics(cx, |c| c.trade_arrow_scale = v)
                    });
                }
            },
        )
    };
    let connector_row = {
        let entity = entity.clone();
        let current = nearest(&CONNECTOR_PX, graphics.connector_thickness_px);
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
                    entity.update(app, |this, cx| {
                        this.write_graphics(cx, |c| c.connector_thickness_px = v)
                    });
                }
            },
        )
    };
    let real_cb = graphics_cb(
        &entity,
        "real",
        "chart.graphics.real_trades",
        graphics.show_real_trades,
        |c, v| c.show_real_trades = v,
    );
    let emulator_cb = graphics_cb(
        &entity,
        "emulator",
        "chart.graphics.emulator_trades",
        graphics.show_emulator_trades,
        |c, v| c.show_emulator_trades = v,
    );
    // The one line switch that still acts on a frozen chart: the archived lines carry their
    // repricing trail. The closed sell line's switch is lifted on this viewer by design (the
    // closed order IS the picture), so it is not offered.
    let hide_path_cb = graphics_cb(
        &entity,
        "hide-move-history",
        "chart.graphics.hide_move_history",
        graphics.hide_order_move_history,
        |c, v| c.hide_order_move_history = v,
    );

    // --- Candles: mode and outline. No timeframe row (pinned) and no zone rows (see
    // `TRADE_WINDOW_MODES`). ---
    let mode_label = |m: u8| -> String {
        match m {
            CANDLE_MODE_OFF => t!("chart.candles.mode_off").to_string(),
            CANDLE_MODE_FILLED => t!("chart.candles.mode_filled").to_string(),
            _ => t!("chart.candles.mode_outline").to_string(),
        }
    };
    let mode_row = {
        let entity = entity.clone();
        let lit = trade_window_mode_segment(candles.mode);
        seg_row(
            format!("{id}-mode"),
            t!("chart.candles.mode").to_string(),
            TRADE_WINDOW_MODES
                .iter()
                .map(|m| (mode_label(*m), *m == lit))
                .collect(),
            ROW_W / 3.0,
            p,
            cx,
            move |ix, app| {
                if let Some(m) = TRADE_WINDOW_MODES.get(ix) {
                    let m = *m;
                    entity.update(app, |this, cx| this.write_candles(cx, |c| c.mode = m));
                }
            },
        )
    };
    let outline_row = {
        let entity = entity.clone();
        let cur = (candles.outline_px.round() as u8).clamp(1, 3);
        seg_row(
            format!("{id}-outline"),
            t!("chart.candles.outline").to_string(),
            CANDLE_OUTLINES
                .iter()
                .map(|w| (format!("{w}"), *w == cur))
                .collect(),
            34.0,
            p,
            cx,
            move |ix, app| {
                if let Some(w) = CANDLE_OUTLINES.get(ix) {
                    let w = *w as f32;
                    entity.update(app, |this, cx| this.write_candles(cx, |c| c.outline_px = w));
                }
            },
        )
    };
    // Chrome is MoonPopover's; see `popover_contents_do_not_paint_a_second_surface`.
    v_flex()
        .id("trade-window-settings-popup")
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .child(popup_title(t!("trade_window.settings.title"), p, cx))
                .child(popup_close_button(
                    SharedString::from(format!("{id}-close")),
                    {
                        let entity = entity.clone();
                        move |_, _w, app: &mut App| {
                            entity.update(app, |this, cx| this.set_settings_open(false, cx));
                        }
                    },
                )),
        )
        .child(
            popup_group("frame-trades", t!("trade_window.settings.frame_trades")).child(
                v_flex()
                    .w(px(ROW_W))
                    .gap(gap)
                    .child(other_trades_cb)
                    .child(fit_cb)
                    .child(ticks_cb)
                    .child(corridor_cb)
                    .child(labels_cb)
                    .child(hide_rail_cb),
            ),
        )
        .child(
            popup_group("frame-volumes", t!("chart.volumes.title"))
                .child(v_flex().w(px(ROW_W)).gap(gap).child(volumes_cb)),
        )
        .child(
            popup_group("frame-history", t!("chart.graphics.frame_history")).child(
                v_flex()
                    .w(px(ROW_W))
                    .gap(gap)
                    .child(trade_style_row)
                    .child(arrow_row)
                    .child(connector_row)
                    .child(real_cb)
                    .child(emulator_cb)
                    .child(hide_path_cb),
            ),
        )
        .child(
            popup_group("frame-candles", t!("chart.candles.frame_candles")).child(
                v_flex()
                    .w(px(ROW_W))
                    .gap(gap)
                    .child(mode_row)
                    .child(outline_row),
            ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests;
