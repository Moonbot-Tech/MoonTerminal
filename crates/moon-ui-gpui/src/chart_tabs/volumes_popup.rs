//! The "Volumes" popup (the `V` button beside the labels one) configures the chart's volume
//! indicators. Today that is the HORIZONTAL volumes — Moonbot's `HVol`, turnover by price over a
//! trailing window drawn as rows in a zone beside the plot; the bottom band's own rows still live
//! in the graphics popup and are to move here next.
//!
//! Like the popups beside it, these settings are PER TAB and part of `ChartGraphicsCfg`: the
//! target is the tab strip's active tab or the detached window's panel, the tab spec persists them
//! to `charts.json` through `ChartTabSpec::chart_graphics`, and a tab without an override follows
//! the default of its KIND of tab (`chart_tabs::apply_all`). Sharing the struct is what lets the
//! ⧉ row, the settings signature and the engine's normaliser serve this popup unchanged.
//!
//! Controls read stored config on every render and are stateless on purpose (see the graphics
//! popup's module docs for why there are no sliders here). Colour and opacity are not here either:
//! the rows draw with the bottom band's, as the reference draws its two volume indicators alike.

use gpui::*;
use moon_core::config::{ChartGraphicsCfg, HVOL_TF_MAX_S, HvolSide};
use moon_ui::{MoonCheckbox, MoonPalette, MoonPopover, MoonPopoverPlacement, h_flex, v_flex};
use rust_i18n::t;

use super::common::{StackSetting, seg_row};
use super::graphics_popup::GraphicsPopupHost;
use super::popup_slot::ChartPopup;
use crate::design;
use crate::panels::{
    popup_apply_all_button, popup_close_button, popup_group, popup_group_inset_px, popup_title,
};

/// Selectable price windows, percent of price. `0.1` is Moonbot's shipped `PriceFrame`.
const PRICE_FRAME_PCTS: [f32; 6] = [0.05, 0.1, 0.2, 0.3, 0.5, 1.0];

/// Selectable zone widths, as a fraction of the pane width. `0.2` is the shipped default.
const WIDTHS: [f32; 6] = [0.1, 0.15, 0.2, 0.25, 0.3, 0.4];

/// Segment widths, in rendered pixels. The window row is the widest — `Auto`, nine windows and
/// `Max` — so the content width is that row's, and every other row divides it among its own
/// segments.
const WINDOW_SEGMENTS: f32 = (moon_chart::hvol::HVOL_TF_CHOICES_S.len() + 2) as f32;
const SEG_W_WINDOW: f32 = 34.0;
const ROW_W: f32 = WINDOW_SEGMENTS * SEG_W_WINDOW;
const SEG_W2: f32 = ROW_W / 2.0;
const SEG_W6: f32 = ROW_W / 6.0;

/// Popup CONTENT width in rendered pixels. `MoonPopover` adds its own padding and border outside it.
pub(super) fn content_width(cx: &App) -> Pixels {
    px(ROW_W + popup_group_inset_px(cx))
}

/// Index of the step nearest a stored value; see the graphics popup's twin for why nearest.
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

/// Label a 0..1 fraction as whole percent.
fn percent_label(v: f32) -> String {
    format!("{}%", (v * 100.0).round())
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

/// Edit the target's config by loading its current value, mutating it, and applying it to the tab.
fn write_cfg<T: VolumesPopupHost>(
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
fn render_volumes_popup<T: VolumesPopupHost>(
    id: &str,
    entity: Entity<T>,
    cfg: ChartGraphicsCfg,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
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
    let window_row = hvol_on.then(|| {
        let entity = entity.clone();
        // Exact equality: the stored value is already snapped onto the list by
        // `normalize_chart_graphics`, so a segment lights only for the window the zone covers.
        let mut labels = vec![(
            t!("chart.graphics.volume_tf_auto").to_string(),
            cfg.hvol_tf_s == 0,
        )];
        labels.extend(moon_chart::hvol::HVOL_TF_CHOICES_S.iter().map(|s| {
            (
                moon_chart::hvol::tf_label(*s).unwrap_or_else(|| format!("{s}s")),
                *s == cfg.hvol_tf_s,
            )
        }));
        labels.push((
            t!("chart.hvol.caption_max").to_string(),
            cfg.hvol_tf_s == HVOL_TF_MAX_S,
        ));
        seg_row(
            format!("{id}-hvol-window"),
            t!("chart.volumes.hvol_window").to_string(),
            labels,
            SEG_W_WINDOW,
            p,
            cx,
            move |ix, app| {
                let choices = moon_chart::hvol::HVOL_TF_CHOICES_S;
                let v = match ix {
                    0 => 0,
                    i if i == choices.len() + 1 => HVOL_TF_MAX_S,
                    i => match choices.get(i - 1) {
                        Some(s) => *s,
                        None => return,
                    },
                };
                write_cfg(&entity, app, |c| c.hvol_tf_s = v);
            },
        )
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
            SEG_W6,
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
            SEG_W6,
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
            SEG_W2,
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
            SEG_W2,
            p,
            cx,
            move |ix, app| {
                write_cfg(&entity, app, |c| c.hvol_side = side_of(left, ix == 1));
            },
        )
    });
    let kind_row = hvol_on.then(|| {
        let entity = entity.clone();
        seg_row(
            format!("{id}-hvol-kind"),
            t!("chart.graphics.volume_kind").to_string(),
            vec![
                (
                    t!("chart.graphics.volume_kind_overlay").to_string(),
                    !cfg.hvol_stacked,
                ),
                (
                    t!("chart.graphics.volume_kind_stacked").to_string(),
                    cfg.hvol_stacked,
                ),
            ],
            SEG_W2,
            p,
            cx,
            move |ix, app| {
                write_cfg(&entity, app, |c| c.hvol_stacked = ix == 1);
            },
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
            popup_group("frame-hvol", t!("chart.volumes.frame_hvol")).child(
                v_flex()
                    .w(px(ROW_W))
                    .gap(design::ui_px(cx, 6.0))
                    .child(enabled_cb)
                    .children(window_row)
                    .children(price_frame_row)
                    .children(width_row)
                    .children(side_row)
                    .children(backdrop_row)
                    .children(kind_row),
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
    popover = popover.content(
        v_flex()
            .gap_2()
            .children(row)
            .child(render_volumes_popup(id_prefix, entity, cfg, p, cx)),
    );
    popover
}

#[cfg(test)]
mod tests;
