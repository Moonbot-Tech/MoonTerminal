//! The "Candles and Trades" popup configures candle and trade-zone rendering for the ACTIVE tab
//! or detached window (the candlestick button beside ⚙; per-tab like the layout settings).
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
    MoonCheckbox, MoonCheckboxSize, MoonPalette, MoonPopover, MoonPopoverPlacement, h_flex, v_flex,
};
use rust_i18n::t;

use super::common::{LayoutPopupHost, StackSetting, seg_row};
use crate::design;
use crate::panels::{
    popup_apply_all_button, popup_close_button, popup_group, popup_group_inset_px, popup_title,
};

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

/// Popup CONTENT width in rendered pixels. `MoonPopover` adds its own padding and border outside it.
///
/// The row allowance is 7×42 rather than any row's real width: the widest row is the four 70-unit
/// mode segments (280), and the zone/hide rows are seven 34-unit segments (238). The extra ~14 is
/// slack, kept deliberately — the mode labels are localized and ES runs longer than RU/EN.
pub(super) fn content_width(cx: &App) -> Pixels {
    px(7.0 * 42.0 + popup_group_inset_px(cx))
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

/// Build one checkbox bound to a single `CandleViewCfg` flag.
///
/// The popup's flag rows differ only in id, label and the field they write, so they share one
/// builder instead of a hand-rolled block each; `set` is a plain fn pointer because none of these
/// closures captures anything.
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
fn flag_cb<T: CandlePopupHost>(
    entity: &Entity<T>,
    id: &str,
    suffix: &str,
    label_key: &str,
    checked: bool,
    set: fn(&mut CandleViewCfg, bool),
) -> impl IntoElement {
    let entity = entity.clone();
    MoonCheckbox::new(SharedString::from(format!("{id}-{suffix}")))
        .label(t!(label_key).to_string())
        .checked(checked)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| {
            let v = *ch;
            write_cfg(&entity, app, |c| set(c, v));
        })
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
    // The two MoonProto price lines carry a toggle each: the orange LastPrice and the blue
    // MarkPrice. A market whose provider reports no mark price draws none regardless of the flag.
    let last_line_cb = flag_cb(
        &entity,
        id,
        "last-price-line",
        "chart.candles.last_price_line",
        cfg.last_price_line,
        |c, v| c.last_price_line = v,
    );
    let mark_line_cb = flag_cb(
        &entity,
        id,
        "mark-price-line",
        "chart.candles.mark_price_line",
        cfg.mark_price_line,
        |c, v| c.mark_price_line = v,
    );
    let wicks_cb = flag_cb(
        &entity,
        id,
        "wicks",
        "chart.candles.wicks_in_zone",
        cfg.wicks_in_zone,
        |c, v| c.wicks_in_zone = v,
    );
    let neutral_cb = flag_cb(
        &entity,
        id,
        "neutral-zone",
        "chart.candles.neutral_in_zone",
        cfg.neutral_in_zone,
        |c, v| c.neutral_in_zone = v,
    );
    // The MoonShot order's own corridor fill, NOT the layout popup's "zone" (that one shades the
    // trading control strip). It spans the full pane width, so it is the one order area worth a
    // switch of its own.
    let moonshot_cb = flag_cb(
        &entity,
        id,
        "moonshot-zone",
        "chart.candles.moonshot_zone",
        cfg.moonshot_zone,
        |c, v| c.moonshot_zone = v,
    );
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
        popup_apply_all_button(
            SharedString::from(format!("{id}-apply-all")),
            t!("chart.apply_all_tabs_windows").to_string(),
            move |_, _w, app: &mut App| {
                entity.update(app, |this, cx| {
                    let cfg = this.candle_view_current(cx);
                    this.apply_candle_view_all(cfg, cx);
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
                .child(popup_title(t!("chart.candles.title"), p, cx))
                .child(apply_all_btn)
                .child(popup_close_button(
                    SharedString::from(format!("{id}-close")),
                    {
                        let entity = entity.clone();
                        move |_, _w, app: &mut App| {
                            entity.update(app, |this, cx| this.close_candle_popup(cx));
                        }
                    },
                )),
        )
        .child(
            // Group ids are `&'static str`: they only need to be unique among their siblings, and
            // the enclosing root already carries the per-host prefix.
            popup_group("frame-candles", t!("chart.candles.frame_candles")).child(
                v_flex()
                    .gap(design::ui_px(cx, 6.0))
                    .child(tf_row)
                    .child(mode_row)
                    .child(hint_block("chart.candles.mode_hint", p, cx))
                    .child(outline_row),
            ),
        )
        .child(
            popup_group("frame-trades", t!("chart.candles.frame_trades")).child(
                v_flex()
                    .gap(design::ui_px(cx, 6.0))
                    .child(zone_row)
                    .child(hint_block("chart.candles.zone_hint", p, cx))
                    .child(hide_row)
                    .child(hint_block("chart.candles.hide_hint", p, cx))
                    .child(last_line_cb)
                    .child(mark_line_cb)
                    .child(wicks_cb)
                    .child(neutral_cb)
                    .child(moonshot_cb)
                    .child(colors_hint),
            ),
        )
        .into_any_element()
}

/// Host for the candle popup in either the tab strip or a detached-window header.
///
/// The target is the strip's active tab or the window panel. Applying and persisting use
/// [`LayoutPopupHost`] through `apply_tab_setting`; each host implements its own "apply to all".
pub(super) trait CandlePopupHost: LayoutPopupHost {
    fn candle_popup_open(&self) -> bool;
    fn set_candle_popup_open(&mut self, open: bool);
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

    /// Close the popup.
    ///
    /// The already-closed guard is load-bearing: clicking the button while the popup is open makes `Popover`
    /// fire `on_open_change(false)` twice (outside-click handler, then the trigger re-arming).
    fn close_candle_popup(&mut self, cx: &mut Context<Self>) {
        if !self.candle_popup_open() {
            return;
        }
        self.set_candle_popup_open(false);
        cx.notify();
    }
}

/// Build the candles-and-trades popup: a `MoonPopover` anchored to the button that opens it.
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
pub(super) fn candle_popup_host<T: CandlePopupHost>(
    this: &T,
    id_prefix: &'static str,
    trigger: impl IntoElement,
    cx: &mut Context<T>,
) -> MoonPopover {
    let open_entity = cx.entity();
    let mut popover = MoonPopover::new(SharedString::from(format!("{id_prefix}-popover")))
        // Anchored bottom-right of the button: growing left keeps the wide popup inside the
        // window rather than running off its right edge.
        .placement(MoonPopoverPlacement::BottomEnd)
        .content_width(f32::from(content_width(cx)))
        .close_on_content_click(false)
        .open(this.candle_popup_open())
        .on_open_change(move |open, _window, app| {
            open_entity.update(app, |this, cx| {
                this.set_candle_popup_open(open);
                cx.notify();
            });
        })
        .trigger(trigger);
    if !this.candle_popup_open() {
        return popover;
    }
    let p = MoonPalette::active(cx);
    let cfg = this.candle_view_current(cx);
    let entity = cx.entity();
    popover = popover.content(render_candle_popup(id_prefix, entity, cfg, p, cx));
    popover
}
