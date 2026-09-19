//! The "Volumes" popup (the `V` button beside the labels one) configures the chart's two volume
//! indicators, one column each: the VERTICAL volumes — the bottom band, per candle, with Moonbot's
//! bought/sold `Vol` on top of it — and the HORIZONTAL volumes — Moonbot's `HVol`, turnover by
//! price over a trailing window drawn as rows in a zone beside the plot.
//!
//! The band's rows used to live in the graphics popup, whose subject is what the chart draws over
//! the candles. They belong beside the profile: the two are the reference's pair of volume
//! indicators, and they share one colour and one opacity — the profile's rows draw with the band's
//! (`chartdx::data_state::market`), so the one opacity row in the shared frame above the two
//! columns governs both. That shared row is why they are one popup and not two buttons; the kind
//! row beside it writes both indicators' stacked flags for the same reason.
//!
//! Like the popups beside it, these settings are PER TAB and part of `ChartGraphicsCfg`: the
//! target is the tab strip's active tab or the detached window's panel, the tab spec persists them
//! to `charts.json` through `ChartTabSpec::chart_graphics`, and a tab without an override follows
//! the default of its KIND of tab (`chart_tabs::apply_all`). Sharing the struct is what lets the
//! ⧉ row, the settings signature and the engine's normaliser serve this popup unchanged.
//!
//! Controls read stored config on every render and are stateless on purpose (see the graphics
//! popup's module docs for why there are no sliders here).

use gpui::*;
use moon_core::config::{ChartGraphicsCfg, HVOL_TF_MAX_S, HvolSide};
use moon_ui::{
    MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonDropdown, MoonPalette, MoonPopover,
    MoonPopoverPlacement, MoonTooltipView, h_flex, v_flex,
};
use rust_i18n::t;

use super::common::{StackSetting, seg_row};
use super::graphics_popup::{GraphicsPopupHost, nearest, percent_label, write_cfg};
use super::popup_slot::ChartPopup;
use crate::design;
use crate::panels::{
    popup_apply_all_button, popup_close_button, popup_group, popup_group_inset_px, popup_title,
};

/// Selectable price windows, percent of price. `0.1` is Moonbot's shipped `PriceFrame`.
const PRICE_FRAME_PCTS: [f32; 6] = [0.05, 0.1, 0.2, 0.3, 0.5, 1.0];

/// Selectable zone widths, as a fraction of the pane width. `0.2` is the shipped default; `0.05`
/// is `moon_chart::hvol::WIDTH_MIN`, and on a pane narrower than `ZONE_MIN_PX / 0.05` it asks for
/// a zone the layout leaves out rather than draws as a sliver.
const WIDTHS: [f32; 7] = [0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.4];

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

/// Segment widths, in rendered pixels. Each column is as wide as its widest row and no wider,
/// and every other row in it divides that width among its own segments.
///
/// Both columns' widest rows are seven cells at the width the graphics popup uses: the sides
/// band's intervals on the left, the zone widths on the right. The window used to be the widest
/// row by far — `Auto`, nine windows and `Max`, eleven cells — and set the whole popup's width
/// on its own; it is a dropdown now, one line wide, for exactly that reason.
const BAND_ROW_W: f32 = 7.0 * 42.0;
const B_SEG_W2: f32 = BAND_ROW_W / 2.0;
const B_SEG_W3: f32 = BAND_ROW_W / 3.0;
const B_SEG_W6: f32 = BAND_ROW_W / 6.0;
const B_SEG_W7: f32 = BAND_ROW_W / 7.0;
const HVOL_ROW_W: f32 = BAND_ROW_W;
const H_SEG_W2: f32 = HVOL_ROW_W / 2.0;
const H_SEG_W6: f32 = HVOL_ROW_W / 6.0;
const H_SEG_W7: f32 = HVOL_ROW_W / 7.0;

/// Width of the window dropdown's trigger, in design units: the widest choice (`Auto`, `Max`)
/// plus the caret.
const WINDOW_DD_W: f32 = 80.0;

/// Gap between the popup's two columns, in rendered pixels.
///
/// Wider than the gap between rows in a column: the two columns are read as two pages side by
/// side, and a gap no larger than the one between rows made the two frames read as one grid.
const COLUMN_GAP: f32 = 12.0;

/// Width of the shared frame's rows: the two columns and the gap between them, less the one frame
/// inset the shared frame draws around its rows itself.
fn common_row_width(cx: &App) -> f32 {
    BAND_ROW_W + HVOL_ROW_W + COLUMN_GAP + popup_group_inset_px(cx)
}

/// Popup CONTENT width in rendered pixels. `MoonPopover` adds its own padding and border outside it.
///
/// Two columns, each a row plus its frame, and the gap between them.
pub(super) fn content_width(cx: &App) -> Pixels {
    px(BAND_ROW_W + HVOL_ROW_W + 2.0 * popup_group_inset_px(cx) + COLUMN_GAP)
}

/// Label a bucket width in seconds the way the chart's timeframe controls spell one: `5s`, `1m`.
///
/// Delegates to the volume band's own `bucket_label` so the popup and the cursor readout can never
/// spell the same width two ways; a width it cannot name (none on the list) prints its seconds.
fn tf_label(tf_s: u32) -> String {
    moon_chart::volume_bars::bucket_label(f64::from(tf_s) * 1_000.0)
        .unwrap_or_else(|| format!("{tf_s}s"))
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

/// Label a price window in percent of price, trimmed of trailing zeros: `0.1%`, `0.05%`, `1%`.
fn price_frame_label(pct: f32) -> String {
    let text = format!("{pct:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{text}%")
}

/// The side a `(left, transparent)` pair spells.
fn side_of(left: bool, transparent: bool) -> HvolSide {
    match (left, transparent) {
        (false, false) => HvolSide::Right,
        (true, false) => HvolSide::Left,
        (false, true) => HvolSide::RightTransparent,
        (true, true) => HvolSide::LeftTransparent,
    }
}

/// Render popup content by reading the stored values on every render for the stateless controls.
fn render_volumes_popup<T: VolumesPopupHost>(
    id: &str,
    entity: Entity<T>,
    cfg: ChartGraphicsCfg,
    target: (u32, moon_core::config::ChartBucket),
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
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
            B_SEG_W3,
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
            B_SEG_W6,
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
    // --- Shared frame: what both indicators draw with. The opacity is ONE value by design —
    // the profile's rows take the band's (`chartdx::data_state::market`) — and the kind is two
    // stored fields that mean the same thing, written together here so the two pictures agree;
    // a profile whose two values had drifted apart shows the band's and is brought into line by
    // the first click. ---
    let common_w = common_row_width(cx);
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
            common_w / 6.0,
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
    // --- Moonbot's `Vol` on top of either style: the switch, then its interval row, which
    // appears only while it is on so the plain popup keeps the shape it had. ---
    let sides_on = cfg.candle_volume_sides;
    let volume_sides_cb = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-volume-sides")))
            .label(t!("chart.graphics.volume_sides").to_string())
            .checked(sides_on)
            .on_change(move |ch: &bool, _w, app| {
                let v = *ch;
                write_cfg(&entity, app, |c| c.candle_volume_sides = v);
            })
    };
    let volume_kind_row = {
        let entity = entity.clone();
        seg_row(
            format!("{id}-volume-kind"),
            t!("chart.graphics.volume_kind").to_string(),
            vec![
                (
                    t!("chart.graphics.volume_kind_overlay").to_string(),
                    !cfg.candle_volume_stacked,
                ),
                (
                    t!("chart.graphics.volume_kind_stacked").to_string(),
                    cfg.candle_volume_stacked,
                ),
            ],
            common_w / 2.0,
            p,
            cx,
            move |ix, app| {
                write_cfg(&entity, app, |c| {
                    c.candle_volume_stacked = ix == 1;
                    c.hvol_stacked = ix == 1;
                });
            },
        )
    };
    let volume_tf_row = sides_on.then(|| {
        let entity = entity.clone();
        // Exact equality, like the style row: the stored value is already snapped onto this list
        // by `normalize_chart_graphics`, so a segment lights only for the width the band draws.
        let mut labels = vec![(
            t!("chart.graphics.volume_tf_auto").to_string(),
            cfg.candle_volume_tf_s == 0,
        )];
        labels.extend(
            moon_chart::side_volume::SIDE_TF_CHOICES_S
                .iter()
                .map(|s| (tf_label(*s), *s == cfg.candle_volume_tf_s)),
        );
        seg_row(
            format!("{id}-volume-tf"),
            t!("chart.graphics.volume_tf").to_string(),
            labels,
            B_SEG_W7,
            p,
            cx,
            move |ix, app| {
                let v = match ix.checked_sub(1) {
                    None => 0,
                    Some(i) => match moon_chart::side_volume::SIDE_TF_CHOICES_S.get(i) {
                        Some(s) => *s,
                        None => return,
                    },
                };
                write_cfg(&entity, app, |c| c.candle_volume_tf_s = v);
            },
        )
    });
    let volume_scale_pos_row = {
        let entity = entity.clone();
        seg_row(
            format!("{id}-volume-scale-pos"),
            t!("chart.graphics.volume_scale_pos").to_string(),
            vec![
                (
                    t!("chart.graphics.volume_scale_left").to_string(),
                    !cfg.candle_volume_scale_right,
                ),
                (
                    t!("chart.graphics.volume_scale_right").to_string(),
                    cfg.candle_volume_scale_right,
                ),
            ],
            B_SEG_W2,
            p,
            cx,
            move |ix, app| {
                write_cfg(&entity, app, |c| c.candle_volume_scale_right = ix == 1);
            },
        )
    };
    // Where the plot's bottom captions go once the band takes the floor: lifted above it, or
    // printed over the bars on their plates. A per-tab switch beside the band's own controls,
    // because it is the BAND that displaces them; the label editor knows nothing of it.
    let volume_labels_row = {
        let entity = entity.clone();
        seg_row(
            format!("{id}-volume-labels"),
            t!("chart.graphics.volume_labels").to_string(),
            vec![
                (
                    t!("chart.graphics.volume_labels_above").to_string(),
                    !cfg.candle_volume_labels_over,
                ),
                (
                    t!("chart.graphics.volume_labels_over").to_string(),
                    cfg.candle_volume_labels_over,
                ),
            ],
            B_SEG_W2,
            p,
            cx,
            move |ix, app| {
                write_cfg(&entity, app, |c| c.candle_volume_labels_over = ix == 1);
            },
        )
    };
    let volume_scale_row = {
        let entity = entity.clone();
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 2.0))
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text))
                    .child(t!("chart.graphics.candle_volume_scale").to_string()),
            )
            .child(crate::controls::color_picker::ColorPicker::new(
                format!("{id}-volume-scale-{target:?}"),
                cfg.candle_volume_scale,
                move |color, app| {
                    if entity.read(app).spec_key() == target {
                        write_cfg(&entity, app, |c| c.candle_volume_scale = color);
                    }
                },
            ))
    };

    let hvol_on = cfg.hvol_enabled;
    let enabled_cb = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-hvol-enabled")))
            .label(t!("chart.volumes.hvol_enabled").to_string())
            .checked(hvol_on)
            .on_change(move |ch: &bool, _w, app| {
                let v = *ch;
                write_cfg(&entity, app, |c| c.hvol_enabled = v);
            })
    };
    // The remaining rows appear only while the zone is on, so the popup keeps its shape when
    // there is nothing to configure — the graphics popup does the same with the sides rows.
    // The window as a dropdown on one line — caption left, trigger right — rather than a row of
    // eleven cells: that row alone set the popup's width. Exact equality for the mark: the stored
    // value is already snapped onto the list by `normalize_chart_graphics`, so one item is
    // checked for the window the zone covers.
    let window_row = hvol_on.then(|| {
        let entity = entity.clone();
        let label_of = |tf_s: u32| -> String {
            if tf_s == 0 {
                t!("chart.graphics.volume_tf_auto").to_string()
            } else if tf_s == HVOL_TF_MAX_S {
                t!("chart.hvol.caption_max").to_string()
            } else {
                moon_chart::hvol::tf_label(tf_s).unwrap_or_else(|| format!("{tf_s}s"))
            }
        };
        let choices = std::iter::once(0)
            .chain(moon_chart::hvol::HVOL_TF_CHOICES_S.iter().copied())
            .chain(std::iter::once(HVOL_TF_MAX_S));
        let items = crate::panels::radio_items(
            choices.map(|tf_s| {
                (
                    tf_s,
                    SharedString::from(format!("{id}-hvol-window-{tf_s}")),
                    SharedString::from(label_of(tf_s)),
                )
            }),
            cfg.hvol_tf_s,
            crate::panels::RadioMark::Check,
            move |app, tf_s: u32| {
                write_cfg(&entity, app, |c| c.hvol_tf_s = tf_s);
            },
        );
        let dd = MoonDropdown::new(SharedString::from(format!("{id}-hvol-window")))
            .label(label_of(cfg.hvol_tf_s))
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::density(cx))
            .trigger_width_scaled(WINDOW_DD_W)
            .menu_width_scaled(WINDOW_DD_W + 24.0)
            .items(items);
        h_flex()
            .w_full()
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .child(
                div()
                    .flex_1()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text))
                    .child(t!("chart.volumes.hvol_window").to_string()),
            )
            .child(dd)
    });
    let price_frame_row = hvol_on.then(|| {
        let entity = entity.clone();
        let current = nearest(&PRICE_FRAME_PCTS, cfg.hvol_price_frame_pct);
        seg_row(
            format!("{id}-hvol-price-frame"),
            t!("chart.volumes.hvol_price_frame").to_string(),
            PRICE_FRAME_PCTS
                .iter()
                .enumerate()
                .map(|(index, v)| (price_frame_label(*v), index == current))
                .collect(),
            H_SEG_W6,
            p,
            cx,
            move |ix, app| {
                if let Some(v) = PRICE_FRAME_PCTS.get(ix) {
                    let v = *v;
                    write_cfg(&entity, app, |c| c.hvol_price_frame_pct = v);
                }
            },
        )
    });
    let width_row = hvol_on.then(|| {
        let entity = entity.clone();
        let current = nearest(&WIDTHS, cfg.hvol_width);
        seg_row(
            format!("{id}-hvol-width"),
            t!("chart.volumes.hvol_width").to_string(),
            WIDTHS
                .iter()
                .enumerate()
                .map(|(index, v)| (percent_label(*v), index == current))
                .collect(),
            H_SEG_W7,
            p,
            cx,
            move |ix, app| {
                if let Some(v) = WIDTHS.get(ix) {
                    let v = *v;
                    write_cfg(&entity, app, |c| c.hvol_width = v);
                }
            },
        )
    });
    // Moonbot's one `Disp. vol` list as two switches: which edge of the zone the cursor readout
    // prints at, and whether the zone's captions keep their backing plates. The rows' colours and opacity are the
    // bottom band's (the graphics popup's rows), so there is no opacity row here.
    let side_row = hvol_on.then(|| {
        let entity = entity.clone();
        let left = cfg.hvol_side.is_left();
        let transparent = cfg.hvol_side.is_transparent();
        seg_row(
            format!("{id}-hvol-side"),
            t!("chart.volumes.hvol_side").to_string(),
            vec![
                (t!("chart.graphics.volume_scale_right").to_string(), !left),
                (t!("chart.graphics.volume_scale_left").to_string(), left),
            ],
            H_SEG_W2,
            p,
            cx,
            move |ix, app| {
                write_cfg(&entity, app, |c| {
                    c.hvol_side = side_of(ix == 1, transparent)
                });
            },
        )
    });
    let backdrop_row = hvol_on.then(|| {
        let entity = entity.clone();
        let left = cfg.hvol_side.is_left();
        let transparent = cfg.hvol_side.is_transparent();
        seg_row(
            format!("{id}-hvol-backdrop"),
            t!("chart.volumes.hvol_backdrop").to_string(),
            vec![
                (
                    t!("chart.volumes.hvol_backdrop_on").to_string(),
                    !transparent,
                ),
                (
                    t!("chart.volumes.hvol_backdrop_off").to_string(),
                    transparent,
                ),
            ],
            H_SEG_W2,
            p,
            cx,
            move |ix, app| {
                write_cfg(&entity, app, |c| c.hvol_side = side_of(left, ix == 1));
            },
        )
    });
    // Over the plot, like the bottom band, instead of in a zone carved out beside it. The theme's
    // zone backdrop goes unused then, which the label says.
    // What "over the plot" means rides as a tooltip rather than a caption under the box. It is
    // invisible today — a tooltip inside a `MoonPopover` paints under it (the Tooltip entry in
    // docs-internal/FORK_BUGS.md) — and comes into view with the fork's fix; nothing here has to
    // change for it.
    let overlay_cb = hvol_on.then(|| {
        let entity = entity.clone();
        let tip = t!("chart.volumes.hvol_overlay_hint").to_string();
        div()
            .id(SharedString::from(format!("{id}-hvol-overlay-tip")))
            .w_full()
            .tooltip(move |_w, cx| cx.new(|_| MoonTooltipView::new(tip.clone())).into())
            .child(
                MoonCheckbox::new(SharedString::from(format!("{id}-hvol-overlay")))
                    .label(t!("chart.volumes.hvol_overlay").to_string())
                    .checked(cfg.hvol_overlay)
                    .on_change(move |ch: &bool, _w, app| {
                        let v = *ch;
                        write_cfg(&entity, app, |c| c.hvol_overlay = v);
                    }),
            )
    });
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
                .child(popup_title(t!("chart.volumes.title"), p, cx))
                .child(apply_all_btn)
                .child(popup_close_button(
                    SharedString::from(format!("{id}-close")),
                    {
                        let entity = entity.clone();
                        move |_, _w, app: &mut App| {
                            entity.update(app, |this, cx| this.close_volumes_popup(cx));
                        }
                    },
                )),
        )
        .child(
            popup_group("frame-common", t!("chart.volumes.frame_common")).child(
                v_flex()
                    .w(px(common_w))
                    .gap(design::ui_px(cx, 6.0))
                    .child(volume_alpha_row)
                    .child(volume_kind_row),
            ),
        )
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap(px(COLUMN_GAP))
                .child(
                    popup_group("frame-band", t!("chart.volumes.frame_band")).child(
                        v_flex()
                            .w(px(BAND_ROW_W))
                            .gap(design::ui_px(cx, 6.0))
                            .child(volume_style_row)
                            .child(volume_sides_cb)
                            .children(volume_tf_row)
                            .child(volume_height_row)
                            .child(volume_scale_pos_row)
                            .child(volume_labels_row)
                            .child(volume_scale_row),
                    ),
                )
                .child(
                    popup_group("frame-hvol", t!("chart.volumes.frame_hvol")).child(
                        v_flex()
                            .w(px(HVOL_ROW_W))
                            .gap(design::ui_px(cx, 6.0))
                            .child(enabled_cb)
                            .children(window_row)
                            .children(price_frame_row)
                            .children(width_row)
                            .children(overlay_cb)
                            .children(side_row)
                            .children(backdrop_row),
                    ),
                ),
        )
        .into_any_element()
}

/// Host for the volumes popup in either the tab strip or a detached-window header.
///
/// Everything it edits lives in `ChartGraphicsCfg`, so the graphics popup's host — the target's
/// override, its normalised effective value, the apply path — serves this one unchanged; only the
/// slot it occupies is its own.
pub(super) trait VolumesPopupHost: GraphicsPopupHost {
    /// Close the popup. Ownership is checked by the slot; see `GraphicsPopupHost`.
    fn close_volumes_popup(&mut self, cx: &mut Context<Self>) {
        self.close_chart_popup(ChartPopup::Volumes, cx);
    }
}

impl<T: GraphicsPopupHost> VolumesPopupHost for T {}

/// Build the volumes popup: a `MoonPopover` anchored to the button that opens it.
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
pub(super) fn volumes_popup_host<T: VolumesPopupHost>(
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
        // The window dropdown's menu paints in its OWN deferred layer outside this popover's box,
        // and `on_mouse_down_out` is bounds-based in the CAPTURE phase: the click that picks a
        // window would read as "outside" and shut the popup before the pick lands. The labels
        // popup makes the same trade for the same reason (see its host and the Popover entry in
        // docs-internal/FORK_BUGS.md): the ✕, the toolbar button and a press on a neighbouring
        // settings button — every popup on a host shares one slot — are the ways out.
        .overlay_closable(false)
        .open(this.popup_shows(ChartPopup::Volumes))
        .on_open_change(move |open, _window, app| {
            open_entity.update(app, |this, cx| {
                this.report_chart_popup(ChartPopup::Volumes, open, cx)
            });
        })
        .trigger(trigger);
    if !this.popup_shows(ChartPopup::Volumes) {
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
    popover = popover.content(v_flex().gap_2().children(row).child(render_volumes_popup(
        id_prefix,
        entity,
        cfg,
        this.spec_key(),
        p,
        cx,
    )));
    popover
}

#[cfg(test)]
mod tests;
