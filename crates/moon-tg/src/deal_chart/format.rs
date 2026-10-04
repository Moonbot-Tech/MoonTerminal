//! Numbers and clocks as the picture writes them.

use chrono::{DateTime, Offset, TimeZone};
use chrono_tz::Tz;

/// A dollar amount: two decimals, none from a hundred up — decided on the amount as it rounds,
/// so 99.996 is 100, not 100.00.
fn dollars(v: f64) -> String {
    if ((v * 100.0).round() / 100.0).abs() >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// A dollar amount with its sign. A value that rounds to zero is written flat, without a sign.
pub(crate) fn signed_money(v: f64) -> String {
    let body = dollars(v.abs());
    if body.chars().all(|c| c == '0' || c == '.') {
        body
    } else if v < 0.0 {
        format!("-{body}")
    } else {
        format!("+{body}")
    }
}

/// A dollar amount without a sign, as a position size is.
pub(super) fn money(v: f64) -> String {
    dollars(v)
}

/// A percent with two decimals and its sign, flat when it rounds to zero.
pub(crate) fn signed_percent(v: f64) -> String {
    let body = format!("{:.2}", v.abs());
    if body == "0.00" {
        body
    } else if v < 0.0 {
        format!("-{body}")
    } else {
        format!("+{body}")
    }
}

/// A volume in a few characters: 0.0312, 4.25, 950, 1.2K, 3.4M, 5.6B — a quantity of the base
/// coin, which for a coin like BTC is a fraction. The unit is picked after rounding, so 999 960 is
/// 1.0M, not 1000.0K.
pub(super) fn volume(v: f64) -> String {
    // A unit is taken once the next smaller one, as written, would reach a thousand: whole
    // numbers below a K, one decimal below an M or a B.
    let abs = v.abs();
    let reaches =
        |smaller: f64, decimals: f64| (abs / smaller * decimals).round() >= 1_000.0 * decimals;
    if reaches(1e6, 10.0) {
        format!("{:.1}B", v / 1e9)
    } else if reaches(1e3, 10.0) {
        format!("{:.1}M", v / 1e6)
    } else if reaches(1.0, 1.0) {
        format!("{:.1}K", v / 1e3)
    } else if abs == 0.0 {
        "0".to_owned()
    } else {
        // Three significant digits however small, counted after rounding: 99.96 is 100, not 100.0.
        let decimals = (2 - abs.log10().floor() as i64).clamp(0, 12) as usize;
        let text = format!("{v:.decimals$}");
        let digits = text
            .chars()
            .filter(char::is_ascii_digit)
            .skip_while(|c| *c == '0')
            .count();
        if digits > 3 && decimals > 0 {
            format!("{v:.0$}", decimals - 1)
        } else {
            text
        }
    }
}

/// How many decimals tell the lines of a grid `step` apart: one past the step's first digit.
pub(super) fn step_decimals(step: f64) -> usize {
    if !(step.is_finite() && step > 0.0) {
        return 2;
    }
    (-step.log10().floor()).max(0.0) as usize + 1
}

/// `price` with `decimals` decimals, trailing zeros kept so the axis lines up.
pub(super) fn price(price: f64, decimals: usize) -> String {
    format!("{price:.decimals$}")
}

/// `price` with as many decimals as it needs, at least `decimals`: the entry and the exit are
/// exact prices, not grid lines.
pub(super) fn exact_price(price: f64, decimals: usize) -> String {
    // The shortest text that reads back as this price: a fixed number of decimals would print
    // the tail of the binary fraction, 64123.450000000004 for 64123.45.
    let shortest = format!("{price}");
    let have = shortest.split('.').nth(1).map_or(0, str::len);
    format!("{price:.0$}", have.max(decimals).min(12))
}

/// How far `zone` is ahead of UTC at `unix_ms`, in milliseconds.
pub(super) fn offset_ms(zone: Tz, unix_ms: i64) -> i64 {
    DateTime::from_timestamp_millis(unix_ms).map_or(0, |at| {
        i64::from(
            zone.offset_from_utc_datetime(&at.naive_utc())
                .fix()
                .local_minus_utc(),
        ) * 1_000
    })
}

/// The moment in `zone` with `pattern` (chrono's `strftime`).
pub(super) fn clock(zone: Tz, unix_ms: i64, pattern: &str) -> String {
    DateTime::from_timestamp_millis(unix_ms)
        .map(|at| at.with_timezone(&zone).format(pattern).to_string())
        .unwrap_or_default()
}
