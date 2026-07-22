//! Pure axis-layout logic: "nice" price/time grid intervals, decimal precision,
//! and clock formatting. UI-agnostic (no gpui) — the GPUI wrapper
//! (`moon-ui-gpui`) draws the scales on top using ticks from here. Ported from moonweb
//! (`coords.ts` niceInterval/priceDecimals).

/// Snapshot of the view state captured for the current GPUI/chartdx scale consumer.
/// Values come from the current render frame after that frame is rendered.
#[derive(Clone, Copy)]
pub struct AxisSnapshot {
    /// Physical pixels per millisecond — the width of the time window.
    pub px_per_ms: f32,
    /// Fraction of the "future" window on the right (right_margin_frac).
    pub right_margin_frac: f32,
    /// Price at the center of the area and the visible range.
    pub render_center: f32,
    pub render_range: f32,
    /// Time origin (unix ms) and the time at the right anchor (unix ms).
    pub epoch_ms: f64,
    pub right_time_ms: f64,
    /// Local time offset from UTC in seconds (for clock labels).
    pub tz_offset_sec: i64,
}

/// "Nice" price-grid interval for approximately `target_lines` lines (ported from niceInterval).
pub fn nice_interval(range: f32, target_lines: f32) -> f32 {
    let rough = range / target_lines.max(1.0);
    if !(rough > 0.0) {
        return 1.0;
    }
    let mag = 10f32.powf(rough.log10().floor());
    let n = rough / mag;
    let nice = if n < 1.5 {
        1.0
    } else if n < 3.0 {
        2.0
    } else if n < 7.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

/// Number of decimal places for a price of this magnitude (ported from priceDecimals).
pub fn price_decimals(price: f32) -> usize {
    let p = price.abs();
    if p >= 1000.0 {
        1
    } else if p >= 10.0 {
        2
    } else if p >= 1.0 {
        3
    } else {
        4
    }
}

/// "Nice" time interval in seconds that fits approximately `target` labels.
pub fn nice_time_step(window_sec: f64, target: f64) -> f64 {
    const STEPS: [f64; 16] = [
        1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0,
        14400.0, 21600.0,
    ];
    let want = window_sec / target.max(1.0);
    for s in STEPS {
        if s >= want {
            return s;
        }
    }
    STEPS[STEPS.len() - 1]
}

/// Local-time clock from unix ms (HH:MM:SS or HH:MM).
pub fn fmt_clock(unix_ms: f64, offset_sec: i64, with_sec: bool) -> String {
    let total = (unix_ms / 1000.0).floor() as i64 + offset_sec;
    let sod = ((total % 86_400) + 86_400) % 86_400;
    let (h, m, s) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    if with_sec {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{h:02}:{m:02}")
    }
}

/// Clock and date when the day is NOT today: "DD.MM HH:MM:SS" (on larger timeframes/windows,
/// the cursor moves across previous days, making a time without a date ambiguous).
pub fn fmt_clock_dated(unix_ms: f64, offset_sec: i64, with_sec: bool, now_ms: f64) -> String {
    let day_of = |ms: f64| ((ms / 1000.0).floor() as i64 + offset_sec).div_euclid(86_400);
    let clock = fmt_clock(unix_ms, offset_sec, with_sec);
    if day_of(unix_ms) == day_of(now_ms) {
        return clock;
    }
    let (_, month, day) = civil_from_days(day_of(unix_ms));
    format!("{day:02}.{month:02} {clock}")
}

/// Gregorian date from the number of days since the unix epoch (civil_from_days algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
