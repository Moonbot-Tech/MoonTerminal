//! Форматирование чисел для UI/feed.

/// Компактное число: точность `decimals`, хвостовые нули и точка срезаются
/// ("1.500000" → "1.5", "2.000000" → "2"). Нули режутся ТОЛЬКО в дробной
/// части: при `decimals=0` строка без точки, и слепой трим калечил целые
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

/// Компактное число с SI-суффиксом (K/M/B/T): 1_500 → «1.5K», 2_300_000 → «2.3M».
/// Значения меньше 1000 идут через [`adaptive`] (без суффикса). Хвостовые нули срезаются.
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
                format!("{n:.0}")
            } else if n.abs() >= 10.0 {
                format!("{n:.1}")
            } else {
                format!("{n:.2}")
            };
            let s = s.trim_end_matches('0').trim_end_matches('.');
            return format!("{s}{suffix}");
        }
    }
    adaptive(v)
}

/// Точность `decimals`, хвостовые нули срезаются, но МИНИМУМ один знак после точки
/// остаётся ("45.20" → "45.2", "45.00" → "45.0", "10000.000" → "10000.0").
fn trim_keep_one(v: f64, decimals: usize) -> String {
    let mut s = format!("{v:.decimals$}");
    if let Some(dot) = s.find('.') {
        let min_len = dot + 2; // точка + один знак
        while s.len() > min_len && s.ends_with('0') {
            s.pop();
        }
    }
    s
}

/// Количество для таблиц активов: знаков по величине (крупнее — меньше), максимум
/// тысячные, минимум десятые. "0.16206"→"0.162", "7.7972"→"7.797", "35483"→"35483.0".
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

/// Долларовая сумма (без символа): максимум сотые, минимум десятые.
/// "45.238"→"45.24", "45.2"→"45.2", "10176"→"10176.0".
pub fn usd(v: f64) -> String {
    trim_keep_one(v, 2)
}

/// Адаптивное число под размер/цену: точность подбирается по величине, а не фиксирована.
/// Крупные значения — без дробной части (5000000.0001 → "5000000", 5000 → "5000");
/// мелкие — с достаточным числом знаков, чтобы значащие цифры были видны
/// (0.0000001 → "0.0000001"). `sig` — желаемое число значащих цифр (для дробной части).
pub fn adaptive(v: f64) -> String {
    let a = v.abs();
    if a == 0.0 {
        return "0".to_string();
    }
    // Тысячи и больше — без дробной части.
    if a >= 1000.0 {
        return compact(v, 0);
    }
    const SIG: i32 = 5;
    // Экспонента старшего разряда: для a<1 отрицательна (0.0001 → -4).
    let exp = a.log10().floor() as i32;
    // Знаков после запятой = столько, чтобы набрать SIG значащих цифр (с запасом
    // на ведущие нули у мелких чисел). Ограничиваем сверху на разумный максимум.
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
mod tests {
    use super::*;

    #[test]
    fn compact_keeps_integer_zeros() {
        // Регрессия: слепой трим нулей калечил целые (330 → «33», 1000 → «1»).
        assert_eq!(compact(330.0, 0), "330");
        assert_eq!(compact(1000.0, 0), "1000");
        assert_eq!(compact(0.0, 0), "0");
        assert_eq!(compact(-500.0, 0), "-500");
        // Fractional trailing zeros are trimmed.
        assert_eq!(compact(1.5, 6), "1.5");
        assert_eq!(compact(2.0, 6), "2");
        assert_eq!(compact(45.20, 2), "45.2");
    }

    #[test]
    fn adaptive_thousands_intact() {
        assert_eq!(adaptive(25000.0), "25000");
        assert_eq!(adaptive(1000.0), "1000");
    }

    #[test]
    fn group_thousands_splits_by_three() {
        assert_eq!(group_thousands("1111"), "1 111");
        assert_eq!(group_thousands("999"), "999");
        assert_eq!(group_thousands("1000000"), "1 000 000");
        assert_eq!(group_thousands(""), "");
    }

    #[test]
    fn group_thousands_keeps_sign_out_of_the_grouping() {
        // The leading sign does not participate in three-digit grouping.
        assert_eq!(group_thousands("-123456"), "-123 456");
        assert_eq!(group_thousands("-1234"), "-1 234");
        assert_eq!(group_thousands("-999"), "-999");
    }

    #[test]
    fn signed_pct_rounds_before_signing() {
        assert_eq!(signed_pct(2.04, 1).unwrap().0, "+2.0%");
        assert_eq!(signed_pct(-0.31, 1).unwrap().0, "-0.3%");
        // A small negative rounds to canonical, unsigned zero.
        assert_eq!(
            signed_pct(-0.04, 1).unwrap(),
            ("0.0%".to_string(), DeltaSign::Zero)
        );
        // Literal -0.0 is canonicalized and classified as zero.
        assert_eq!(
            signed_pct(-0.0, 1).unwrap(),
            ("0.0%".to_string(), DeltaSign::Zero)
        );
        assert_eq!(signed_pct(0.0, 2).unwrap().0, "0.00%");
    }

    #[test]
    fn signed_pct_rejects_what_cannot_be_a_percentage() {
        assert!(signed_pct(f64::NAN, 1).is_none());
        assert!(signed_pct(f64::INFINITY, 1).is_none());
        // A finite input is rejected when scaling makes the rounded result non-finite.
        assert!(signed_pct(f64::MAX, 1).is_none());
        assert!(signed_pct(f64::MAX / 5.0, 1).is_none());
        // Just inside the range: still a real number, still formatted.
        assert!(signed_pct(1e300, 1).is_some());
    }

    #[test]
    fn group_decimal_groups_only_the_integer_part() {
        assert_eq!(group_decimal("19983.48"), "19 983.48");
        assert_eq!(group_decimal("999.5"), "999.5");
    }

    #[test]
    fn usd_grouped_is_one_precision_for_every_amount_surface() {
        // The header balance and the Assets panel must not drift on the same figure.
        assert_eq!(usd_grouped(19983.48), "19 983.48");
        assert_eq!(usd_grouped(19983.50), "19 983.5");
        assert_eq!(usd_grouped(-1234.0), "-1 234.0");
    }

    #[test]
    fn pct_shares_the_signed_rounding_without_the_sign() {
        assert_eq!(pct(2.04, 1).unwrap().0, "2.0%");
        assert_eq!(pct(-0.31, 1).unwrap().0, "-0.3%");
        // The no-forced-plus form also canonicalizes a rounded zero.
        assert_eq!(
            pct(-0.04, 1).unwrap(),
            ("0.0%".to_string(), DeltaSign::Zero)
        );
        assert!(pct(f64::NAN, 1).is_none());
    }
}
