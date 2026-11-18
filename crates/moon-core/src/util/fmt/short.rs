//! The one shortening of large figures (`12.3 k`, `1.2 m`) every surface prints through.
//!
//! Two independent choices make up a shortened figure, and they are owned by different parties:
//!
//! - the [`Form`] — how many digits survive — belongs to the PLACE. A table column is read down
//!   its rows and needs one decimal on every row so the points line up; a lone caption is read on
//!   its own and wants the fewest characters. The caller states its role; the user cannot turn a
//!   column into a caption.
//! - the [`Notation`] — the unit letter and the space before it — belongs to the USER. It is one
//!   process-wide setting, read here, so no surface has to carry it down to its formatter.
//!
//! Before this module each surface kept its own copy: eleven functions, three letter cases, and a
//! `999 999` that printed as `1M`, `1000k` or `1000.00K` depending on the panel (#1390).

use std::sync::atomic::{AtomicU8, Ordering};

use super::{adaptive, compact, round_to, trim_keep_one};

/// How the unit of a shortened figure is written.
///
/// The digits are the [`Form`]'s; this decides only the letter and the space before it. Thousands,
/// millions, billions and trillions are `k`/`m`/`b`/`t` in either notation — only the spelling
/// differs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Notation {
    /// MoonBot's spelling: a lowercase letter after a space, `12.3 k`, `1.2 m`. The default,
    /// because a MoonBot user reads both programs side by side.
    #[default]
    MoonBot,
    /// The terminal's earlier spelling: an uppercase letter with no space, `12.3K`, `1.2M`.
    Compact,
}

impl Notation {
    /// Every notation, in the order the Settings dropdown lists them.
    pub const ALL: [Self; 2] = [Self::MoonBot, Self::Compact];

    /// Stable code persisted in `settings.toml`.
    pub const fn code(self) -> &'static str {
        match self {
            Self::MoonBot => "moonbot",
            Self::Compact => "compact",
        }
    }

    /// Parse a `settings.toml` code, returning `None` for an unknown one.
    pub fn from_code(code: &str) -> Option<Self> {
        match code.trim() {
            "moonbot" => Some(Self::MoonBot),
            "compact" => Some(Self::Compact),
            _ => None,
        }
    }

    /// The spelled unit at `tier`, an index into [`SCALES`].
    fn unit(self, tier: usize) -> &'static str {
        match self {
            Self::MoonBot => [" t", " b", " m", " k"][tier],
            Self::Compact => ["T", "B", "M", "K"][tier],
        }
    }

    /// The wire value of [`NOTATION`].
    const fn as_u8(self) -> u8 {
        match self {
            Self::MoonBot => 0,
            Self::Compact => 1,
        }
    }

    /// Inverse of [`Self::as_u8`]; anything unknown is the default.
    const fn from_u8(raw: u8) -> Self {
        match raw {
            1 => Self::Compact,
            _ => Self::MoonBot,
        }
    }
}

/// How many digits a shortened figure keeps, chosen by the place that prints it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// A figure standing alone — a chart caption, a tile, a button: three significant digits and a
    /// unit from a thousand up (`950`, `1.23 k`, `163 k`, `12.3 m`). Below a thousand the figure
    /// prints whole, through [`adaptive`].
    Label,
    /// A figure in a table column, read against the rows above and below it: MoonBot's report
    /// ladder. Two places under ten, one place under ten thousand, then one place with a unit
    /// (`3.46`, `950.0`, `1234.6`, `12.3 k`, `188.8 m`). Every value carries a decimal point, so
    /// the points of one column line up.
    Column,
    /// A label on the chart's volume scale, where neighbours are compared by eye: one density per
    /// tier and a `k` from zero (`0.86 k`, `16 k`, `875.3 m`). Issue #579.
    Scale,
}

/// Unit scales, largest first; [`Notation::unit`] spells the matching index.
const SCALES: [f64; 4] = [1e12, 1e9, 1e6, 1e3];

/// Index of the thousands tier in [`SCALES`].
const KILO: usize = SCALES.len() - 1;

/// The process-wide [`Notation`], as [`Notation::as_u8`].
///
/// A global and not a parameter, the way the interface language is: forty-odd surfaces across the
/// chart's text pass, tables and tiles print shortened figures, most of them far from anything
/// that holds the configuration, and threading one more argument through every one of them is how
/// a surface ends up forgetting it. Written at startup and after a saved Settings change; read on
/// every call. A relaxed load is enough — the value is a display preference, and a frame drawn
/// with the old one is replaced by the refresh that follows the write.
static NOTATION: AtomicU8 = AtomicU8::new(Notation::MoonBot.as_u8());

/// Make `notation` the one every [`short`] call prints with.
///
/// Only the application's startup and its Settings save call this. Tests never do: they run in
/// parallel inside one process, so they state their notation through [`short_in`] instead.
pub fn set_notation(notation: Notation) {
    NOTATION.store(notation.as_u8(), Ordering::Relaxed);
}

/// The notation [`short`] currently prints with.
///
/// Also the key a cache of formatted text folds in, so a Settings change reformats it.
pub fn notation() -> Notation {
    Notation::from_u8(NOTATION.load(Ordering::Relaxed))
}

/// Shorten `v` in `form`, in the user's notation.
///
/// Args:
///     v: The figure. A negative value keeps its sign; the tiers are chosen on its magnitude.
///     form: The role of the place that prints it.
///
/// Returns:
///     The shortened text. A non-finite value prints as Rust prints it (`NaN`, `inf`), never as a
///     plausible figure.
pub fn short(v: f64, form: Form) -> String {
    short_in(v, form, notation())
}

/// [`short`] with the notation stated rather than read.
///
/// For the Settings example, which shows a notation before it is saved, and for tests.
///
/// Args:
///     v: The figure.
///     form: The role of the place that prints it.
///     notation: The spelling of the unit.
///
/// Returns:
///     The shortened text.
pub fn short_in(v: f64, form: Form, notation: Notation) -> String {
    match form {
        Form::Label => label(v, notation),
        Form::Column => column(v, notation),
        Form::Scale => scale(v, notation),
    }
}

/// [`Form::Label`]: three significant digits, a unit from a thousand up.
///
/// The fraction is trimmed, the mantissa never is: `100 k` stays `100 k`, not `1 k`. A mantissa that
/// rounds up to a thousand prints in the next unit (`999 600` is `1 m`, not `1000 k`); trillions
/// have no next unit and keep four digits. The same carry holds at the first unit: a figure that
/// [`adaptive`]'s two places round up to a thousand (`999.996`) prints `1 k`, never a bare `1000`.
fn label(v: f64, notation: Notation) -> String {
    let a = v.abs();
    if !v.is_finite() || round_to(a, 2).unwrap_or(a) < 1000.0 {
        return adaptive(v);
    }
    let tier = tier_of(a);
    let m = v / SCALES[tier];
    // `a` is below the next scale, so a carried mantissa is exactly one of it.
    if m.abs().round() >= 1000.0 && tier > 0 {
        let sign = if v < 0.0 { "-" } else { "" };
        return format!("{sign}1{}", notation.unit(tier - 1));
    }
    let digits = if m.abs() >= 100.0 {
        compact(m, 0)
    } else if m.abs() >= 10.0 {
        compact(m, 1)
    } else {
        compact(m, 2)
    };
    format!("{digits}{}", notation.unit(tier))
}

/// [`Form::Column`]: MoonBot's report ladder, chosen on the magnitude before rounding.
///
/// MoonBot's report SELECT prints `hourlyVol`, `HVolFast` and `dailyVol` this way: `Round(v, 2)`
/// below 10, `Round(v, 1)` below 10 000, then thousands or millions to one place. Rounding is
/// half away from zero, as SQLite's `ROUND` is, and a whole number keeps its `.0` the way the
/// SELECT's cast to text keeps it. Two departures, both decided for the terminal: a figure that
/// rounds up to the next tier prints there (`999 999` is `1.0 m`, where MoonBot prints
/// `1000.0 k`; `9999.96` is `10.0 k`, where MoonBot prints `10000.0`), so no column ever grows a
/// fifth whole digit, and billions have their own letter (`1.5 b`, where MoonBot prints
/// `1500.0 m`). A negative value is tiered on its magnitude; MoonBot's turnover is never
/// negative, so it has no rule.
fn column(v: f64, notation: Notation) -> String {
    if !v.is_finite() {
        return format!("{v}");
    }
    let a = v.abs();
    if a < 10.0 {
        return trim_keep_one(round_to(v, 2).unwrap_or(0.0), 2);
    }
    if a < 10_000.0 {
        let rounded = round_to(v, 1).unwrap_or(0.0);
        if rounded.abs() < 10_000.0 {
            return trim_keep_one(rounded, 1);
        }
    }
    let mut tier = tier_of(a);
    loop {
        let raw = v / SCALES[tier];
        let m = round_to(raw, 1).unwrap_or(raw);
        if m.abs() >= 1000.0 && tier > 0 {
            tier -= 1;
            continue;
        }
        return format!("{m:.1}{}", notation.unit(tier));
    }
}

/// [`Form::Scale`]: precision fixed by tier so neighbouring scale labels read at one density.
///
/// - below `1 k` — two places, and the `k` is still there: `0.86 k`, `0.03 k`;
/// - `1–9.99 k` — one place: `1.6 k`;
/// - `10–999 k` — none: `16 k`, `163 k`;
/// - `m`, `b` and `t` — one place at any mantissa: `1.2 m`, `875.3 m`, `15.4 b`.
///
/// A fraction that is ALL zeros is dropped (`1 b`, not `1.0 b`); a fraction with a zero in its
/// last place is kept (`0.50 k`), because that zero holds the tier's density. Rounding picks the
/// tier: `999.6 k` prints as `1 m`, never `1000 k`.
///
/// Under five units the two-place mantissa would round to `0.00 k` and print a real turnover as
/// nothing — the everyday case on a BTC-quoted market, where the whole band lives below one coin.
/// Such a value prints bare through [`adaptive`] (`0.125`, `4.9`).
///
/// Scale labels ONLY: a profit of `12.5$` as `0.01 k$` would trade a meaningful figure for a
/// uniform one.
fn scale(v: f64, notation: Notation) -> String {
    const BARE_BELOW: f64 = 5.0;
    let a = v.abs();
    // A non-finite input has no tier; it prints whatever `adaptive` makes of it rather than an
    // `inf t` that looks like a figure.
    if !v.is_finite() || a < BARE_BELOW {
        return adaptive(v);
    }
    // Everything under a thousand still prints in `k`, so the search falls through to it.
    let mut tier = tier_of(a);
    loop {
        let n = v / SCALES[tier];
        let decimals = if tier != KILO {
            1
        } else if n.abs() < 1.0 {
            2
        } else if n.abs() < 10.0 {
            1
        } else {
            0
        };
        let rounded = round_to(n, decimals).unwrap_or(n);
        // Rounding carried the mantissa into the next unit: print it there.
        if rounded.abs() >= 1000.0 && tier > 0 {
            tier -= 1;
            continue;
        }
        let mut s = format!("{rounded:.decimals$}");
        if let Some(dot) = s.find('.') {
            if s[dot + 1..].bytes().all(|b| b == b'0') {
                s.truncate(dot);
            }
        }
        return format!("{s}{}", notation.unit(tier));
    }
}

/// The largest tier `a` reaches, or thousands when it reaches none.
fn tier_of(a: f64) -> usize {
    SCALES.iter().position(|s| a >= *s).unwrap_or(KILO)
}

#[cfg(test)]
mod tests;
