//! The "Candles and Trades" popup configures candle and trade-zone rendering for the ACTIVE tab
//! or detached window (the ❚ button beside ⚙; per-tab like the layout settings).
//! The tab spec persists to charts.json through `ChartTabSpec::candle_view`; tabs without an
//! override follow the global `layout.candle_view` default. Like the ⚙ "apply to all" action, the
//! ⧉ button distributes settings to all Add/Custom tabs and detached windows and updates the global
//! default. It includes Main only when Main is the source (`include_main = true`); Add, Custom, and
//! detached-window sources preserve Main's current view.
//! All controls are stateless segments or checkboxes. Candle colors for up, down, and neutral are
//! edited under Settings -> Interface in theme.toml and are shared by all windows.

use gpui::*;
use moon_core::market::candles::{
    CANDLE_MODE_FILLED, CANDLE_MODE_OFF, CANDLE_MODE_OUTLINE, CANDLE_MODE_OUTLINE_IN_ZONE,
    CandleViewCfg,
};
use moon_ui::{
    MoonAccent, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize,
    MoonPalette, MoonSegmentItem, MoonSegmentedControl, h_flex, v_flex,
};
use rust_i18n::t;

use super::common::{LayoutPopupHost, StackSetting};
use crate::design;

/// Time-frame labels kept in sync with `CANDLE_TF_CHOICES_MIN`.
///
/// The 30-second option was removed entirely; legacy configs with `tf_min=0` are clamped to one
/// minute by `tf_ms`.
const TFS: [(u32, &str); 6] = [
    (1, "1м"),
    (5, "5м"),
    (30, "30м"),
    (60, "1ч"),
    (240, "4ч"),
    (1440, "1д"),
];

/// Modes with "Off" (a plain tick chart) first, followed by the Moonbot order.
const MODES: [u8; 4] = [
    CANDLE_MODE_OFF,
    CANDLE_MODE_FILLED,
    CANDLE_MODE_OUTLINE,
    CANDLE_MODE_OUTLINE_IN_ZONE,
];

/// Steps for how many recent candles are redrawn with trades, where zero means candles only.
///
/// The same steps are used for hiding recent candles.
const ZONES: [u16; 7] = [0, 1, 2, 3, 5, 10, 20];

const OUTLINES: [u8; 3] = [1, 2, 3];

/// Scene-popup width in logical pixels; the widest row is seven 42-pixel zone segments plus framing.
pub(super) fn content_width(cx: &App) -> Pixels {
    let pad = f32::from(design::ui_px(cx, 8.0));
    let fpx = f32::from(design::ui_px(cx, 6.0));
    px(7.0 * 42.0 + 2.0 * pad + 2.0 * fpx + 8.0)
}

/// Build a framed group with a thin border and caption, mirroring `layout_popup::framed`.
fn framed(title: String, p: MoonPalette, cx: &App, body: AnyElement) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 4.0))
        .px(design::ui_px(cx, 6.0))
        .py(design::ui_px(cx, 4.0))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(design::r_button(cx))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(title),
        )
        .child(body)
}

/// Build one setting as a caption with a segmented control below it.
fn seg_row(
    id: String,
    caption: String,
    labels: Vec<(String, bool)>,
    seg_w: f32,
    p: MoonPalette,
    cx: &App,
    on_pick: impl Fn(usize, &mut App) + 'static,
) -> impl IntoElement {
    let items: Vec<MoonSegmentItem> = labels
        .into_iter()
        .map(|(label, selected)| {
            let mut it = MoonSegmentItem::new("", label).width(seg_w);
            if selected {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new(id)
        .accent(MoonAccent::Blue)
        .items(items)
        .on_click(move |ix, _, _, cx| on_pick(ix, cx))
        .render();
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 2.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(caption),
        )
        .child(seg)
}

/// Build a multiline hint below a control.
fn hint_block(key: &str, p: MoonPalette, cx: &App) -> impl IntoElement {
    v_flex().children(
        t!(key)
            .to_string()
            .split('\n')
            .map(|line| {
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(line.to_string())
            })
            .collect::<Vec<_>>(),
    )
}

/// Edit the target config by loading its current value, mutating it, and applying it to the tab spec.
fn write_cfg<T: CandlePopupHost>(
    entity: &Entity<T>,
    app: &mut App,
    f: impl FnOnce(&mut CandleViewCfg),
) {
    entity.update(app, |this, cx| {
        let mut cfg = this.candle_view_current(cx);
        f(&mut cfg);
        this.apply_candle_view(cfg, cx);
    });
}

/// Render popup content by reading target values on every render for the stateless controls.
fn render_candle_popup<T: CandlePopupHost>(
    id: &str,
    entity: Entity<T>,
    cfg: CandleViewCfg,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    // --- Candles frame: time frame, mode, and outline thickness. ---
    let tf_row = {
        let entity = entity.clone();
        seg_row(
            format!("{id}-tf"),
            t!("chart.candles.tf").to_string(),
            TFS.iter()
                // Highlight legacy 30-second configs (`tf_min=0`) as one minute, their clamp target.
                .map(|(m, l)| {
                    (
                        l.to_string(),
                        *m == cfg.tf_min || (cfg.tf_min == 0 && *m == 1),
                    )
                })
                .collect(),
            42.0,
            p,
            cx,
            move |ix, app| {
                if let Some((m, _)) = TFS.get(ix) {
                    let m = *m;
                    write_cfg(&entity, app, |c| c.tf_min = m);
                }
            },
        )
    };
    let mode_label = |m: u8| -> String {
        match m {
            CANDLE_MODE_OFF => t!("chart.candles.mode_off").to_string(),
            CANDLE_MODE_FILLED => t!("chart.candles.mode_filled").to_string(),
            CANDLE_MODE_OUTLINE => t!("chart.candles.mode_outline").to_string(),
            _ => t!("chart.candles.mode_zone").to_string(),
        }
    };
    let mode_row = {
        let entity = entity.clone();
        seg_row(
            format!("{id}-mode"),
            t!("chart.candles.mode").to_string(),
            MODES
                .iter()
                .map(|m| (mode_label(*m), *m == cfg.mode.min(CANDLE_MODE_OFF)))
                .collect(),
            70.0,
            p,
            cx,
            move |ix, app| {
                if let Some(m) = MODES.get(ix) {
                    let m = *m;
                    write_cfg(&entity, app, |c| c.mode = m);
                }
            },
        )
    };
    let outline_row = {
        let entity = entity.clone();
        let cur = (cfg.outline_px.round() as u8).clamp(1, 3);
        seg_row(
            format!("{id}-outline"),
            t!("chart.candles.outline").to_string(),
            OUTLINES
                .iter()
                .map(|w| (format!("{w}"), *w == cur))
                .collect(),
            34.0,
            p,
            cx,
            move |ix, app| {
                if let Some(w) = OUTLINES.get(ix) {
                    let w = *w as f32;
                    write_cfg(&entity, app, |c| c.outline_px = w);
                }
            },
        )
    };

    // --- Trades frame: K-candle zone, hidden candles, limit, checkboxes, and neutral mode. ---
    let zone_row = {
        let entity = entity.clone();
        seg_row(
            format!("{id}-zone"),
            t!("chart.candles.zone").to_string(),
            ZONES
                .iter()
                .map(|k| (format!("{k}"), *k == cfg.trade_candles))
                .collect(),
            34.0,
            p,
            cx,
            move |ix, app| {
                if let Some(k) = ZONES.get(ix) {
                    let k = *k;
                    write_cfg(&entity, app, |c| c.trade_candles = k);
                }
            },
        )
    };
    // "Hide recent candles": these buckets draw no candles, only trades.
    let hide_row = {
        let entity = entity.clone();
        seg_row(
            format!("{id}-hide"),
            t!("chart.candles.hide").to_string(),
            ZONES
                .iter()
                .map(|k| (format!("{k}"), *k == cfg.hide_candles))
                .collect(),
            34.0,
            p,
            cx,
            move |ix, app| {
                if let Some(k) = ZONES.get(ix) {
                    let k = *k;
                    write_cfg(&entity, app, |c| c.hide_candles = k);
                }
            },
        )
    };
    // "Price lines" toggles the orange LastPrice and blue MarkPrice MoonProto lines.
    let price_lines_cb = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-price-lines")))
            .label(t!("chart.candles.price_lines").to_string())
            .checked(cfg.price_lines)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| {
                let v = *ch;
                write_cfg(&entity, app, |c| c.price_lines = v);
            })
    };
    let wicks_cb = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-wicks")))
            .label(t!("chart.candles.wicks_in_zone").to_string())
            .checked(cfg.wicks_in_zone)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| {
                let v = *ch;
                write_cfg(&entity, app, |c| c.wicks_in_zone = v);
            })
    };
    let neutral_cb = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-neutral-zone")))
            .label(t!("chart.candles.neutral_in_zone").to_string())
            .checked(cfg.neutral_in_zone)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| {
                let v = *ch;
                write_cfg(&entity, app, |c| c.neutral_in_zone = v);
            })
    };
    // Candle colors for up, down, and neutral are edited under Settings -> Interface. The single
    // theme is shared by all windows and stored with every color in theme.toml, so this only tells
    // users where to edit them.
    let colors_hint = div()
        .text_size(design::t_caption(cx))
        .text_color(rgb(p.text_muted))
        .child(t!("chart.candles.colors_hint").to_string());

    // The ⧉ "apply to all" icon mirrors the layout popup: distribute THIS target's settings to all
    // non-Main tabs and windows, include Main only when it is the source, then update the global
    // default inherited by new tabs.
    let apply_all_btn = {
        let entity = entity.clone();
        MoonButton::new(SharedString::from(format!("{id}-apply-all")))
            .label("⧉")
            .tooltip(t!("chart.candles.apply_all").to_string())
            .size(MoonButtonSize::Micro)
            .variant(MoonButtonVariant::Ghost)
            .on_click(move |_, _w, app| {
                entity.update(app, |this, cx| {
                    let cfg = this.candle_view_current(cx);
                    this.apply_candle_view_all(cfg, cx);
                });
            })
            .render()
    };

    v_flex()
        .id(SharedString::from(format!("{id}-popup")))
        .w_full()
        .p(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 8.0))
        .bg(rgb(p.panel_high))
        .border_1()
        .border_color(rgb(p.border))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .child(t!("chart.candles.title").to_string()),
                )
                .child(div().flex_1())
                .child(apply_all_btn),
        )
        .child(framed(
            t!("chart.candles.frame_candles").to_string(),
            p,
            cx,
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .child(tf_row)
                .child(mode_row)
                .child(hint_block("chart.candles.mode_hint", p, cx))
                .child(outline_row)
                .into_any_element(),
        ))
        .child(framed(
            t!("chart.candles.frame_trades").to_string(),
            p,
            cx,
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .child(zone_row)
                .child(hint_block("chart.candles.zone_hint", p, cx))
                .child(hide_row)
                .child(hint_block("chart.candles.hide_hint", p, cx))
                .child(price_lines_cb)
                .child(wicks_cb)
                .child(neutral_cb)
                .child(colors_hint)
                .into_any_element(),
        ))
        .into_any_element()
}

/// Host for the candle popup in either the tab strip or a detached-window header.
///
/// The target is the strip's active tab or the window panel. Applying and persisting use
/// [`LayoutPopupHost`] through `apply_tab_setting`; each host implements its own "apply to all".
pub(super) trait CandlePopupHost: LayoutPopupHost {
    fn candle_popup_open(&self) -> bool;
    fn set_candle_popup_open(&mut self, open: bool);
    fn candle_popup_hovered(&self) -> bool;
    fn set_candle_popup_hovered(&mut self, hovered: bool);
    /// Return the target's per-tab override, or `None` to follow the global default.
    fn candle_view_override(&self, cx: &App) -> Option<CandleViewCfg>;
    /// Apply settings to all non-Main tabs and windows and update the global default. Include Main
    /// only when the host's source is Main; Add, Custom, and detached sources leave it unchanged.
    fn apply_candle_view_all(&mut self, cfg: CandleViewCfg, cx: &mut Context<Self>);

    /// Return the target's effective settings: its override or the global layout default.
    fn candle_view_current(&self, cx: &App) -> CandleViewCfg {
        self.candle_view_override(cx)
            .unwrap_or(self.backend().read(cx).layout.candle_view)
    }

    /// Apply settings to the target stacks and persist them in the tab spec.
    fn apply_candle_view(&mut self, cfg: CandleViewCfg, cx: &mut Context<Self>) {
        self.apply_tab_setting(StackSetting::CandleView(cfg), cx);
    }

    fn toggle_candle_popup(&mut self, cx: &mut Context<Self>) {
        let open = !self.candle_popup_open();
        self.set_candle_popup_open(open);
        self.set_candle_popup_hovered(false);
        cx.notify();
    }

    fn close_candle_popup(&mut self, cx: &mut Context<Self>) {
        if !self.candle_popup_open() {
            return;
        }
        self.set_candle_popup_open(false);
        self.set_candle_popup_hovered(false);
        cx.notify();
    }
}

/// Build the candle-popup overlay as a positioned scene that closes after hover exit.
///
/// Returns `None` while the popup is closed.
pub(super) fn candle_popup_overlay<T: CandlePopupHost>(
    this: &T,
    id_prefix: &'static str,
    top: Pixels,
    cx: &mut Context<T>,
) -> Option<Stateful<Div>> {
    if !this.candle_popup_open() {
        return None;
    }
    let p = MoonPalette::active(cx);
    let cfg = this.candle_view_current(cx);
    let entity = cx.entity();
    let hover_entity = entity.clone();
    let popup_w = content_width(cx);
    Some(
        div()
            .id(SharedString::from(format!("{id_prefix}-popup-scene")))
            .absolute()
            .right(px(6.0))
            .top(top)
            .w(popup_w)
            .on_mouse_down(MouseButton::Left, |_, _window, app| {
                app.stop_propagation();
            })
            .on_hover(move |hovered, _window, app| {
                hover_entity.update(app, |this, cx| {
                    if *hovered {
                        this.set_candle_popup_hovered(true);
                    } else if this.candle_popup_hovered() {
                        this.close_candle_popup(cx);
                    }
                });
            })
            .child(render_candle_popup(id_prefix, entity, cfg, p, cx)),
    )
}

/// Build the outside-click dismissal layer for the candle popup, or `None` while it is closed.
pub(super) fn candle_popup_dismiss<T: CandlePopupHost>(
    this: &T,
    id_prefix: &'static str,
    cx: &mut Context<T>,
) -> Option<Stateful<Div>> {
    if !this.candle_popup_open() {
        return None;
    }
    let entity = cx.entity();
    Some(
        div()
            .id(SharedString::from(format!("{id_prefix}-popup-dismiss")))
            .absolute()
            .inset_0()
            .on_mouse_down(MouseButton::Left, move |_, _window, app| {
                entity.update(app, |this, cx| this.close_candle_popup(cx));
                app.stop_propagation();
            }),
    )
}
