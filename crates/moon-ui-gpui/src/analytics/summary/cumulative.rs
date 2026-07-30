//! Cumulative-profit chart of the "Summary" tab: the period total as a filled area with a
//! stroked line on top, the per-core cumulative curves as thin lines inside it, thin column
//! separators from the baseline up to the sum line, and dated X ticks.
//!
//! Only the SWING points carry a label — the troughs and the peaks between them, each
//! showing the running total at that moment. The last bucket is deliberately left bare: it
//! is the period total, which the card header prints. Labelling every bucket turned the
//! chart into a wall of text, and the per-bucket detail belongs in the hover popup: it
//! lists that date's running total per core, plus the number of trades.

use gpui::*;
use moon_ui::{h_flex, v_flex, MoonPalette};

use super::super::AnalyticsView;
use super::charts::{bucket_label, bucket_popup, core_color, muted_caption, PopupMode, CHART_H};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::{CoreSeries, DayPoint};

/// Per-core lines drawn inside the cumulative area, picked by ABSOLUTE contribution. It is
/// a CAP, not a filter: when it bites, the card header says how many of how many are drawn
/// — with 200+ cores every extra path is per-frame canvas work for a line nobody can tell
/// apart. The hover popup has its own, larger limit (`POPUP_ROWS`), so a core with no line
/// can still be read there.
pub(super) const MAX_CORE_LINES: usize = 12;
/// Width of one swing label ("+12345.67"), in font units.
const LABEL_W: f32 = 62.0;
/// A swing is confirmed once the curve retraces this share of the TOTAL's own range.
/// Smaller → every wiggle becomes a label; larger → only the biggest moves survive.
const SWING_FRAC: f32 = 0.07;
/// Labels the chart will show at most. A fixed fraction cannot bound the count on its own:
/// a jagged curve can turn at every bucket and every one of those turns is genuine, so the
/// threshold is backed off until the labels fit instead of drawing a wall of text.
const MAX_SWING_LABELS: usize = 14;
/// Date ticks under the chart (first and last always included).
const X_TICKS: usize = 6;
/// Minimum pixel spacing for the column separators — below it they merge into a wash.
const MIN_SEP_STEP: f32 = 3.0;

/// Swing points of a curve — the indices worth labelling. A running extremum is CONFIRMED
/// only once the curve turns back from it by more than `thresh`, which is what keeps a
/// jittery series from producing a label per bucket.
///
/// The LAST bucket is never labelled: its value is the period total, which the card header
/// already prints, and a label there lands on top of the final swing beside it.
fn swing_points(pts: &[f32], thresh: f32) -> Vec<usize> {
    let n = pts.len();
    if n < 3 {
        return (0..n.saturating_sub(1)).collect();
    }
    let mut out: Vec<usize> = Vec::new();
    let mut ext = 0usize;
    let mut up: Option<bool> = None;
    for i in 1..n {
        let v = pts[i];
        match up {
            // Direction not yet known: wait for the first move big enough to call it.
            None => {
                if (v - pts[ext]).abs() > thresh {
                    up = Some(v > pts[ext]);
                    out.push(ext);
                    ext = i;
                }
            }
            Some(true) => {
                if v >= pts[ext] {
                    ext = i;
                } else if pts[ext] - v > thresh {
                    out.push(ext);
                    ext = i;
                    up = Some(false);
                }
            }
            Some(false) => {
                if v <= pts[ext] {
                    ext = i;
                } else if v - pts[ext] > thresh {
                    out.push(ext);
                    ext = i;
                    up = Some(true);
                }
            }
        }
    }
    // The pending extremum is a real turn the curve has not yet retraced from — keep it,
    // unless it IS the last bucket, which carries no label.
    if ext != n - 1 && out.last() != Some(&ext) {
        out.push(ext);
    }
    out
}

/// Which buckets of the total curve get a label.
///
/// The threshold is a share of the curve's OWN range — not of the chart's display span,
/// which the per-core lines stretch: measuring the total's turns against a core's amplitude
/// silently collapses the labels to the two ends. It is then backed off until the labels
/// fit, because no fixed fraction can bound their number on its own — a jagged curve turns
/// at every bucket and each of those turns is genuine.
fn swing_labels(pts: &[f32]) -> Vec<usize> {
    let hi = pts.iter().copied().fold(f32::MIN, f32::max);
    let lo = pts.iter().copied().fold(f32::MAX, f32::min);
    let mut thresh = (hi - lo).max(1e-6) * SWING_FRAC;
    let mut out = swing_points(pts, thresh);
    // Bounded: 1.8^8 ≈ 110× the starting fraction is past the whole range, which leaves
    // the two ends and terminates.
    for _ in 0..8 {
        if out.len() <= MAX_SWING_LABELS {
            break;
        }
        thresh *= 1.8;
        out = swing_points(pts, thresh);
    }
    out
}

/// Cumulative profit: the total as an area + line, the per-core curves inside it, swing
/// labels carrying the running total, and a per-date hover popup.
///
/// The cores share the total's Y scale, so the scale is taken over BOTH — a core that
/// outruns the total would otherwise be painted outside the canvas.
pub(super) fn cumulative_area(
    days: &[DayPoint],
    cores: &[CoreSeries],
    colors: &[Hsla],
    hover: Option<usize>,
    bucket: i64,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    if days.is_empty() {
        return div().h(px(CHART_H)).into_any_element();
    }
    let n = days.len();
    // A non-finite profit would poison the whole curve: `f32::max/min` folds skip NaN, so
    // the range would stay finite while the point itself fed px(NaN) into the layout.
    let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
    // Running total in f64 — it is money, and it is what the labels and popup print; the
    // f32 copy below exists only for the pixel math.
    let mut acc = 0.0f64;
    let cum: Vec<f64> = days
        .iter()
        .map(|d| {
            acc += fin(d.profit);
            acc
        })
        .collect();
    let pts: Vec<f32> = cum.iter().map(|&v| v as f32).collect();
    // Which cores get a line: the biggest by ABSOLUTE contribution. `core_days` arrives
    // sorted by SIGNED total, so a plain `take` would keep the twelve best earners and hide
    // exactly the cores that lost the most — the ones worth looking at.
    let mut order: Vec<usize> = (0..cores.len()).collect();
    order.sort_by(|&a, &b| cores[b].total.abs().total_cmp(&cores[a].total.abs()));
    order.truncate(MAX_CORE_LINES);
    let curves: Vec<(Hsla, Vec<f32>)> = order
        .iter()
        .map(|&ci| {
            let mut c = 0.0f32;
            let v: Vec<f32> = cores[ci]
                .per_bucket
                .iter()
                .take(n)
                .map(|v| {
                    c += fin(*v) as f32;
                    c
                })
                .collect();
            (core_color(colors, ci, p), v)
        })
        .collect();
    // ONE Y range over the total AND every core line drawn inside it. Cores sum to the
    // total, so as soon as one of them loses, another peaks ABOVE the total and the total
    // curve is compressed — that is the honest picture, and the alternative (scaling the
    // cores to fit) would draw lines whose height means nothing.
    let mut vmax = pts.iter().copied().fold(0.0f32, f32::max);
    let mut vmin = pts.iter().copied().fold(0.0f32, f32::min);
    for (_, c) in &curves {
        for &v in c {
            vmax = vmax.max(v);
            vmin = vmin.min(v);
        }
    }
    let vmax = vmax.max(1e-6);
    let vmin = vmin.min(0.0);
    let span = (vmax - vmin).max(1e-6);
    // Tone from the f64 running total through the same rounding rule the header number
    // uses: off the f32 curve, accumulated drift at five-digit cumulatives is enough to
    // paint a green area under an orange number, and a raw `>= 0.0` would colour an area
    // under a "0". (The header sums without the `fin` guard, so a non-finite profit shows
    // "—" there while the curve here still draws — data that cannot occur from SQLite.)
    let tone = super::sign_color(p, cum.last().copied().unwrap_or(0.0));
    let line_col = moon(tone);
    let fill_col = moon_alpha(tone, 0.14);
    let sep_col = moon_alpha(p.text_muted, 0.22);
    let zero_col = moon_alpha(p.text_muted, 0.5);
    // Labels sit above their points, so the plot gives up a band for them. The band is
    // derived from the caption size — a raw px reserve would stop covering the text the
    // moment the UI font-size slider grows it. 1.7× covers gpui's own line height (≈1.618×).
    // Clamped: at an extreme font scale the band could otherwise eat the whole plot.
    let lift = f32::from(design::ui_px(cx, 3.0));
    let band = f32::from(design::t_caption(cx)) * 1.7 + lift;
    let plot_h = (CHART_H - band).max(20.0);
    // Labels: resolved HERE against the same vmin/span the canvas paints with, so the
    // points can move into the paint closure instead of being cloned for it.
    let labels: Vec<(f32, f32, String, u32)> = swing_labels(&pts)
        .into_iter()
        .map(|k| {
            let y_up = (pts[k] - vmin) / span * (plot_h - 2.0) + 1.0;
            let frac = if n > 1 {
                k as f32 / (n - 1) as f32
            } else {
                0.0
            };
            (
                frac,
                y_up,
                super::fmt_signed(cum[k]),
                super::sign_color(p, cum[k]),
            )
        })
        .collect();

    let pts_paint = pts;
    let canvas_el = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            if w < 4.0 || h < 4.0 || pts_paint.len() < 2 {
                return;
            }
            let n = pts_paint.len();
            let x = |k: usize| bounds.origin.x + px(k as f32 / (n - 1) as f32 * w);
            let y = |v: f32| bounds.origin.y + px((vmax - v) / span * (h - 2.0) + 1.0);
            let y0 = y(vmin); // area baseline = the bottom of the plot
            let mut fill = PathBuilder::fill();
            fill.move_to(gpui::point(x(0), y0));
            for (k, &v) in pts_paint.iter().enumerate() {
                fill.line_to(gpui::point(x(k), y(v)));
            }
            fill.line_to(gpui::point(x(n - 1), y0));
            if let Ok(path) = fill.build() {
                window.paint_path(path, fill_col);
            }
            // Column separators: baseline → sum line, one per bucket. Skipped once the
            // buckets are closer than a few pixels, where they would read as a solid wash
            // instead of dividing the chart.
            if w / (n - 1) as f32 >= MIN_SEP_STEP {
                // The last bucket sits exactly on the right edge, where a 1px quad would
                // fall outside the canvas — pull it back inside so both ends get a divider.
                let right = bounds.origin.x + px(w - 1.0);
                for (k, &v) in pts_paint.iter().enumerate() {
                    let (a, b) = (y(v).min(y0), y(v).max(y0));
                    window.paint_quad(gpui::fill(
                        Bounds::new(gpui::point(x(k).min(right), a), gpui::size(px(1.0), b - a)),
                        sep_col,
                    ));
                }
            }
            // Zero line, if the curve ever went negative.
            if vmin < 0.0 {
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        gpui::point(bounds.origin.x, y(0.0)),
                        gpui::size(px(w), px(1.0)),
                    ),
                    zero_col,
                ));
            }
            // Per-core curves: thin and translucent so they read as detail inside the
            // total, never competing with it.
            for (col, c) in &curves {
                if c.len() < 2 {
                    continue;
                }
                let mut pb = PathBuilder::stroke(px(1.0));
                for (k, &v) in c.iter().enumerate() {
                    let pt = gpui::point(x(k), y(v));
                    if k == 0 {
                        pb.move_to(pt);
                    } else {
                        pb.line_to(pt);
                    }
                }
                if let Ok(path) = pb.build() {
                    window.paint_path(path, gpui::hsla(col.h, col.s, col.l, 0.55));
                }
            }
            // The total line, on top of everything.
            let mut pb = PathBuilder::stroke(px(2.0));
            for (k, &v) in pts_paint.iter().enumerate() {
                let pt = gpui::point(x(k), y(v));
                if k == 0 {
                    pb.move_to(pt);
                } else {
                    pb.line_to(pt);
                }
            }
            if let Ok(path) = pb.build() {
                window.paint_path(path, line_col);
            }
        },
    )
    .w_full()
    .h(px(plot_h));

    // The plot is pinned to the BOTTOM of the stack, so a label's offset from the bottom is
    // the same number the canvas maps a value to. The hover columns cover the whole stack.
    let mut stack = div()
        .relative()
        .w_full()
        .h(px(CHART_H))
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(plot_h))
                .child(canvas_el),
        )
        .child(hover_row(n, hover, p, cx));
    let lw = design::font_w_px(cx, LABEL_W);
    for (frac, y_up, text, col) in labels {
        // Centred on its point, except at the very ends: there the label is pulled fully
        // inside, or half of it would hang past the card's edge (nothing clips it).
        let shift = if frac <= 0.0 {
            px(0.0)
        } else if frac >= 1.0 {
            -lw
        } else {
            -lw / 2.0
        };
        stack = stack.child(
            div()
                .absolute()
                .bottom(px(y_up + lift))
                .left(relative(frac))
                .ml(shift)
                .w(lw)
                .flex()
                .justify_center()
                .whitespace_nowrap()
                .text_size(design::t_caption(cx))
                .text_color(moon(col))
                .child(text),
        );
    }

    let popup = hover
        .filter(|bi| *bi < n && !cores.is_empty())
        // The curve puts bucket `bi` at bi/(n-1) — the same mapping the canvas paints with.
        .map(|bi| {
            let frac = if n > 1 {
                bi as f32 / (n - 1) as f32
            } else {
                0.0
            };
            bucket_popup(
                days,
                cores,
                colors,
                bi,
                frac,
                bucket,
                PopupMode::Running,
                p,
                cx,
            )
        });
    div()
        .relative()
        .w_full()
        .child(
            v_flex()
                .w_full()
                .gap(px(4.0))
                .child(stack)
                .child(date_ticks(days, bucket, p)),
        )
        .children(popup)
        .into_any_element()
}

/// Invisible hover-catcher columns over the plot — one per bucket, the hovered one washed.
///
/// Positioned ABSOLUTELY around each point rather than as `flex_1` cells: the curve puts
/// bucket `k` at `k/(n-1)` of the width, while equal flex cells centre it at `(k+0.5)/n`,
/// so the wash would sit up to half a column away from the point it claims to highlight.
fn hover_row(
    n: usize,
    hover: Option<usize>,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> impl IntoElement {
    let mut row = div().absolute().inset_0();
    // Half a step on each side of the point; the end columns are clipped by the edges.
    let step = 1.0 / (n.saturating_sub(1)).max(1) as f32;
    for bi in 0..n {
        let centre = bi as f32 * step;
        let left = (centre - step / 2.0).max(0.0);
        let right = (centre + step / 2.0).min(1.0);
        let mut col = div()
            .id(SharedString::from(format!("an-cum-{bi}")))
            .absolute()
            .top_0()
            .bottom_0()
            .left(relative(left))
            .w(relative(right - left))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    if this.hover_cum_bucket != Some(bi) {
                        this.hover_cum_bucket = Some(bi);
                        cx.notify();
                    }
                } else if this.hover_cum_bucket == Some(bi) {
                    this.hover_cum_bucket = None;
                    cx.notify();
                }
            }));
        if hover == Some(bi) {
            col = col.bg(moon_alpha(p.text_muted, 0.07));
        }
        row = row.child(col);
    }
    row
}

/// Dated X ticks: up to `X_TICKS` evenly spaced labels, first and last always present.
/// Laid out with `justify_between`, so the end labels sit flush with the chart's edges
/// rather than centred on the outermost points — the usual axis convention, and the only
/// one that keeps them inside the card. Ticks in between are therefore approximate: they
/// name evenly spaced dates, they are not plumb lines onto their points.
fn date_ticks(days: &[DayPoint], bucket: i64, p: MoonPalette) -> AnyElement {
    let n = days.len();
    if n == 0 {
        // Callers return early on an empty period, but this runs in the render path and a
        // panic here takes the whole app down — do not rely on the caller.
        return div().into_any_element();
    }
    let count = X_TICKS.min(n).max(1);
    let mut row = h_flex().w_full().justify_between();
    for i in 0..count {
        // Round rather than truncate, or the ticks name buckets that drift steadily left
        // of the position they are drawn at.
        let k = if count == 1 {
            0
        } else {
            (i * (n - 1) + (count - 1) / 2) / (count - 1)
        };
        row = row.child(muted_caption(p, bucket_label(days[k].start, bucket)));
    }
    row.into_any_element()
}

#[cfg(test)]
mod tests;
