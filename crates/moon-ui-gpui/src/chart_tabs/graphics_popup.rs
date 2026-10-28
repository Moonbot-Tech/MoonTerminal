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
//! The order book has its own frame: whether it is shown at all, whether a hidden book still
//! leaves the order strip, its width, its KIND — Moonbot's glass or a price ladder — and the
//! ladder's highlight. The kind and the width are fields of [`ChartGraphicsCfg`] and travel per
//! tab and with ⧉ like every switch here. The two checkboxes are tab-spec fields with no per-kind
//! default (`StackSetting::Orderbook`, `StackSetting::ShowZone`); the ⧉ press writes them into the
//! addressed tabs beside storing the graphics default.
//!
//! Controls read stored config on every render and are stateless on purpose: `MoonSlider` needs a
//! state entity held on the host, and every other control in these popups reads its value straight
//! from the config, so the sizes are offered as steps. The book width is the one exception — a
//! number field asked for by name — and its state lives on the host beside the ⚙ popup's fields.

use gpui::*;
use moon_core::config::{BookKind, ChartGraphicsCfg, TradeKind, book_width};
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonVariant, MoonCheckbox, MoonInput, MoonInputEvent,
    MoonInputState, MoonPalette, MoonPopover, MoonPopoverPlacement, MoonSize, h_flex, v_flex,
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

/// Selectable opacities for the per-TRADE volume bars. `0.35` is the shipped default.
const TRADE_VOLUME_ALPHAS: [f32; 6] = [0.0, 0.15, 0.35, 0.5, 0.75, 1.0];

/// Row width in rendered pixels, and the segment width of its six-step rows.
///
/// Kept at the width the two-column popup's rows had, so the checkbox labels wrap where they did;
/// the localized ES strings are the longest of the three.
pub(crate) const ROW_W: f32 = 7.0 * 42.0;
const SEG_W6: f32 = ROW_W / 6.0;

/// The trade kinds the popup offers, in display order, with their locale keys.
const TRADE_KINDS: [(TradeKind, &str); 2] = [
    (TradeKind::Crosses, "chart.graphics.trade_kind_crosses"),
    (TradeKind::Bubbles, "chart.graphics.trade_kind_bubbles"),
];

/// The book kinds the popup offers, in display order, with their locale keys.
const BOOK_KINDS: [(BookKind, &str); 2] = [
    (BookKind::Bars, "chart.graphics.book_kind_bars"),
    (BookKind::Ladder, "chart.graphics.book_kind_ladder"),
];

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

/// The order-book frame's tab-spec state, read from the host for one render.
struct BookToggles<'a> {
    /// Whether the tab draws its order book.
    shown: bool,
    /// Whether a hidden book still leaves the marked order strip on the right.
    zone: bool,
    /// The width field; its state belongs to the host.
    width_input: &'a Entity<MoonInputState>,
}

/// Render popup content by reading the stored values on every render for the stateless controls.
fn render_graphics_popup<T: GraphicsPopupHost>(
    id: &str,
    entity: Entity<T>,
    cfg: ChartGraphicsCfg,
    book: BookToggles<'_>,
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

    // --- Trade kind: crosses, or bubbles sized by notional. ---
    let trade_kind_row = {
        let entity = entity.clone();
        seg_row(
            format!("{id}-trade-kind"),
            t!("chart.graphics.trade_kind").to_string(),
            TRADE_KINDS
                .iter()
                .map(|(kind, key)| (t!(*key).to_string(), *kind == cfg.trade_kind))
                .collect(),
            ROW_W / TRADE_KINDS.len() as f32,
            p,
            cx,
            move |ix, app| {
                if let Some((kind, _)) = TRADE_KINDS.get(ix) {
                    let kind = *kind;
                    write_cfg(&entity, app, |c| c.trade_kind = kind);
                }
            },
        )
    };

    // --- Bubbles: the merge window and the small-print threshold, shown only in bubble mode. ---
    let bubbles = cfg.trade_kind == TradeKind::Bubbles;
    let trade_merge_row = bubbles.then(|| {
        let entity = entity.clone();
        let windows = moon_chart::clusters::MERGE_WINDOWS_MS;
        seg_row(
            format!("{id}-trade-merge"),
            t!("chart.graphics.trade_merge").to_string(),
            windows
                .iter()
                .map(|v| (v.to_string(), *v == cfg.trade_merge_ms))
                .collect(),
            ROW_W / windows.len() as f32,
            p,
            cx,
            move |ix, app| {
                if let Some(v) = windows.get(ix) {
                    let v = *v;
                    write_cfg(&entity, app, |c| c.trade_merge_ms = v);
                }
            },
        )
    });
    let trade_small_row = bubbles.then(|| {
        let entity = entity.clone();
        let pcts = moon_chart::clusters::SMALL_PCTS;
        seg_row(
            format!("{id}-trade-small"),
            t!("chart.graphics.trade_small").to_string(),
            pcts.iter()
                .map(|v| (format!("{v}%"), *v == cfg.trade_small_pct))
                .collect(),
            ROW_W / pcts.len() as f32,
            p,
            cx,
            move |ix, app| {
                if let Some(v) = pcts.get(ix) {
                    let v = *v;
                    write_cfg(&entity, app, |c| c.trade_small_pct = v);
                }
            },
        )
    });
    // Says what a bubble is and what its size is measured against.
    let trade_bubbles_hint = bubbles.then(|| {
        div()
            .text_size(design::t_caption(cx))
            .text_color(rgb(p.text_muted))
            .child(t!("chart.graphics.trade_bubbles_hint").to_string())
    });

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

    // --- Order book: shown or not, the strip it leaves when hidden, its width, its kind, and for
    // a ladder the highlight. ---
    let book_show_cb = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-book-show")))
            .label(t!("chart.graphics.book_show").to_string())
            .checked(book.shown)
            .on_change(move |ch: &bool, _w, app| {
                let v = *ch;
                entity.update(app, |this, cx| {
                    this.apply_tab_setting(StackSetting::Orderbook(v), cx)
                });
            })
    };
    // Off together with the book, such a tab is chart edge to edge and takes no order gesture —
    // click placement, drag, cancel, menu, keyboard cancel (#557).
    let book_zone_cb = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-book-zone")))
            .label(t!("chart.graphics.book_zone").to_string())
            .checked(book.zone)
            .on_change(move |ch: &bool, _w, app| {
                let v = *ch;
                entity.update(app, |this, cx| {
                    this.apply_tab_setting(StackSetting::ShowZone(v), cx)
                });
            })
    };
    let book_width_row = v_flex()
        .gap(design::ui_px(cx, 2.0))
        .child(
            h_flex()
                .gap(design::ui_px(cx, 6.0))
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text))
                        .child(t!("chart.graphics.book_width").to_string()),
                )
                .child(
                    div().w(px(64.0)).child(
                        MoonInput::new(SharedString::from(format!("{id}-book-width")))
                            .state(book.width_input)
                            .size(design::INPUT_SIZE),
                    ),
                )
                .child(div().text_color(rgb(p.text_muted)).child("px")),
        )
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(
                    t!(
                        "chart.graphics.book_width_hint",
                        min = book_width::MIN,
                        max = book_width::MAX
                    )
                    .to_string(),
                ),
        );
    let book_kind_row = {
        let entity = entity.clone();
        seg_row(
            format!("{id}-book-kind"),
            t!("chart.graphics.book_kind").to_string(),
            BOOK_KINDS
                .iter()
                .map(|(kind, key)| (t!(*key).to_string(), *kind == cfg.book_kind))
                .collect(),
            ROW_W / BOOK_KINDS.len() as f32,
            p,
            cx,
            move |ix, app| {
                if let Some((kind, _)) = BOOK_KINDS.get(ix) {
                    let kind = *kind;
                    write_cfg(&entity, app, |c| c.book_kind = kind);
                }
            },
        )
    };
    let ladder = cfg.book_kind == BookKind::Ladder;
    let ladder_big_row = ladder.then(|| {
        let entity = entity.clone();
        let pcts = moon_chart::ladder::LADDER_BIG_PCTS;
        seg_row(
            format!("{id}-ladder-big"),
            t!("chart.graphics.ladder_big").to_string(),
            pcts.iter()
                .map(|v| {
                    let label = if *v == 0 {
                        t!("chart.graphics.ladder_big_off").to_string()
                    } else {
                        format!("{v}%")
                    };
                    (label, *v == cfg.ladder_big_pct)
                })
                .collect(),
            ROW_W / pcts.len() as f32,
            p,
            cx,
            move |ix, app| {
                if let Some(v) = pcts.get(ix) {
                    let v = *v;
                    write_cfg(&entity, app, |c| c.ladder_big_pct = v);
                }
            },
        )
    });

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
                        move |_, window, app: &mut App| {
                            entity.update(app, |this, cx| this.close_graphics_popup(window, cx));
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
                    .child(trade_kind_row)
                    .children(trade_merge_row)
                    .children(trade_small_row)
                    .children(trade_bubbles_hint)
                    // Kept in bubble mode too: the liquidations stay crosses.
                    .child(marker_scale_row)
                    .child(trade_volume_alpha_row)
                    .child(trade_volume_alpha_hint)
                    .child(liquidations_cb),
            ),
        )
        .child(
            popup_group("frame-book", t!("chart.graphics.frame_book")).child(
                v_flex()
                    .w(px(ROW_W))
                    .gap(design::ui_px(cx, 6.0))
                    .child(book_show_cb)
                    .child(book_zone_cb)
                    .child(book_width_row)
                    .child(book_kind_row)
                    .children(ladder_big_row),
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

    /// The book-width field. Held by the host, like the ⚙ popup's fields, because a
    /// `MoonInputState` has to outlive the popup's per-render content.
    fn book_width_input(&self) -> &Entity<MoonInputState>;

    /// Fill the width field with what the target draws now, rounded to whole pixels.
    fn seed_book_width_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        let width = self.graphics_cfg(cx).book_width_px.round().to_string();
        self.book_width_input()
            .clone()
            .update(cx, |input, c| input.set_value(width, window, c));
    }

    /// Keep the width field in step with the target while nobody is typing in it, and hand the
    /// keyboard back when the popup is gone.
    ///
    /// Called on every render of the host. Seeding only on open would leave the field stale after
    /// anything else moves the width while the popup is up — a ⧉ press setting or resetting the
    /// kind's default, another window editing the default this tab follows. A popup DISPLACED by
    /// a neighbouring button closes through the slot, where no window is at hand to release the
    /// field's focus; that release happens here, on the next frame.
    fn sync_book_width_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.book_width_input();
        if !self.popup_shows(ChartPopup::Graphics) {
            crate::hotkeys::release_field_focus(input, window, cx);
            return;
        }
        let focused = window
            .focused(cx)
            .is_some_and(|f| f == input.read(cx).focus_handle(cx));
        let shown = self.graphics_cfg(cx).book_width_px.round().to_string();
        if !focused && input.read(cx).value() != shown.as_str() {
            self.seed_book_width_input(window, cx);
        }
    }

    /// The field's number when it lies OUTSIDE the offered range, for a clamped commit.
    ///
    /// An in-range number has already been applied by the field's change event; committing it
    /// again on leaving the field could only re-apply a value something else has replaced since,
    /// such as a ⧉ reset, turning the tab back into an override of its own.
    fn pending_out_of_range_width(&self, cx: &App) -> Option<f32> {
        self.book_width_input()
            .read(cx)
            .value()
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|w| !(book_width::MIN..=book_width::MAX).contains(w))
    }

    /// Store a book width on the target, bounded the way the layout reads it.
    ///
    /// Writes nothing when the target already draws that width, so a blur on an untouched field
    /// does not turn a tab that follows its kind default into one with an override of its own.
    fn commit_book_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let cfg = self.graphics_cfg(cx);
        let width = book_width::normalize(width);
        if cfg.book_width_px == width {
            return;
        }
        self.apply_graphics(
            ChartGraphicsCfg {
                book_width_px: width,
                ..cfg
            },
            cx,
        );
    }

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

    /// Close the popup, handing the keyboard back if the width field held it.
    ///
    /// Ownership is checked by the slot, which is load-bearing: clicking the button while the popup
    /// is open makes `Popover` fire `on_open_change(false)` twice (outside-click handler, then the
    /// trigger re-arming). The focus release is the ⚙ popup's reason exactly — see
    /// `LayoutPopupHost::release_layout_field_focus`: a host-owned field left focused after its
    /// popup stops rendering silently kills every hotkey. A pending out-of-range number is applied
    /// clamped FIRST: the blur the release causes arrives with the popup already gone, and the
    /// field's subscription ignores it then.
    fn close_graphics_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup_shows(ChartPopup::Graphics)
            && let Some(width) = self.pending_out_of_range_width(cx)
        {
            self.commit_book_width(width, cx);
        }
        self.close_chart_popup(ChartPopup::Graphics, cx);
        crate::hotkeys::release_field_focus(self.book_width_input(), window, cx);
    }
}

/// Keep the book-width field and the target's width in step.
///
/// Every edit whose number is already in range is applied at once: both bounds have three digits,
/// so the one- and two-digit states a number passes through on its way are never in range and
/// never applied. Leaving the field or pressing Enter applies an out-of-range number clamped and
/// rewrites the field with what the chart now draws, so a stray "700" reads back as 600; an
/// in-range number is not applied a second time there (see `pending_out_of_range_width`).
///
/// One subscription per host, set up where the field is created, as the ⚙ popup's fields are.
pub(super) fn subscribe_book_width<T: GraphicsPopupHost>(
    input: &Entity<MoonInputState>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    cx.subscribe_in(
        input,
        window,
        |this, input, ev: &MoonInputEvent, window, cx| {
            if !this.popup_shows(ChartPopup::Graphics) {
                return;
            }
            match ev {
                MoonInputEvent::Change => {
                    let typed = input.read(cx).value().trim().parse::<f32>().ok();
                    if let Some(width) =
                        typed.filter(|w| (book_width::MIN..=book_width::MAX).contains(w))
                    {
                        this.commit_book_width(width, cx);
                    }
                }
                MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. } => {
                    if let Some(width) = this.pending_out_of_range_width(cx) {
                        this.commit_book_width(width, cx);
                    }
                    this.seed_book_width_input(window, cx);
                }
                _ => {}
            }
        },
    )
    .detach();
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
///     window: The host's window, for keeping the width field in step.
///     cx: Host context.
///
/// Returns:
///     The trigger with its anchored popover.
pub(super) fn graphics_popup_host<T: GraphicsPopupHost>(
    this: &T,
    id_prefix: &'static str,
    trigger: impl IntoElement,
    window: &mut Window,
    cx: &mut Context<T>,
) -> MoonPopover {
    this.sync_book_width_field(window, cx);
    let open_entity = cx.entity();
    let mut popover = MoonPopover::new(SharedString::from(format!("{id_prefix}-popover")))
        // Anchored bottom-right of the button, as the candle popup beside it is: growing left
        // keeps the popup inside the window rather than running off its right edge.
        .placement(MoonPopoverPlacement::BottomEnd)
        .content_width(f32::from(content_width(cx)))
        .close_on_content_click(false)
        .open(this.popup_shows(ChartPopup::Graphics))
        .on_open_change(move |open, window, app| {
            open_entity.update(app, |this, cx| match open {
                true => {
                    this.seed_book_width_input(window, cx);
                    this.open_chart_popup(ChartPopup::Graphics, cx);
                }
                false => this.close_graphics_popup(window, cx),
            });
        })
        .trigger(trigger);
    if !this.popup_shows(ChartPopup::Graphics) {
        return popover;
    }
    let p = MoonPalette::active(cx);
    let cfg = this.graphics_cfg(cx);
    let snap = this.layout_popup_snapshot(cx);
    let entity = cx.entity();
    // The graphics travel as a kind default; the two book checkboxes have none and are written
    // into the addressed tabs by the same press (`ChartTabs::set_kind_defaults`).
    let values = this.applicable_here(
        vec![
            StackSetting::Graphics(cfg),
            StackSetting::Orderbook(snap.orderbook),
            StackSetting::ShowZone(snap.show_zone),
        ],
        cx,
    );
    let row = crate::chart_tabs::apply_row::render_apply_row(this, id_prefix, values, None, p, cx);
    let book = BookToggles {
        shown: snap.orderbook,
        zone: snap.show_zone,
        width_input: this.book_width_input(),
    };
    popover = popover.content(
        v_flex()
            .gap_2()
            .children(row)
            .child(render_graphics_popup(id_prefix, entity, cfg, book, p, cx)),
    );
    popover
}

#[cfg(test)]
mod tests;
