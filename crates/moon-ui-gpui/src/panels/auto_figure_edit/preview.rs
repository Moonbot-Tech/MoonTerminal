//! The editor's preview: demo candles, the row's own detector run over them, and the result drawn
//! the way the chart draws it.
//!
//! The CALCULATION is the chart's: `moon_chart::auto_figures::detect`, then `take::select`, then
//! `draw::append_shapes` — the same three calls the chart's job and its geometry rebuild make, so
//! the preview cannot find a level the chart would not, or draw a line longer than the chart
//! would. The PAINTING is a small `canvas` instead of a chart engine: the engine is a GPU surface
//! with its own pass, panes, camera and input, built to sit in a dock, and none of the reusable
//! pieces in the tree draw figures without one (the captions editor's sample is text). Here only
//! the handful of figure tools `append_shapes` emits (a level, a horizontal ray, a segment, a ray)
//! and the tolerance bands have to be drawn, each as a stroked path with the row's own dash pattern.
//!
//! The candles are SYNTHETIC and deterministic — a range that breaks up into a squeeze under a
//! flat top — rather than a slice of the committed fixture: the fixture is a SQLite file the UI
//! would have to open on the UI thread, and a made-up series can be shaped so that every detector
//! has something to find. A formation row gets a series of its own ([`demo_formation_bars`]): a
//! two-line pattern is only reported while price is still inside it, and no one series holds a
//! current triangle, channel, flag and double at once.

use std::f64::consts::TAU;

use gpui::*;
use moon_chart::auto_figures::draw::{ResolvedStyle, append_shapes};
use moon_chart::auto_figures::{DetectCtx, take};
use moon_chart::figures::FigureBand;
use moon_core::config::ChartTheme;
use moon_core::config::LabelColor;
use moon_core::config::auto_figures::{
    AutoFigureRow, DetectorParams, FormationParams, LineStyle, TfId, TfSource,
};
use moon_core::figures::{Figure, FigureKind, LineKind};
use moon_core::market::ChartCandle;

use crate::chartdx::auto_figures_sync::resolve_rgb;
use crate::design;

/// Most bars the preview builds and draws: about two pixels a bar across the dialog.
pub(super) const MAX_BARS: usize = 240;
/// Fewest: below this a pivot detector has nothing to work with.
pub(super) const MIN_BARS: usize = 80;
/// Bars of one swing of the demo series.
const SWING_BARS: f64 = 20.0;
/// Shortest swing of the demo series, in bars: a pivot two bars wide still finds its turns.
const MIN_SWING_BARS: f64 = 8.0;

/// Bars of one swing of the demo series for a row that reads the newest `lookback` bars:
/// [`SWING_BARS`], or fewer when the lookback is short, so the window it reads still holds five
/// swings. A sloped line of three touches needs three troughs inside the window, and a 40-bar
/// window (the shipped hour rows) over 20-bar swings holds two — the preview would show nothing.
pub(super) fn demo_swing(lookback: u32) -> f64 {
    (f64::from(lookback) / 5.0).clamp(MIN_SWING_BARS, SWING_BARS)
}

/// Everything one preview draws, computed once per edit rather than per frame.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct PreviewData {
    pub bars: Vec<ChartCandle>,
    /// The bar length of `bars`.
    pub tf: TfId,
    pub figures: Vec<Figure>,
    pub bands: Vec<FigureBand>,
    /// Figures the detector found, and how many the row keeps.
    pub found: usize,
    pub kept: usize,
}

/// The timeframe the demo candles are built on.
///
/// The row's own where it is pinned, `15m` for `Auto` (the chart's span is not known here), both
/// capped by what the detector can answer on. A period extreme then steps up until its whole
/// period fits in [`MAX_BARS`]: "this week's high" on five-minute demo bars would need four
/// thousand of them, and a preview of a period it cannot see draws nothing.
pub(super) fn preview_tf(row: &AutoFigureRow, params: &DetectorParams) -> TfId {
    let cap = params.coarsest_tf();
    let wanted = match row.tf {
        TfSource::Fixed(tf) => tf.min(cap),
        TfSource::Auto => TfId::M15.min(cap),
    };
    match params {
        // The calendar kinds read whole days or weeks, so they step up the same way.
        DetectorParams::PeriodExtreme(_)
        | DetectorParams::VolumeProfile(_)
        | DetectorParams::PeriodLevel(_) => TfId::ALL
            .into_iter()
            .filter(|tf| *tf >= wanted && *tf <= cap)
            .find(|tf| params.bars_needed(*tf) as usize <= MAX_BARS)
            .unwrap_or(wanted),
        _ => wanted,
    }
}

/// A deterministic demo series of `n` closed bars of `tf`, the last one closing before `now_ms`.
///
/// Two phases, so that every detector finds something: a range between two flat levels for the
/// first 40 %, then a squeeze — lows rising along a line into a flat top above the old range.
/// The wicks carry a small fixed jitter so pivots do not sit at one exact price, and every bar
/// carries turnover, so the volume detectors have a profile to build. The old range lies below
/// everything after it, so its days' POCs stay untouched — naked. Each swing is `swing` bars long
/// ([`demo_swing`]).
pub(super) fn demo_bars(tf: TfId, n: usize, now_ms: f64, swing: f64) -> Vec<ChartCandle> {
    let tf_ms = tf.ms() as f64;
    let last_open = (now_ms / tf_ms).floor() * tf_ms - tf_ms;
    let split = (n as f64 * 0.4).round();
    let path = |i: f64| -> f64 {
        let wave = (1.0 - (TAU * i / swing).cos()) / 2.0;
        if i < split {
            95.0 + 4.0 * wave
        } else {
            let k = (i - split) / (n as f64 - split).max(1.0);
            let floor = 99.0 + 3.5 * k;
            let top = 104.0;
            top - (top - floor) * (1.0 - wave)
        }
    };
    // A small linear-congruential jitter: fixed seed, so the same settings always preview alike.
    let mut seed: u32 = 0x9E37_79B9;
    let mut jitter = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(seed >> 8) / f64::from(1u32 << 24)
    };
    (0..n)
        .map(|i| {
            let i_f = i as f64;
            let open = path(i_f - 0.5);
            let close = path(i_f + 0.5);
            let hi = open.max(close) * (1.0 + 0.0004 * jitter());
            let lo = open.min(close) * (1.0 - 0.0004 * jitter());
            // Turnover grows with the bar's range, with the same fixed jitter: the volume profile
            // gets a day with a clear POC and value area, and "strongest by volume" has something
            // to rank. Base volume is the turnover over the mid price, as a cache row carries it.
            let quote_volume = 1_000.0 * (1.0 + (hi - lo)) * (0.5 + jitter());
            ChartCandle {
                t_open_ms: last_open - (n - 1 - i) as f64 * tf_ms,
                open: open as f32,
                high: hi as f32,
                low: lo as f32,
                close: close as f32,
                volume: (quote_volume / ((hi + lo) / 2.0)) as f32,
                quote_volume: quote_volume as f32,
            }
        })
        .collect()
}

/// One formation demo: its turning points as `(bars before the last bar, price)`, oldest first.
type Scenario = &'static [(f64, f64)];

/// A symmetric triangle still forming at the last bar: tops 112 → 110.5 (−0.05 a bar), bottoms
/// 104 → 105.5 (+0.05 a bar), apex past the right edge, price between the sides at the end.
const TRIANGLE: Scenario = &[
    (70.0, 106.0),
    (60.0, 112.0),
    (55.0, 104.0),
    (50.0, 111.5),
    (45.0, 104.5),
    (40.0, 111.0),
    (35.0, 105.0),
    (30.0, 110.5),
    (25.0, 105.5),
    (15.0, 107.8),
    (10.0, 107.2),
    (5.0, 107.9),
    (0.0, 107.5),
];

/// A rising channel of three touches a side (+1/16 a bar, 2.5 wide), price inside it at the end;
/// 40 bars long, past any flag's cap.
const CHANNEL: Scenario = &[
    (60.0, 101.0),
    (50.0, 100.0),
    (42.0, 103.0),
    (34.0, 101.0),
    (26.0, 104.0),
    (18.0, 102.0),
    (10.0, 105.0),
    (2.0, 103.5),
    (0.0, 103.8),
];

/// A bull flag: a pole 100 → 108 over eight bars, then a falling channel (−1/6 a bar, 2 wide —
/// a quarter of the pole) of 15 bars, price inside it at the end.
const FLAG: Scenario = &[
    (40.0, 101.0),
    (30.0, 100.0),
    (22.0, 108.0),
    (19.0, 105.5),
    (16.0, 107.0),
    (13.0, 104.5),
    (10.0, 106.0),
    (7.0, 103.5),
    (3.0, 104.0),
    (0.0, 103.6),
];

/// A double top: 110 and 110.2 twenty bars apart over a trough of 106 (3.6 % deep), price back
/// between the neckline and the tops at the end.
const DOUBLE_TOP: Scenario = &[
    (52.0, 105.0),
    (40.0, 110.0),
    (30.0, 106.0),
    (20.0, 110.2),
    (10.0, 107.5),
    (5.0, 108.3),
    (0.0, 107.8),
];

/// The demo a formation row is previewed on: the first kind it looks for, in the editor's order —
/// a triangle (or wedge), a channel, a flag, a double. A row looking for nothing gets the triangle;
/// it finds nothing on it, which is what the preview should then say.
fn formation_scenario(p: &FormationParams) -> Scenario {
    if p.triangle || p.wedge {
        TRIANGLE
    } else if p.channel {
        CHANNEL
    } else if p.flag {
        FLAG
    } else if p.double {
        DOUBLE_TOP
    } else {
        TRIANGLE
    }
}

/// A deterministic demo series of `n` closed bars of `tf` holding the formation `p` looks for
/// first ([`formation_scenario`]), ending at the last bar closed before `now_ms`.
///
/// The pattern is laid out from the RIGHT edge, so the shorter lookback of a flag row (100 bars)
/// and the longer one of a triangle row (150) both hold all of it; before its first turning point
/// the series waves gently (±0.4 %) around that point's price. Each bar opens and closes on the
/// path half a bar either side of its own point and reaches the path at that point, so a turning
/// point is a pivot of exactly its own bar; the wicks carry the same small fixed jitter as
/// [`demo_bars`] (well inside the 0.2 % touch tolerance), and every bar carries turnover.
pub(super) fn demo_formation_bars(
    p: &FormationParams,
    tf: TfId,
    n: usize,
    now_ms: f64,
) -> Vec<ChartCandle> {
    let points = formation_scenario(p);
    let tf_ms = tf.ms() as f64;
    let last_open = (now_ms / tf_ms).floor() * tf_ms - tf_ms;
    // Price at `j` bars before the last bar, linear between the turning points.
    let path = |j: f64| -> f64 {
        let (j0, p0) = points[0];
        if j >= j0 {
            return p0 * (1.0 + 0.004 * (TAU * (j - j0) / 12.0).sin());
        }
        for w in points.windows(2) {
            let ((ja, pa), (jb, pb)) = (w[0], w[1]);
            if j <= ja && j >= jb {
                return pa + (pb - pa) * (ja - j) / (ja - jb);
            }
        }
        points[points.len() - 1].1
    };
    let mut seed: u32 = 0x2545_F491;
    let mut jitter = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(seed >> 8) / f64::from(1u32 << 24)
    };
    (0..n)
        .map(|i| {
            let j = (n - 1 - i) as f64;
            let (open, mid, close) = (path(j + 0.5), path(j), path(j - 0.5));
            let hi = open.max(close).max(mid) * (1.0 + 0.0003 * jitter());
            let lo = open.min(close).min(mid) * (1.0 - 0.0003 * jitter());
            let quote_volume = 1_000.0 * (1.0 + (hi - lo)) * (0.5 + jitter());
            ChartCandle {
                t_open_ms: last_open - (n - 1 - i) as f64 * tf_ms,
                open: open as f32,
                high: hi as f32,
                low: lo as f32,
                close: close as f32,
                volume: (quote_volume / ((hi + lo) / 2.0)) as f32,
                quote_volume: quote_volume as f32,
            }
        })
        .collect()
}

/// Run the row over the demo candles and build what it draws. `row` must already be sanitized:
/// the preview shows what the chart would draw, and the chart only ever sees sanitized rows.
///
/// `None` for a row this build cannot interpret — there is nothing to run.
pub(super) fn compute(row: &AutoFigureRow, now_ms: f64, theme: &ChartTheme) -> Option<PreviewData> {
    let params = row.detector.known()?;
    let tf = preview_tf(row, params);
    let needed = params.bars_needed(tf);
    let n = (needed as usize).clamp(MIN_BARS, MAX_BARS);
    let bars = match params {
        DetectorParams::Formation(p) => demo_formation_bars(p, tf, n, now_ms),
        _ => demo_bars(tf, n, now_ms, demo_swing(needed)),
    };
    let ctx = DetectCtx {
        tf_ms: tf.ms(),
        now_ms,
    };
    let found = moon_chart::auto_figures::detect(params, &bars, &ctx);
    let ref_price = bars.last().map_or(f64::NAN, |b| f64::from(b.close));
    let kept = take::select(&found, &row.take, ref_price, now_ms);
    let mut figures = Vec::new();
    let mut bands = Vec::new();
    for &ix in &kept {
        let Some(f) = found.get(ix) else {
            continue;
        };
        // A formation's side of price is its nearer side's, as `take` judged it.
        let above = f
            .shape
            .price_near(now_ms, ref_price)
            .is_some_and(|p| p >= ref_price);
        let rgb = resolve_rgb(row.style.color, above, theme);
        // The zone band comes from the tolerance the detector reports with each figure.
        append_shapes(
            f,
            &ResolvedStyle::new(&row.style, rgb),
            now_ms,
            &mut figures,
            &mut bands,
        );
    }
    Some(PreviewData {
        bars,
        tf,
        figures,
        bands,
        found: found.len(),
        kept: kept.len(),
    })
}

/// Dash pattern of a line kind as alternating on/off lengths in pixels, scaled by the stroke.
/// Empty for a solid line.
pub(super) fn dash_pattern(kind: LineKind, thickness: f32) -> Vec<f32> {
    let u = thickness.max(1.0);
    match kind {
        LineKind::Solid => Vec::new(),
        LineKind::Dash => vec![6.0 * u, 4.0 * u],
        LineKind::Dot => vec![1.5 * u, 2.5 * u],
        LineKind::DashDot => vec![6.0 * u, 3.0 * u, 1.5 * u, 3.0 * u],
        LineKind::DashDotDot => vec![6.0 * u, 3.0 * u, 1.5 * u, 3.0 * u, 1.5 * u, 3.0 * u],
    }
}

/// Cut the segment `a → b` into the visible dashes of `pattern` (whole segment when empty).
pub(super) fn dashes(
    a: (f32, f32),
    b: (f32, f32),
    pattern: &[f32],
) -> Vec<((f32, f32), (f32, f32))> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if !len.is_finite() || len <= 0.0 {
        return Vec::new();
    }
    let period: f32 = pattern.iter().sum();
    if pattern.is_empty() || period <= 0.0 {
        return vec![(a, b)];
    }
    let at = |s: f32| (a.0 + dx * s / len, a.1 + dy * s / len);
    let mut out = Vec::new();
    let mut s = 0.0;
    // Bounded by the length over the shortest step, so a degenerate pattern cannot spin.
    let min_step = pattern
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min)
        .max(0.5);
    let max_steps = (len / min_step) as usize + pattern.len() + 1;
    for k in 0..max_steps {
        if s >= len {
            break;
        }
        let step = pattern[k % pattern.len()];
        let end = (s + step).min(len);
        if k % 2 == 0 {
            out.push((at(s), at(end)));
        }
        s = end;
    }
    out
}

/// The two ends of a figure on the time axis `[t_lo, t_hi]`, as `(t, price)` pairs, or `None` for
/// a shape the preview does not draw.
fn figure_ends(kind: &FigureKind, t_lo: f64, t_hi: f64) -> Option<((f64, f64), (f64, f64))> {
    let along = |a: (f64, f64), b: (f64, f64), t: f64| {
        let span = b.0 - a.0;
        if span.abs() < f64::EPSILON {
            a.1
        } else {
            a.1 + (b.1 - a.1) * (t - a.0) / span
        }
    };
    match kind {
        FigureKind::HLine(h) => Some(((t_lo, h.price), (t_hi, h.price))),
        FigureKind::HorizontalRay(r) => {
            let t0 = r.origin.time_ms.max(t_lo);
            Some(((t0, r.origin.price), (t_hi, r.origin.price)))
        }
        FigureKind::Segment(s) => Some(((s.a.time_ms, s.a.price), (s.b.time_ms, s.b.price))),
        FigureKind::Ray(r) => {
            let a = (r.a.time_ms, r.a.price);
            let b = (r.b.time_ms, r.b.price);
            let end_t = if b.0 >= a.0 { t_hi } else { t_lo };
            Some((a, (end_t, along(a, b, end_t))))
        }
        _ => None,
    }
}

/// Paint the preview into `b`: background, grid, candles, bands, then the figures.
pub(super) fn paint(
    data: &PreviewData,
    theme: &ChartTheme,
    b: Bounds<Pixels>,
    window: &mut Window,
) {
    let w = f32::from(b.size.width);
    let h = f32::from(b.size.height);
    let n = data.bars.len();
    if w < 8.0 || h < 8.0 || n == 0 {
        return;
    }
    let tf_ms = data.tf.ms() as f64;
    let t_lo = data.bars[0].t_open_ms;
    // A little room past the last bar, the way the chart leaves its live edge some air.
    let t_hi = data.bars[n - 1].t_open_ms + tf_ms * (1.0 + n as f64 * 0.06);
    let (mut lo, mut hi) = data.bars.iter().fold((f64::MAX, f64::MIN), |(lo, hi), c| {
        (lo.min(f64::from(c.low)), hi.max(f64::from(c.high)))
    });
    for band in &data.bands {
        lo = lo.min(band.p0.min(band.p1));
        hi = hi.max(band.p0.max(band.p1));
    }
    let pad = (hi - lo).max(1e-9) * 0.08;
    let (lo, hi) = (lo - pad, hi + pad);
    let x = |t: f64| b.origin.x + px(((t - t_lo) / (t_hi - t_lo)) as f32 * w);
    let y = |p: f64| b.origin.y + px(((hi - p) / (hi - lo)) as f32 * h);
    let color = |rgb: [u8; 3], alpha: f32| design::moon_alpha(design::rgb_to_u32(rgb), alpha);

    window.paint_quad(fill(b, color(theme.bg, 1.0)));
    window.with_content_mask(Some(ContentMask { bounds: b }), |window| {
        for i in 1..4 {
            let yy = b.origin.y + px(h * i as f32 / 4.0);
            window.paint_quad(fill(
                Bounds::new(point(b.origin.x, yy), size(px(w), px(1.0))),
                color(theme.grid, theme.grid_alpha.clamp(0.05, 1.0)),
            ));
        }
        let bar_w = (w / ((t_hi - t_lo) / tf_ms) as f32 * 0.7).max(1.0);
        for c in &data.bars {
            let up = c.close >= c.open;
            let rgb = if up {
                theme.candle_up
            } else {
                theme.candle_down
            };
            let cx = x(c.t_open_ms + tf_ms / 2.0);
            let (o, cl) = (f64::from(c.open), f64::from(c.close));
            window.paint_quad(fill(
                Bounds::from_corners(
                    point(cx - px(0.5), y(f64::from(c.high))),
                    point(cx + px(0.5), y(f64::from(c.low))),
                ),
                color(rgb, 1.0),
            ));
            let top = y(o.max(cl));
            let bottom = y(o.min(cl)).max(top + px(1.0));
            window.paint_quad(fill(
                Bounds::from_corners(
                    point(cx - px(bar_w / 2.0), top),
                    point(cx + px(bar_w / 2.0), bottom),
                ),
                color(rgb, 1.0),
            ));
        }
        for band in &data.bands {
            let t0 = band.t0_ms.max(t_lo);
            let t1 = band.t1_ms.min(t_hi);
            if !(t1 > t0) || band.color[3] <= 0.0 {
                continue;
            }
            let rgba = Rgba {
                r: band.color[0],
                g: band.color[1],
                b: band.color[2],
                a: band.color[3],
            };
            window.paint_quad(fill(
                Bounds::from_corners(
                    point(x(t0), y(band.p0.max(band.p1))),
                    point(x(t1), y(band.p0.min(band.p1))),
                ),
                rgba,
            ));
        }
        for f in &data.figures {
            let Some(((ta, pa), (tb, pb))) = figure_ends(&f.kind, t_lo, t_hi) else {
                continue;
            };
            if ![ta, pa, tb, pb].iter().all(|v| v.is_finite()) {
                continue;
            }
            let a = (f32::from(x(ta)), f32::from(y(pa)));
            let bb = (f32::from(x(tb)), f32::from(y(pb)));
            let stroke = Rgba {
                r: f32::from(f.color[0]) / 255.0,
                g: f32::from(f.color[1]) / 255.0,
                b: f32::from(f.color[2]) / 255.0,
                a: 1.0,
            };
            for (s, e) in dashes(a, bb, &dash_pattern(f.line_kind, f.thickness)) {
                let mut path = PathBuilder::stroke(px(f.thickness.max(0.5)));
                path.move_to(point(px(s.0), px(s.1)));
                path.line_to(point(px(e.0), px(e.1)));
                if let Ok(path) = path.build() {
                    window.paint_path(path, stroke);
                }
            }
        }
    });
}

/// A small sample of a row's line: its colour and its dash pattern, at its own thickness.
///
/// `BySign` has two colours, so the sample is split — support's colour on the left, resistance's
/// on the right — the way the chart colours the two sides of price.
pub(crate) fn line_swatch(
    style: LineStyle,
    theme: &ChartTheme,
    w: f32,
    h: f32,
) -> impl IntoElement {
    let theme = theme.clone();
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let mid = f32::from(b.origin.y) + f32::from(b.size.height) / 2.0;
            let x0 = f32::from(b.origin.x) + 1.0;
            let x1 = f32::from(b.origin.x) + f32::from(b.size.width) - 1.0;
            let halves: Vec<((f32, f32), (f32, f32), [u8; 3])> = match style.color {
                LabelColor::BySign => {
                    let xm = (x0 + x1) / 2.0;
                    vec![
                        (
                            (x0, mid),
                            (xm, mid),
                            resolve_rgb(style.color, false, &theme),
                        ),
                        ((xm, mid), (x1, mid), resolve_rgb(style.color, true, &theme)),
                    ]
                }
                c => vec![((x0, mid), (x1, mid), resolve_rgb(c, false, &theme))],
            };
            let thickness = style.thickness.clamp(1.0, 4.0);
            for (a, e, rgb) in halves {
                for (s, t) in dashes(a, e, &dash_pattern(style.line_kind, thickness.min(1.5))) {
                    let mut path = PathBuilder::stroke(px(thickness));
                    path.move_to(point(px(s.0), px(s.1)));
                    path.line_to(point(px(t.0), px(t.1)));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, design::moon(design::rgb_to_u32(rgb)));
                    }
                }
            }
        },
    )
    .w(px(w))
    .h(px(h))
    .flex_none()
}
