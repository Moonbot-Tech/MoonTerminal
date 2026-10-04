//! The picture of one closed trade, sent to Telegram: every print of its tape as a cross in the
//! colour of the side that took it, the volume along the bottom, the entry and exit orders as the
//! steps they rested at, the stop, both fills with their prices, and the result above.
//!
//! Drawn without a graphics library — the station runs on one CPU and a gigabyte: an RGB canvas
//! ([`canvas`]), a coverage rasterizer ([`raster`]), our own TrueType reader for the terminal's
//! face ([`font`]) and a PNG writer ([`png`]). The colours are the terminal's dark theme.
//!
//! The frame is the trade's own window, not the tape's: a quiet market prints a handful of times
//! a minute, and a frame stretched to its prints would leave the trade a sliver at one edge.

mod canvas;
mod font;
mod format;
mod png;
mod raster;

use chrono_tz::Tz;
use moon_core::feed::{Side, Tick};

use canvas::{Canvas, Rgb, cross_mask, rgb};
use font::Text;

const WIDTH: usize = 2000;
const HEIGHT: usize = 1200;

/// The plot's frame inside the canvas: the header above, the price axis right, the clock below.
const PLOT_LEFT: f32 = 24.0;
const PLOT_RIGHT: f32 = WIDTH as f32 - 150.0;
const PLOT_TOP: f32 = 76.0;
const PLOT_BOTTOM: f32 = HEIGHT as f32 - 48.0;
/// The share of the plot's height the volume may take, from the bottom.
const VOLUME_SHARE: f32 = 0.22;
/// Width of one volume column.
const VOLUME_BAR: f32 = 6.0;

const SIZE_TITLE: f32 = 32.0;
const SIZE_TEXT: f32 = 26.0;
const SIZE_SMALL: f32 = 22.0;

/// About this many price intervals, and this many clock labels.
const PRICE_LINES: f64 = 8.0;
const CLOCK_LABELS: i64 = 6;
/// The clock steps an axis may use, in seconds.
const CLOCK_STEPS: [i64; 16] = [
    1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1_800, 3_600, 7_200, 21_600, 86_400,
];

/// Past this many prints the crosses are cut to what a pixel column shows: its first, last,
/// lowest and highest print. The volume still sums every print.
const MAX_CROSSES: usize = 20_000;
const CROSS_ARM: f32 = 5.0;
const CROSS_WIDTH: f32 = 2.2;
const DOT: f32 = 9.0;
const ORDER_WIDTH: f32 = 3.0;
/// The height of a price chip on the axis.
const CHIP_HEIGHT: f32 = 36.0;

// The terminal's dark theme (MoonUI `MoonPalette` dark: surface, border, text, text_muted,
// green, red, blue, accent).
const PANEL: Rgb = rgb(0x16_18_1B);
const RULE: Rgb = rgb(0x2A_2D_31);
const INK: Rgb = rgb(0xE8_E4_DC);
const MUTED: Rgb = rgb(0x7D_76_69);
const BUY: Rgb = rgb(0x1E_8C_5B);
const SELL: Rgb = rgb(0xE5_48_4D);
const EXIT_LONG: Rgb = rgb(0x7F_C9_FF);
const EXIT_SHORT: Rgb = rgb(0xFF_B3_47);
/// Text on a chip of a light colour.
const CHIP_INK: Rgb = rgb(0x16_18_1B);

/// The result line above the plot: the dollar result, its percent and the strategy, written as
/// the rest of the picture writes money (a result that rounds to zero is flat).
pub(crate) fn caption(profit_usd: f64, pct: Option<f64>, strategy: &str) -> String {
    // A gain is written bare, as on the reference picture; a loss keeps its minus.
    let money = format::signed_money(profit_usd);
    let mut caption = format!("{} USDT", money.trim_start_matches('+'));
    if let Some(pct) = pct.filter(|p| p.is_finite()) {
        caption.push_str(&format!(" ({}%)", format::signed_percent(pct)));
    }
    if !strategy.is_empty() {
        caption.push_str("  ");
        caption.push_str(strategy);
    }
    caption
}

/// Whether a result is drawn in the colour of a gain: it does not print with a minus.
pub(crate) fn won(profit_usd: f64) -> bool {
    !format::signed_money(profit_usd).starts_with('-')
}

/// What the picture is about. Prices are the market's own; moments are Unix milliseconds.
pub(crate) struct DealChart<'a> {
    pub market: &'a str,
    /// The coin the quantities are in, which names the volume.
    pub base: &'a str,
    pub short: bool,
    /// `(ms, price)` of each fill; a price of 0 is not drawn.
    pub entry: (i64, f64),
    pub exit: (i64, f64),
    pub stop: Option<f64>,
    /// Where each order rested, oldest first. Empty when the core did not archive it: then no
    /// line is drawn — a flat line would be a rest nobody recorded.
    pub entry_line: &'a [(i64, f64)],
    pub exit_line: &'a [(i64, f64)],
    /// The result line: profit, percent, strategy.
    pub caption: &'a str,
    pub won: bool,
    pub spent_usd: Option<f64>,
    pub day_usd: Option<f64>,
    /// The prints, ascending; the ones outside [`Self::window`] are left out.
    pub ticks: &'a [Tick],
    /// The stretch of time the picture shows.
    pub window: (i64, i64),
    pub zone: Tz,
}

/// The PNG of `chart`, `None` when it has no price to draw or an empty window.
pub(crate) fn render(chart: &DealChart<'_>) -> Option<Vec<u8>> {
    let (t0, t1) = chart.window;
    if t1 <= t0 {
        return None;
    }
    let ticks: Vec<&Tick> = chart
        .ticks
        .iter()
        .filter(|t| t.price.is_finite() && t.price > 0.0 && t.qty.is_finite())
        .filter(|t| (t0..=t1).contains(&(t.time_ms as i64)))
        .collect();
    let entry_line = clip_line(chart.entry_line, t0, chart.entry.0);
    let exit_from = if chart.entry.1 > 0.0 {
        chart.entry.0.max(t0)
    } else {
        t0
    };
    let exit_line = clip_line(chart.exit_line, exit_from, chart.exit.0);
    let prices = ticks
        .iter()
        .map(|t| f64::from(t.price))
        .chain([chart.entry.1, chart.exit.1])
        .chain(chart.stop)
        .chain(entry_line.iter().chain(&exit_line).map(|p| p.1))
        .filter(|p| p.is_finite() && *p > 0.0);
    let scale = PriceScale::fit(prices)?;
    let plot = Plot {
        t0,
        t1,
        low: scale.low,
        high: scale.high,
    };
    let mut canvas = Canvas::new(WIDTH, HEIGHT, PANEL);
    let mut text = Text::default();

    if chart.entry.1 > 0.0 && chart.exit.1 > 0.0 {
        let (a, b) = (plot.x(chart.entry.0), plot.x(chart.exit.0));
        let band = if chart.won { BUY } else { SELL };
        canvas.rect((a, PLOT_TOP), (b.max(a + 2.0), PLOT_BOTTOM), band, 0.10);
    }
    price_grid(&mut canvas, &mut text, &plot, &scale);
    clock_axis(&mut canvas, &mut text, &plot, chart.zone);
    let peak_volume = volume(&mut canvas, &plot, &ticks);
    crosses(&mut canvas, &plot, &ticks);
    if let Some(stop) = chart.stop.filter(|p| *p > 0.0) {
        stop_level(&mut canvas, &mut text, &plot, &scale, stop);
    }
    let exit_ink = if chart.short { EXIT_SHORT } else { EXIT_LONG };
    order_steps(&mut canvas, &plot, &entry_line, chart.entry, INK);
    order_steps(&mut canvas, &plot, &exit_line, chart.exit, exit_ink);
    fills(&mut canvas, &mut text, &plot, &scale, chart, exit_ink);
    header(&mut canvas, &mut text, chart, &scale);
    summary(&mut canvas, &mut text, chart, &ticks, peak_volume);
    Some(png::encode(canvas.w, canvas.h, &canvas.px))
}

/// The part of an order's line inside `[from, until]`: where it rested at `from`, then each move.
fn clip_line(points: &[(i64, f64)], from: i64, until: i64) -> Vec<(i64, f64)> {
    let mut out: Vec<(i64, f64)> = Vec::new();
    for &(at, price) in points {
        if at <= 0 || !(price.is_finite() && price > 0.0) || at > until {
            continue;
        }
        if at <= from {
            out.clear();
            out.push((from, price));
        } else {
            out.push((at, price));
        }
    }
    out.sort_by_key(|p| p.0);
    out
}

/// The price axis: a round step and the range it spans.
struct PriceScale {
    low: f64,
    high: f64,
    step: f64,
    decimals: usize,
    /// The lowest and the highest price drawn, before the margin and the step.
    drawn: (f64, f64),
}

impl PriceScale {
    /// The range that holds every price with a margin, cut on a round step (1, 2, 2.5 or 5 of a
    /// power of ten) giving about [`PRICE_LINES`] intervals.
    fn fit(prices: impl Iterator<Item = f64>) -> Option<Self> {
        let (lo, hi) = prices.fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p), hi.max(p)));
        if !(lo.is_finite() && hi.is_finite()) || hi < lo {
            return None;
        }
        let drawn = (lo, hi);
        // A flat window still gets a band a fifth of a percent tall; the margin never takes the
        // axis below zero, where no price is.
        let span = (hi - lo).max(hi * 0.002);
        let (lo, hi) = ((lo - span * 0.08).max(0.0), hi + span * 0.08);
        let raw = (hi - lo) / PRICE_LINES;
        let magnitude = 10f64.powf(raw.log10().floor());
        let step = [1.0, 2.0, 2.5, 5.0, 10.0]
            .into_iter()
            .map(|m| m * magnitude)
            .find(|s| *s >= raw)?;
        let low = (lo / step).floor() * step;
        let high = (hi / step).ceil() * step;
        (high > low && step.is_finite()).then(|| Self {
            low,
            high,
            step,
            decimals: format::step_decimals(step),
            drawn,
        })
    }

    /// How far the drawn prices reach, in percent of the lowest: what the market moved within the
    /// picture, not the height of the frame around it.
    fn range_percent(&self) -> f64 {
        let (lo, hi) = self.drawn;
        if lo > 0.0 {
            (hi / lo - 1.0) * 100.0
        } else {
            0.0
        }
    }
}

/// Moments and prices to pixels.
struct Plot {
    t0: i64,
    t1: i64,
    low: f64,
    high: f64,
}

impl Plot {
    fn x(&self, ms: i64) -> f32 {
        let share = (ms - self.t0) as f64 / (self.t1 - self.t0) as f64;
        PLOT_LEFT + (PLOT_RIGHT - PLOT_LEFT) * share.clamp(0.0, 1.0) as f32
    }

    fn y(&self, price: f64) -> f32 {
        let share = (price - self.low) / (self.high - self.low);
        PLOT_BOTTOM - (PLOT_BOTTOM - PLOT_TOP) * share.clamp(0.0, 1.0) as f32
    }
}

/// A line on every step of the price, its price on the axis at the right.
fn price_grid(canvas: &mut Canvas, text: &mut Text, plot: &Plot, scale: &PriceScale) {
    let lines = ((scale.high - scale.low) / scale.step).round() as i64;
    let cap = text.cap_height(SIZE_SMALL);
    for k in 0..=lines.min(40) {
        let price = scale.low + k as f64 * scale.step;
        let y = plot.y(price);
        canvas.hline(PLOT_LEFT, PLOT_RIGHT, y, 2.0, RULE, 1.0);
        let label = format::price(price, scale.decimals);
        text.draw(
            canvas,
            (PLOT_RIGHT + 12.0, y + cap / 2.0),
            &label,
            SIZE_SMALL,
            MUTED,
            1.0,
        );
    }
}

/// Round moments in the zone along the bottom, a line up the plot at each, the label centred
/// under its line — or left out where it would not fit, rather than slid off its line.
///
/// The moments are round on the zone's own clock: each next one is a step later in local time,
/// turned back into UTC with the offset in force then, so a change of offset inside the window
/// (summer time) keeps them on the hour. A window of a day or more names the date as well.
fn clock_axis(canvas: &mut Canvas, text: &mut Text, plot: &Plot, zone: Tz) {
    let span_s = (plot.t1 - plot.t0) / 1_000;
    let step_s = CLOCK_STEPS
        .into_iter()
        .find(|s| *s * CLOCK_LABELS >= span_s)
        .unwrap_or(86_400);
    let step = step_s * 1_000;
    let pattern = clock_pattern(step_s);
    let mut offset = format::offset_ms(zone, plot.t0);
    let mut local = (plot.t0 + offset).div_euclid(step) * step + step;
    let baseline = PLOT_BOTTOM + 12.0 + text.cap_height(SIZE_SMALL);
    let mut last = plot.t0;
    for _ in 0..64 {
        offset = format::offset_ms(zone, local - offset);
        let at = local - offset;
        if at >= plot.t1 {
            break;
        }
        local += step;
        // The hour a clock goes back is lived twice: one line for it, not two over each other.
        if at <= last {
            continue;
        }
        last = at;
        let x = plot.x(at);
        canvas.vline(x, PLOT_TOP, PLOT_BOTTOM, 2.0, RULE, 1.0);
        let label = format::clock(zone, at, pattern);
        let half = text.width(&label, SIZE_SMALL) / 2.0;
        if x - half >= PLOT_LEFT && x + half <= PLOT_RIGHT {
            text.draw(canvas, (x - half, baseline), &label, SIZE_SMALL, MUTED, 1.0);
        }
    }
}

/// How the clock is written for labels `step_s` apart: seconds below a minute, the date from six
/// hours up — a window that long spans days.
fn clock_pattern(step_s: i64) -> &'static str {
    if step_s < 60 {
        "%H:%M:%S"
    } else if step_s >= 21_600 {
        "%m-%d %H:%M"
    } else {
        "%H:%M"
    }
}

/// The traded quantity per column along the bottom, sold under bought. Returns the tallest
/// column, which the summary names.
fn volume(canvas: &mut Canvas, plot: &Plot, ticks: &[&Tick]) -> f64 {
    let columns = ((PLOT_RIGHT - PLOT_LEFT) / VOLUME_BAR).ceil() as usize + 1;
    let mut sums = vec![(0.0f64, 0.0f64); columns];
    for t in ticks {
        let column = ((plot.x(t.time_ms as i64) - PLOT_LEFT) / VOLUME_BAR) as usize;
        let sum = &mut sums[column.min(columns - 1)];
        match t.side {
            Side::Buy => sum.0 += f64::from(t.qty.abs()),
            Side::Sell => sum.1 += f64::from(t.qty.abs()),
        }
    }
    let peak = sums.iter().map(|s| s.0 + s.1).fold(0.0, f64::max);
    if peak <= 0.0 {
        return 0.0;
    }
    let tall = (PLOT_BOTTOM - PLOT_TOP) * VOLUME_SHARE;
    for (i, (bought, sold)) in sums.into_iter().enumerate() {
        let x = PLOT_LEFT + i as f32 * VOLUME_BAR;
        let sold_h = (sold / peak) as f32 * tall;
        let bought_h = (bought / peak) as f32 * tall;
        let right = (x + VOLUME_BAR - 1.0).min(PLOT_RIGHT);
        canvas.rect((x, PLOT_BOTTOM - sold_h), (right, PLOT_BOTTOM), SELL, 0.35);
        canvas.rect(
            (x, PLOT_BOTTOM - sold_h - bought_h),
            (right, PLOT_BOTTOM - sold_h),
            BUY,
            0.35,
        );
    }
    peak
}

/// A cross per print, or per pixel column its first, last, lowest and highest print once the
/// tape is longer than [`MAX_CROSSES`].
fn crosses(canvas: &mut Canvas, plot: &Plot, ticks: &[&Tick]) {
    let Some(mask) = cross_mask(CROSS_ARM, CROSS_WIDTH) else {
        return;
    };
    let shown: Vec<&Tick> = if ticks.len() <= MAX_CROSSES {
        ticks.to_vec()
    } else {
        column_extremes(ticks, plot)
    };
    for t in shown {
        let (x, y) = (plot.x(t.time_ms as i64), plot.y(f64::from(t.price)));
        let ink = match t.side {
            Side::Buy => BUY,
            Side::Sell => SELL,
        };
        canvas.blend_mask(&mask, (x.round() as i64, y.round() as i64), ink, 1.0);
    }
}

/// Per pixel column of the plot: its first, last, lowest and highest print, in time order.
fn column_extremes<'a>(ticks: &[&'a Tick], plot: &Plot) -> Vec<&'a Tick> {
    let columns = (PLOT_RIGHT - PLOT_LEFT).ceil() as usize + 1;
    let mut kept: Vec<[Option<usize>; 4]> = vec![[None; 4]; columns];
    for (i, t) in ticks.iter().enumerate() {
        let column = ((plot.x(t.time_ms as i64) - PLOT_LEFT) as usize).min(columns - 1);
        let k = &mut kept[column];
        k[0].get_or_insert(i);
        k[1] = Some(i);
        if k[2].is_none_or(|j| t.price < ticks[j].price) {
            k[2] = Some(i);
        }
        if k[3].is_none_or(|j| t.price > ticks[j].price) {
            k[3] = Some(i);
        }
    }
    let mut picked: Vec<usize> = kept.into_iter().flatten().flatten().collect();
    picked.sort_unstable();
    picked.dedup();
    picked.into_iter().map(|i| ticks[i]).collect()
}

/// The stop: a dashed line across the plot, its price at the left end.
fn stop_level(canvas: &mut Canvas, text: &mut Text, plot: &Plot, scale: &PriceScale, stop: f64) {
    let y = plot.y(stop);
    let mut x = PLOT_LEFT;
    while x < PLOT_RIGHT {
        canvas.hline(x, (x + 14.0).min(PLOT_RIGHT), y, 2.0, SELL, 0.8);
        x += 24.0;
    }
    let label = format!("STOP {}", format::exact_price(stop, scale.decimals));
    text.draw(
        canvas,
        (PLOT_LEFT + 8.0, y - 8.0),
        &label,
        SIZE_SMALL,
        SELL,
        1.0,
    );
}

/// The steps an order rested at up to its fill, and a riser where the fill was better than the
/// rest. Nothing without a recorded line.
fn order_steps(canvas: &mut Canvas, plot: &Plot, rests: &[(i64, f64)], fill: (i64, f64), ink: Rgb) {
    if fill.1 <= 0.0 || rests.is_empty() {
        return;
    }
    let end = plot.x(fill.0);
    for (i, &(at, price)) in rests.iter().enumerate() {
        let x0 = plot.x(at);
        let y = plot.y(price);
        let x1 = rests.get(i + 1).map_or(end, |next| plot.x(next.0)).max(x0);
        canvas.hline(x0, x1, y, ORDER_WIDTH, ink, 1.0);
        let next = rests.get(i + 1).map_or(fill.1, |next| next.1);
        let y_next = plot.y(next);
        if (y_next - y).abs() > 0.5 {
            canvas.vline(x1, y.min(y_next), y.max(y_next), ORDER_WIDTH, ink, 1.0);
        }
    }
}

/// Both fills: a dot, its name and price beside it, and a chip on the price axis. The entry's
/// label goes above its line and the exit's below, so two fills a second apart do not write over
/// each other; a label that would leave the plot turns to the other side of its dot. Two chips
/// closer than a chip's height are moved apart, the exit's away from the entry's.
fn fills(
    canvas: &mut Canvas,
    text: &mut Text,
    plot: &Plot,
    scale: &PriceScale,
    chart: &DealChart<'_>,
    exit_ink: Rgb,
) {
    let cap = text.cap_height(SIZE_SMALL);
    let mut taken: Option<(f32, f32, f32, f32)> = None;
    let mut chip_at: Option<f32> = None;
    for (name, (at, price), ink, above) in [
        ("ENTRY", chart.entry, INK, true),
        ("EXIT", chart.exit, exit_ink, false),
    ] {
        if price <= 0.0 {
            continue;
        }
        let (x, y) = (plot.x(at), plot.y(price));
        canvas.disc((x, y), DOT, ink, 1.0);
        let label = format!("{name} {}", format::exact_price(price, scale.decimals));
        let width = text.width(&label, SIZE_SMALL);
        let left = if x + DOT + 10.0 + width <= PLOT_RIGHT {
            x + DOT + 10.0
        } else {
            x - DOT - 10.0 - width
        };
        let mut baseline = if above { y - 14.0 } else { y + 14.0 + cap };
        let collides = |b: f32| {
            taken.is_some_and(|(l, t, r, bt)| left < r && left + width > l && b - cap < bt && b > t)
        };
        if collides(baseline) {
            baseline = if above { y + 14.0 + cap } else { y - 14.0 };
        }
        text.draw(canvas, (left, baseline), &label, SIZE_SMALL, ink, 1.0);
        taken = Some((left, baseline - cap, left + width, baseline));
        let chip_y = chip_place(y, chip_at);
        chip_at = Some(chip_y);
        chip(
            canvas,
            text,
            chip_y,
            &format::exact_price(price, scale.decimals),
            ink,
        );
    }
}

/// Where a chip for a price at `y` goes: on its price, or a chip's height off `other` when the two
/// would overlap — to the side the price lies on, or the other side where that leaves the plot.
fn chip_place(y: f32, other: Option<f32>) -> f32 {
    let half = CHIP_HEIGHT / 2.0;
    let inside = |c: f32| c.clamp(PLOT_TOP + half, PLOT_BOTTOM - half);
    let y = inside(y);
    let Some(other) = other.filter(|o| (y - o).abs() < CHIP_HEIGHT) else {
        return y;
    };
    let (near, far) = if y >= other {
        (other + CHIP_HEIGHT, other - CHIP_HEIGHT)
    } else {
        (other - CHIP_HEIGHT, other + CHIP_HEIGHT)
    };
    if inside(near) == near {
        near
    } else {
        inside(far)
    }
}

/// A price on the axis, on a plate of the fill's colour.
fn chip(canvas: &mut Canvas, text: &mut Text, y: f32, label: &str, ink: Rgb) {
    let cap = text.cap_height(SIZE_SMALL);
    let half = CHIP_HEIGHT / 2.0;
    canvas.rect((PLOT_RIGHT, y - half), (WIDTH as f32, y + half), ink, 1.0);
    text.draw(
        canvas,
        (PLOT_RIGHT + 12.0, y + cap / 2.0),
        label,
        SIZE_SMALL,
        CHIP_INK,
        1.0,
    );
}

/// The band on top: the market and its side on the left, the range and the day on the right.
fn header(canvas: &mut Canvas, text: &mut Text, chart: &DealChart<'_>, scale: &PriceScale) {
    let baseline = 24.0 + text.cap_height(SIZE_TITLE);
    text.draw(
        canvas,
        (PLOT_LEFT, baseline),
        chart.market,
        SIZE_TITLE,
        INK,
        1.0,
    );
    let side = if chart.short { "SHORT" } else { "LONG" };
    let x = PLOT_LEFT + text.width(chart.market, SIZE_TITLE) + 20.0;
    text.draw(canvas, (x, baseline), side, SIZE_TEXT, MUTED, 1.0);
    let right = format!(
        "range {:.1}%   {}",
        scale.range_percent(),
        format::clock(chart.zone, chart.entry.0, "%Y-%m-%d %H:%M:%S")
    );
    let width = text.width(&right, SIZE_SMALL);
    text.draw(
        canvas,
        (PLOT_RIGHT - width, baseline),
        &right,
        SIZE_SMALL,
        MUTED,
        1.0,
    );
}

/// The result in its colour, and under it what the window did, what the position cost, the
/// core's day and the busiest volume column.
fn summary(
    canvas: &mut Canvas,
    text: &mut Text,
    chart: &DealChart<'_>,
    ticks: &[&Tick],
    peak: f64,
) {
    let x = PLOT_LEFT + 14.0;
    let room = PLOT_RIGHT - x - 20.0;
    let first = PLOT_TOP + 14.0 + text.cap_height(SIZE_TEXT);
    let ink = if chart.won { BUY } else { SELL };
    let caption = text.fit(chart.caption, SIZE_TEXT, room);
    text.draw(canvas, (x, first), &caption, SIZE_TEXT, ink, 1.0);
    let mut parts = Vec::new();
    if let (Some(a), Some(b)) = (ticks.first(), ticks.last()) {
        let change = (f64::from(b.price) / f64::from(a.price) - 1.0) * 100.0;
        parts.push(format!("WINDOW {}%", format::signed_percent(change)));
    }
    if let Some(spent) = chart.spent_usd.filter(|v| *v > 0.0) {
        parts.push(format!("POS {} USDT", format::money(spent)));
    }
    if let Some(day) = chart.day_usd {
        parts.push(format!("DAY {} USDT", format::signed_money(day)));
    }
    if peak > 0.0 {
        parts.push(format!("VOL max {} {}", format::volume(peak), chart.base));
    }
    let second = first + 14.0 + text.cap_height(SIZE_SMALL) * 1.4;
    let stats = text.fit(&parts.join("   "), SIZE_SMALL, room);
    text.draw(canvas, (x, second), &stats, SIZE_SMALL, MUTED, 1.0);
}

#[cfg(test)]
mod tests;
