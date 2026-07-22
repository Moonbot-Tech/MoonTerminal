//! The three 'By time' heatmap sliders below the row grid: a colored track (average
//! profit per cell 🟢/🟠) + two draggable handles (from/to) + a highlight of the
//! selected span. Like a video trimmer.
//!   - 'Weekly'  (field 0): 168 day×hour cells; drives `WorkingWeekTime` (minute of week).
//!   - 'Day'     (field 1): 24 hour-of-day cells; drives WorkingTime (day mode).
//!   - 'In hour' (field 2): 60 minute-of-hour cells; drives WorkingTime (hour mode).
//! 'Day' and 'In hour' share the single WorkingTime field → the active one clears the
//! other (mutually exclusive).
//!
//! The value lives in the ROW's fields (the source of truth): the full range = 'no
//! restriction' = empty. Dragging reads the captured `slider_track` bounds (via `canvas`)
//! and writes the fields; mouse movement is caught by an overlay in `strat_time` so the
//! cursor is not lost once it leaves the track.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::super::super::calendar::split_i18n;
use super::grid::{CHECK_COL, NAME_COL};
use super::state::{WEEK_MIN, fmt_min, fmt_week_ep, parse_moh, parse_time};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::SliderProfiles;

/// Cell color from the average profit `v` and the maximum `cmax` (🟢 plus / 🟠 minus).
fn cell_color(v: f32, cmax: f32, p: MoonPalette) -> Hsla {
    let a = (v.abs() / cmax * 0.9).min(0.9);
    if v > 0.0 {
        moon_alpha(p.green, a)
    } else if v < 0.0 {
        moon_alpha(p.orange, a)
    } else {
        moon_alpha(p.panel_high, 0.4)
    }
}

/// Heatmap strip: `data` (per-cell profit) laid out in `n≤maxcells` segments, EACH a
/// 2-stop gradient from the left neighbour's color to its own → smooth transitions with
/// no seams (the fork's gradient supports only 2 stops). Large axes (day, 1440) are
/// downsampled.
fn heat_gradient_row(data: &[f32], maxcells: usize, p: MoonPalette) -> Div {
    let len = data.len().max(1);
    let n = len.min(maxcells.max(1));
    let bucket = |i: usize| -> f32 {
        let a = i * len / n;
        let b = ((i + 1) * len / n).max(a + 1).min(len);
        data[a..b].iter().sum::<f32>() / (b - a) as f32
    };
    // Normalize by the DISPLAYED (bucketed) cells, NOT the raw data: a downsampled axis (day:
    // 1440→288, averaged) would otherwise be measured against an un-averaged single-minute peak
    // and come out systematically faint. Averaging first, then taking the max, keeps every axis
    // reaching full intensity at its own brightest cell.
    let cells: Vec<f32> = (0..n).map(bucket).collect();
    let cmax = cells.iter().fold(0f32, |m, &v| m.max(v.abs())).max(1e-9);
    let sign = |v: f32| {
        if v > 0.0 {
            1i8
        } else if v < 0.0 {
            -1
        } else {
            0
        }
    };
    let mut row = h_flex().size_full();
    let mut prev_c = cell_color(cells[0], cmax, p);
    let mut prev_s = sign(cells[0]);
    for &v in &cells {
        let c = cell_color(v, cmax, p);
        let s = sign(v);
        // Blend ONLY within one sign (🟢→🟢 / 🟠→🟠); on a sign change cut hard,
        // otherwise a green↔orange gradient passes through muddy yellow.
        let from = if s != 0 && s == prev_s { prev_c } else { c };
        row = row.child(div().flex_1().h_full().bg(linear_gradient(
            90.0,
            linear_color_stop(from, 0.0),
            linear_color_stop(c, 1.0),
        )));
        prev_c = c;
        prev_s = s;
    }
    row
}

/// Row of tick labels above the track (day/hour/minute), aligned to the segments' left edges.
fn tick_row(
    labels: Vec<String>,
    boundaries: bool,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> Div {
    let base = h_flex()
        .w_full()
        .flex_none()
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_muted));
    if boundaries {
        // Labels on the BOUNDARIES (0,3,…,24 / 0,5,…,60): justify_between = exactly at k/(n-1).
        base.justify_between()
            .children(labels.into_iter().map(|l| div().flex_none().child(l)))
    } else {
        // Labels PER SEGMENT (weekdays): centered within their own 1/n share.
        let mut row = base;
        for l in labels {
            row = row.child(div().flex_1().min_w_0().truncate().text_center().child(l));
        }
        row
    }
}

/// Profile cells of slider `field` (0 week×hour, 1 hour of day, 2 minute of hour).
fn slider_cells(pr: &SliderProfiles, field: usize) -> &[f32] {
    match field {
        0 => &pr.week,
        1 => &pr.day,
        _ => &pr.hour,
    }
}

/// Vertical slider handle at fraction `frac` (0..1) of the track's width.
fn handle_bar(frac: f32, p: MoonPalette, cx: &Context<AnalyticsView>) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(relative(frac))
        .w(design::ui_px(cx, 2.0))
        .bg(moon(p.text))
}

impl AnalyticsView {
    /// Maximum of slider `field` in its own units (minute of week / of day / of hour).
    pub(in crate::analytics::tuner) fn slider_max(field: usize) -> u16 {
        match field {
            0 => WEEK_MIN - 1,
            1 => 1439,
            _ => 59,
        }
    }

    /// Current range of slider `field` from the row's fields; empty → the full range.
    fn slider_range(&self, field: usize) -> (u16, u16) {
        match field {
            0 => self.time_tuner.week_span(0).unwrap_or((0, WEEK_MIN - 1)),
            1 => {
                let (f, t) = &self.time_tuner.bounds[0][1];
                (parse_time(f).unwrap_or(0), parse_time(t).unwrap_or(1439))
            }
            _ => {
                let (f, t) = &self.time_tuner.bounds[0][2];
                (
                    parse_moh(f).unwrap_or(0) as u16,
                    parse_moh(t).unwrap_or(59) as u16,
                )
            }
        }
    }

    /// Does slider `field` hold NO window — i.e. the field restricts nothing? Blank bounds,
    /// but also a span covering the whole axis: that is the same "no restriction" rule
    /// `week_span`/`tod` apply before emitting a value, and exactly what `set_slider_range`
    /// writes back as a cleared field. Unparseable bounds fall into it too, since
    /// `slider_range` widens them to the full axis.
    fn slider_no_window(&self, field: usize) -> bool {
        let (from, to) = &self.time_tuner.bounds[0][field];
        (from.trim().is_empty() && to.trim().is_empty())
            || self.slider_range(field) == (0, Self::slider_max(field))
    }

    /// Value in the slider's units from the mouse X (via the captured track bounds).
    fn slider_value_at(&self, field: usize, x: Pixels) -> u16 {
        let Some(b) = self.time_tuner.slider_track[field] else {
            return 0;
        };
        let w = f32::from(b.size.width);
        if w <= 0.0 {
            return 0;
        }
        let frac = ((f32::from(x) - f32::from(b.origin.x)) / w).clamp(0.0, 1.0);
        (frac * Self::slider_max(field) as f32).round() as u16
    }

    /// Write the slider's range into the row's fields. A full range → clear it ('no
    /// restriction'). The WT rows (1/2) are mutually exclusive. Recomputes the KPIs.
    fn set_slider_range(&mut self, field: usize, from: u16, to: u16, cx: &mut Context<Self>) {
        let max = Self::slider_max(field);
        let (from, to) = (from.min(to), from.max(to));
        let full = from == 0 && to == max;
        if full {
            self.clear_field(0, field);
        } else {
            let (a, b) = match field {
                0 => (fmt_week_ep(from, false), fmt_week_ep(to, true)),
                1 => (fmt_min(from), fmt_min(to)),
                _ => (from.to_string(), to.to_string()),
            };
            self.set_v1_cell(field, a, b);
        }
        // 'Day'↔'In hour' share the single WorkingTime field: the active row clears the
        // other. Only on a real (non-full) window — a full range means "no restriction",
        // which must not wipe the other view's value.
        if !full && field == 1 {
            self.clear_field(0, 2);
        }
        if !full && field == 2 {
            self.clear_field(0, 1);
        }
        // NO reload_time: while dragging we only move the fields/strip; the KPIs are
        // recomputed ONCE on release (`slider_release`) — otherwise a storm of SQL queries.
        cx.notify();
    }

    /// Finish a slider drag: clear the flag and recompute the KPIs ONCE (on release).
    fn slider_release(&mut self, cx: &mut Context<Self>) {
        if self.time_tuner.slider_drag.take().is_some() {
            self.reload_time(cx);
            cx.notify();
        }
    }

    /// Move the dragged edge (`is_from`) of slider `field` to `value`.
    pub(in crate::analytics::tuner) fn slider_drag_to(
        &mut self,
        field: usize,
        is_from: bool,
        value: u16,
        cx: &mut Context<Self>,
    ) {
        let (mut from, mut to) = self.slider_range(field);
        if is_from {
            from = value.min(to);
        } else {
            to = value.max(from);
        }
        self.set_slider_range(field, from, to, cx);
    }

    /// LMB down on the track: pick the nearest handle, start the drag and move it at once.
    fn slider_mouse_down(&mut self, field: usize, x: Pixels, cx: &mut Context<Self>) {
        let value = self.slider_value_at(field, x);
        let (from, to) = self.slider_range(field);
        let is_from = (value as i32 - from as i32).abs() <= (value as i32 - to as i32).abs();
        self.time_tuner.slider_drag = Some((field, is_from));
        self.slider_drag_to(field, is_from, value, cx);
    }

    /// The block of three sliders below the row grid.
    pub(in crate::analytics::tuner) fn time_sliders(
        &mut self,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let prof = self.time_tuner.slider.clone();
        let mut col = v_flex()
            .w_full()
            .flex_none()
            .gap(design::ui_px(cx, 5.0))
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 6.0))
            .border_t_1()
            .border_color(moon(p.border));
        for field in 0..3usize {
            col = col.child(self.time_slider_row(field, prof.as_deref(), p, cx));
        }
        col.into_any_element()
    }

    /// A single slider row: the label + the colored track with its handles.
    fn time_slider_row(
        &self,
        field: usize,
        prof: Option<&SliderProfiles>,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let max = Self::slider_max(field) as f32;
        let (from, to) = self.slider_range(field);
        let (ff, tf) = (from as f32 / max, to as f32 / max);
        // Region without inversion (a wrapping span from>to is entered by hand — we don't
        // overlap-draw it).
        let (rl, rr) = (ff.min(tf), ff.max(tf));
        // Shade a strip that is not in effect: either its field holds no window (it
        // restricts nothing) or it is the unused half of the Day/Hour pair, which shares
        // one WorkingTime field.
        let no_window = self.slider_no_window(field);
        let active = self.time_tuner.active_wt();
        let dim = no_window || matches!((field, active), (1, Some(2)) | (2, Some(1)));
        // Tiling cascade: the mask of the active FINER field is projected onto THIS track —
        // 'In hour' → onto 'Day' AND 'Weekly' (within every hour); 'Day' → onto 'Weekly'
        // (within every day). We SHADE WHAT IS EXCLUDED (outside the window) — what is
        // active keeps showing the heatmap.
        let total = Self::slider_max(field) as u32 + 1;
        let dark_bands: Vec<(u16, u16)> = match active {
            Some(2) if field != 2 => {
                let (hf, ht) = self.slider_range(2);
                let mut v = Vec::new();
                for h in 0..(total / 60) as u16 {
                    let base = h * 60;
                    if hf > 0 {
                        v.push((base, base + hf - 1)); // before the hour window
                    }
                    if ht < 59 {
                        v.push((base + ht + 1, base + 59)); // after the hour window
                    }
                }
                v
            }
            Some(1) if field == 0 => {
                let (df, dt) = self.slider_range(1);
                let mut v = Vec::new();
                for d in 0..7u16 {
                    let base = d * 1440;
                    if df > 0 {
                        v.push((base, base + df - 1)); // before the day window
                    }
                    if dt < 1439 {
                        v.push((base + dt + 1, base + 1439)); // after the day window
                    }
                }
                v
            }
            _ => vec![],
        };
        let label_key = match field {
            0 => "analytics.time.field_week",
            1 => "analytics.time.field_day",
            _ => "analytics.time.field_hour",
        };
        // Gradient cells; the day (1440 minutes) gets 5 minutes per cell = 288.
        let maxcells = match field {
            0 => 168usize,
            1 => 288,
            _ => 60,
        };
        let empty = [0.0f32];
        let data = prof.map(|pr| slider_cells(pr, field)).unwrap_or(&empty);
        let cells_row = heat_gradient_row(data, maxcells, p);
        // Ticks above the track: week — days (per segment); day — hours 0..24 (every 3);
        // hour — minutes 0..60 (every 5). `bound` = labels on the boundaries (justify_between).
        let (ticks, tick_bound): (Vec<String>, bool) = match field {
            0 => (split_i18n(t!("analytics.heat.weekdays").to_string()), false),
            1 => ((0..=8u16).map(|k| (k * 3).to_string()).collect(), true),
            _ => ((0..=12u16).map(|k| (k * 5).to_string()).collect(), true),
        };

        let view = cx.entity();
        let mut track = div()
            .id(SharedString::from(format!("tt-sl-{field}")))
            .relative()
            .w_full()
            .h(design::ui_px(cx, 30.0))
            .rounded(design::ui_px(cx, 3.0))
            .overflow_hidden()
            .cursor(CursorStyle::OpenHand)
            .bg(moon(p.panel_high))
            .child(cells_row);
        // Shade the mask's EXCLUDED intervals (the tiling cascade) — dark hatching;
        // the active time stays a heatmap. Drawn UNDER this row's selection/handles.
        let total_f = total as f32;
        for (a, b) in &dark_bands {
            let l = *a as f32 / total_f;
            let w = (*b as f32 - *a as f32 + 1.0) / total_f;
            track = track.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(l))
                    .w(relative(w))
                    .bg(moon_alpha(p.shell, 0.72)),
            );
        }
        // No window → no selection to draw: a frame over the full range would claim a
        // restriction the field does not hold. The track stays draggable — pressing it
        // picks the nearer edge and creates the window.
        if !no_window {
            track = track
                // This row's own selection (week span / window) — ON TOP of the shading,
                // border only.
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(rl))
                        .right(relative(1.0 - rr))
                        .border_2()
                        .border_color(moon(p.accent)),
                )
                // The two handles (from/to).
                .child(handle_bar(ff, p, cx))
                .child(handle_bar(tf, p, cx));
        }
        let track = track
            // Capture the track's bounds so the mouse X can be turned into a value.
            .child(
                canvas(
                    move |bounds, _window, app| {
                        view.update(app, |this, _| {
                            this.time_tuner.slider_track[field] = Some(bounds)
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _w, cx| {
                    this.slider_mouse_down(field, e.position.x, cx);
                }),
            );

        h_flex()
            .w_full()
            .items_end()
            // Gap 4 (not 6) plus a CHECK_COL lead-in: together they reproduce the grid
            // row's checkbox column, so the slider labels stay under the field names.
            .gap(design::ui_px(cx, 4.0))
            .opacity(if dim { 0.5 } else { 1.0 })
            .child(div().w(design::ui_px(cx, CHECK_COL)).flex_none())
            .child(
                // Matches the grid's field-name column so the slider labels line up under it.
                div()
                    .w(design::font_w_px(cx, NAME_COL))
                    .flex_none()
                    .truncate()
                    .pb(design::ui_px(cx, 3.0))
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_soft))
                    .child(t!(label_key).to_string()),
            )
            .child(
                // The ticks above the track + the track itself (matched in width).
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(design::ui_px(cx, 1.0))
                    .child(tick_row(ticks, tick_bound, p, cx))
                    .child(track),
            )
            .into_any_element()
    }

    /// A transparent overlay over the whole 'By time' body for the duration of a slider
    /// drag: it catches mouse movement (even off the track) and the release. Drawn only
    /// while dragging.
    pub(in crate::analytics::tuner) fn slider_drag_overlay(
        &self,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        self.time_tuner.slider_drag?;
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .cursor(CursorStyle::ClosedHand)
                .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _w, cx| {
                    if let Some((field, is_from)) = this.time_tuner.slider_drag {
                        let v = this.slider_value_at(field, e.position.x);
                        this.slider_drag_to(field, is_from, v, cx);
                    }
                }))
                // Release inside the body AND outside it (`_out`, not hover-gated) — otherwise
                // a release beyond the overlay/window would leave the drag 'stuck'.
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _e, _w, cx| this.slider_release(cx)),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _e, _w, cx| this.slider_release(cx)),
                )
                .into_any_element(),
        )
    }
}
