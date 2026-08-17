//! Number formatting for the UI and feed.

/// Format a compact number to `decimals` places, trimming trailing fractional zeros and the point
/// ("1.500000" → "1.5", "2.000000" → "2"). Zeros are trimmed ONLY from the fractional
/// part: with `decimals=0`, the string has no point, and blindly trimming zeros used to corrupt
/// integers
/// ("330" → "33", "1000" → "1").
pub fn compact(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if !s.contains('.') {
        return s;
    }
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// Format a compact number with an SI suffix (K/M/B/T): 1_500 → "1.5K", 2_300_000 → "2.3M".
/// Values below 1000 use [`adaptive`] without a suffix.
///
/// Trailing zeros are trimmed from the FRACTION only, through [`compact`]. Trimming the whole
/// mantissa used to eat a mantissa's own zeros — 100_000 rendered as "1K" rather than "100K", a
/// hundredfold understatement of any value that happened to land on a round hundred.
pub fn compact_si(v: f64) -> String {
    let a = v.abs();
    if a < 1000.0 {
        return adaptive(v);
    }
    const UNITS: [(f64, &str); 4] = [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K")];
    for (scale, suffix) in UNITS {
        if a >= scale {
            let n = v / scale;
            let s = if n.abs() >= 100.0 {
                compact(n, 0)
            } else if n.abs() >= 10.0 {
                compact(n, 1)
            } else {
                compact(n, 2)
            };
            return format!("{s}{suffix}");
        }
    }
    adaptive(v)
}

/// Format to `decimals` places and trim trailing zeros. When formatting includes a decimal point
/// (`decimals > 0` for the current finite callers, which pass 1–3), AT LEAST one fractional digit
/// remains ("45.20" → "45.2", "45.00" → "45.0", "10000.000" → "10000.0"). With
/// `decimals=0`, no decimal point or fractional digit is added.
fn trim_keep_one(v: f64, decimals: usize) -> String {
    let mut s = format!("{v:.decimals$}");
    if let Some(dot) = s.find('.') {
        let min_len = dot + 2; // Decimal point plus one digit.
        while s.len() > min_len && s.ends_with('0') {
            s.pop();
        }
    }
    s
}

/// Format a quantity for asset tables with magnitude-based precision (larger values use fewer
/// places): at most thousandths and at least tenths. "0.16206"→"0.162", "7.7972"→"7.797",
/// "35483"→"35483.0".
pub fn qty(v: f64) -> String {
    let a = v.abs();
    let decimals = if a >= 100.0 {
        1
    } else if a >= 10.0 {
        2
    } else {
        3
    };
    trim_keep_one(v, decimals)
}

/// Format a dollar amount without the symbol, using at most hundredths and at least tenths.
/// "45.238"→"45.24", "45.2"→"45.2", "10176"→"10176.0".
pub fn usd(v: f64) -> String {
    trim_keep_one(v, 2)
}

/// Format a size or price with precision selected by magnitude rather than fixed precision.
/// Large values have no fractional part (5000000.0001 → "5000000", 5000 → "5000"); small
/// values retain enough places to show their significant digits (0.0000001 → "0.0000001").
/// `SIG` is the desired number of significant digits.
pub fn adaptive(v: f64) -> String {
    let a = v.abs();
    if a == 0.0 {
        return "0".to_string();
    }
    // Thousands and larger values have no fractional part.
    if a >= 1000.0 {
        return compact(v, 0);
    }
    const SIG: i32 = 5;
    // Exponent of the most significant digit; negative for a<1 (0.0001 → -4).
    let exp = a.log10().floor() as i32;
    // Use enough decimal places to reach SIG significant digits, including the leading zeros of
    // small values. Cap the result at a reasonable maximum.
    let decimals = (SIG - 1 - exp).clamp(0, 18) as usize;
    compact(v, decimals)
}

/// Append `int` to `out` in space-separated groups of three.
///
/// A leading `-` is kept out of the grouping: counting the sign as a digit shifts every separator
/// and only happens to look right for 4- and 5-digit values.
fn push_grouped(out: &mut String, int: &str) {
    let (sign, digits) = match int.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", int),
    };
    out.push_str(sign);
    let len = digits.len();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
}

/// Split an ASCII integer string into space-separated groups of three: "1111" -> "1 111".
///
/// A leading minus sign is preserved and excluded from the digit count.
pub fn group_thousands(int: &str) -> String {
    let mut out = String::with_capacity(int.len() + int.len() / 3);
    push_grouped(&mut out, int);
    out
}

/// Group the integer part of an ASCII decimal string: "19983.48" -> "19 983.48".
///
/// Takes the whole number rather than its integer part so callers do not each repeat the
/// split-group-rejoin dance.
pub fn group_decimal(s: &str) -> String {
    match s.split_once('.') {
        Some((int, frac)) => {
            let mut out = String::with_capacity(int.len() + int.len() / 3 + 1 + frac.len());
            push_grouped(&mut out, int);
            out.push('.');
            out.push_str(frac);
            out
        }
        None => group_thousands(s),
    }
}

/// Format a USDT amount with [`usd`] precision and space-grouped thousands.
///
/// For example, `19983.5` becomes `"19 983.5"`. The terminal header and Assets panel share this
/// formatter so the same account figure keeps identical precision on both surfaces.
pub fn usd_grouped(v: f64) -> String {
    group_decimal(&usd(v))
}

/// Sign of a formatted delta, classified from the ROUNDED value so the text a caller renders and
/// the colour it picks cannot disagree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeltaSign {
    Positive,
    Negative,
    Zero,
}

impl DeltaSign {
    /// Pick the per-sign member of a triple — a palette colour, a `MoonTone`, anything.
    ///
    /// Callers map a sign onto differing visual types, and a surface whose convention is inverted
    /// (negative funding is the good case) says so by swapping two arguments rather than
    /// re-spelling the match with its meaning buried in the arms.
    pub fn pick<T>(self, positive: T, negative: T, zero: T) -> T {
        match self {
            Self::Positive => positive,
            Self::Negative => negative,
            Self::Zero => zero,
        }
    }
}

/// Round to `decimals`, returning `None` when the input or rounded result is non-finite.
///
/// `v * scale` overflows to infinity for magnitudes above `f64::MAX / scale`, so a finite input
/// can leave the rounding non-finite. Any zero result is canonicalized to positive `0.0`, ensuring
/// that formatting cannot expose a negative zero after a small negative value rounds to zero.
pub fn round_to(v: f64, decimals: usize) -> Option<f64> {
    if !v.is_finite() {
        return None;
    }
    let scale = 10f64.powi(decimals as i32);
    let rounded = (v * scale).round() / scale;
    if !rounded.is_finite() {
        return None;
    }
    // Canonical zero: any small negative rounds to -0.0, which `format!` prints WITH a minus —
    // the exact misreading this module exists to remove.
    Some(if rounded == 0.0 { 0.0 } else { rounded })
}

/// Percentage without a forced plus sign, using [`signed_pct`]'s rounding rules.
///
/// Positive values render as `"2.0%"`, negative values as `"-0.3%"`, and zero as `"0.0%"`.
/// The returned [`DeltaSign`] still classifies the rounded value so callers can style the exact
/// sign represented by the text. Returns `None` when rounding produces no finite result.
pub fn pct(v: f64, decimals: usize) -> Option<(String, DeltaSign)> {
    let rounded = round_to(v, decimals)?;
    let sign = classify(rounded);
    Some((format!("{:.*}%", decimals, rounded), sign))
}

/// Signed compact amount whose sign is classified from the ROUNDED value.
///
/// A raw `v >= 0.0` disagrees with the text it labels the moment a small negative rounds away: the
/// amount reads `0.00` while the prefix — and any colour picked the same way — still says negative.
/// Rounding first removes that, and returning the classification lets a caller tint exactly the
/// sign its own text shows. A non-finite input has no sign worth stating and renders as zero.
///
/// Sign and digits therefore share ONE rounding rule — [`round_to`]'s half-away-from-zero, as
/// [`pct`] and [`signed_pct`] already use — rather than `{:.*}`'s half-to-even. Splitting them
/// would print an exact `x.xx5` a hair closer while reopening the same desync at the next midpoint
/// down, so an exactly-representable midpoint rounds up here. Display only: the report export
/// writes raw values.
///
/// Args:
///     v: Raw signed amount.
///     decimals: Places to round and format to.
///
/// Returns:
///     The formatted amount with an explicit `+`/`-`, and the sign that text represents.
pub fn signed_amount(v: f64, decimals: usize) -> (String, DeltaSign) {
    let rounded = round_to(v, decimals).unwrap_or(0.0);
    let sign = if rounded < 0.0 { "-" } else { "+" };
    (
        format!("{sign}{}", compact(rounded.abs(), decimals)),
        classify(rounded),
    )
}

/// Signed amount at FIXED decimals, classified from the ROUNDED value.
///
/// Identical in contract to [`signed_amount`] — one rounding rule feeds both the digits and the
/// sign, so text and colour cannot disagree — but the fraction keeps every place instead of being
/// trimmed. A right-aligned money COLUMN needs that: [`compact`]'s trimming renders `12.00` as
/// `12` and `12.50` as `12.5`, so the decimal points in a table stop lining up and two rows of the
/// same magnitude no longer read as comparable. Prefer [`signed_amount`] for prose and single
/// figures; reach for this one when the value sits in a column.
///
/// A value that rounds to zero is rendered UNSIGNED and classified [`DeltaSign::Zero`]: a `+` there
/// would claim a gain the figure does not show. This mirrors [`signed_pct`] exactly.
///
/// Args:
///     v: Raw signed amount.
///     decimals: Places to round and format to.
///
/// Returns:
///     The formatted amount and the sign that text represents, or `None` when rounding produces no
///     finite result, so each caller supplies its own placeholder.
pub fn signed_fixed(v: f64, decimals: usize) -> Option<(String, DeltaSign)> {
    let rounded = round_to(v, decimals)?;
    match classify(rounded) {
        DeltaSign::Zero => Some((format!("{:.*}", decimals, rounded), DeltaSign::Zero)),
        sign => Some((format!("{:+.*}", decimals, rounded), sign)),
    }
}

/// Classify an already-rounded value.
fn classify(rounded: f64) -> DeltaSign {
    if rounded == 0.0 {
        DeltaSign::Zero
    } else if rounded < 0.0 {
        DeltaSign::Negative
    } else {
        DeltaSign::Positive
    }
}

/// Signed percentage rounded to `decimals`: "+2.0%" / "-0.3%" / "0.0%".
///
/// Rounding precedes both formatting and classification. A plain `{:+.1}` prints a misleading
/// "-0.0%" for any small negative, and for a literal `-0.0` pairs that minus with a positive
/// classification (`-0.0 < 0.0` is false) — a red-vs-green disagreement on the same cell. A value
/// that rounds to zero is therefore rendered unsigned and classified [`DeltaSign::Zero`].
/// Returns `None` for non-finite input so each caller supplies its own placeholder.
pub fn signed_pct(v: f64, decimals: usize) -> Option<(String, DeltaSign)> {
    let rounded = round_to(v, decimals)?;
    match classify(rounded) {
        // Unsigned at zero: a "+" there would claim a gain the figure does not show.
        DeltaSign::Zero => Some((format!("{:.*}%", decimals, rounded), DeltaSign::Zero)),
        sign => Some((format!("{:+.*}%", decimals, rounded), sign)),
    }
}

#[cfg(test)]
mod tests;
