//! Toolbar value formatting for metric, size, and sell fields, plus mouse-wheel stepping.

use gpui::ScrollDelta;

/// Formats a value with two decimal places and a period separator, for example `50` as `50.00`.
pub fn fmt_field2(v: f32) -> String {
    format!("{v:.2}")
}

/// Formats a signed value for fields such as SL, which may be positive or negative. For example,
/// `1` becomes `+1.00` and `-20` becomes `-20.00`.
pub fn fmt_field2_signed(v: f32) -> String {
    format!("{v:+.2}")
}

/// Formats size and sell values with precision adapted to magnitude: values at least 100 use no
/// decimals, values at least 10 but below 100 use one, values at least 1 but below 10 use two, and
/// values below 1 use enough places for roughly two significant digits. Examples include 0.6,
/// 0.001, and 0.00001.
/// Trailing zeros are removed, which also hides floating-point noise such as 0.6000000238.
pub fn fmt_adaptive(v: f64) -> String {
    let a = v.abs();
    let decimals: usize = if a == 0.0 {
        0
    } else if a >= 100.0 {
        0
    } else if a >= 10.0 {
        1
    } else if a >= 1.0 {
        2
    } else {
        // Below one, retain roughly two significant digits: 0.6 uses 2 places, 0.001 uses 4,
        // and 0.00001 uses 6.
        let lead = (-a.log10().floor()) as i32; // Leading-place counts are 1, 3, and 5 respectively.
        (lead + 1).clamp(2, 8) as usize
    };
    let s = format!("{:.*}", decimals, v);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

pub(super) fn fmt_sell_pct(v: f64) -> String {
    let a = v.abs();
    if a >= 1000.0 {
        let k = v / 1000.0;
        let mut s = if k.abs() >= 100.0 {
            format!("{k:.0}")
        } else if k.abs() >= 10.0 {
            format!("{k:.1}")
        } else {
            format!("{k:.2}")
        };
        if s.contains('.') {
            s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        }
        format!("{s}k")
    } else {
        fmt_adaptive(v)
    }
}

/// Steps by magnitude using `step = frac * 10^floor(log10(v))`. A `frac` of 1.0 uses a full order
/// of magnitude: 18 to 20 to 30, 93 to 100 to 200, 980 to 1000 to 2000, or 0.001 to 0.002. A
/// `frac` of 0.5 uses half steps, such as 10 to 15 to 20 to 25 or 0.1 to 0.15 to 0.2. Up selects
/// the next multiple and down the previous one. At an exact power of ten, stepping down switches
/// to the lower order, allowing 111 to 100 to 90 to 80 instead of stopping at 100.
pub(super) fn wheel_step(value: f64, up: bool, frac: f64) -> f64 {
    if !(value > 0.0) {
        return value;
    }
    let step = frac * 10f64.powf(value.log10().floor());
    let raw = if up {
        ((value / step + 1e-9).floor() + 1.0) * step
    } else {
        let mut down = ((value / step - 1e-9).ceil() - 1.0) * step;
        if down <= 0.0 {
            // At an exact step such as 100 with frac=1, step down once using the lower order.
            let lower = frac * 10f64.powf((value * (1.0 - 1e-9)).log10().floor());
            down = value - lower;
        }
        if down <= 0.0 {
            return value;
        }
        down
    };
    (raw * 1e8).round() / 1e8
}

/// Vertical component of a wheel gesture (up = +Y).
///
/// `ScrollDelta` is two-dimensional, and the sign alone decides nothing: a zero Y is a horizontal
/// gesture, not "down". The caller (`strips::wheel_step_dir`) owns the interpretation of direction;
/// this only extracts the magnitude.
pub(super) fn scroll_dy(delta: ScrollDelta) -> f32 {
    match delta {
        ScrollDelta::Lines(p) => p.y,
        ScrollDelta::Pixels(p) => f32::from(p.y),
    }
}
