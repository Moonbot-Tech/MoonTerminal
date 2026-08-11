//! Turning one row's numbers into the exact text a cell draws.
//!
//! Split out of `table.rs`, which draws them: these are pure functions of a value and its unit, and
//! the tests exercise them without a window.

use moon_core::db::ProfitUnit;
use moon_core::util::fmt::DeltaSign;

use super::{PROFIT_COLUMN_WIDTH, PROFIT_LAST_TRADE_EXTRA};

/// Return the profit column's design-reference width.
///
/// Args:
///     show_last: Whether the cell carries its `total(last)` suffix.
///
/// Returns:
///     Base width, plus the suffix allowance when the suffix is drawn.
pub(super) fn profit_column_width(show_last: bool) -> f32 {
    if show_last {
        PROFIT_COLUMN_WIDTH + PROFIT_LAST_TRADE_EXTRA
    } else {
        PROFIT_COLUMN_WIDTH
    }
}

/// Format profit with its exact comparable unit, optionally carrying the newest closed trade.
///
/// The suffix goes INSIDE the unit — `-57.11(-0.60) USDT`, not `-57.11 USDT (-0.60)` — so the two
/// amounts read as one measurement in one currency, which is what they are. Both are rounded to
/// the same unit decimals, so the bracket can never claim precision the total does not have.
///
/// The returned sign describes the TOTAL. The suffix is a different trade and may disagree; the
/// cell is coloured by the number it is about.
///
/// Args:
///     value: Projected profit.
///     last: Profit of the newest closed trade, when the suffix is enabled and one exists.
///     unit: Exact quote or percent unit.
///
/// Returns:
///     Signed compact text carrying its unit and the sign represented after display rounding.
pub(super) fn format_profit(
    value: f64,
    last: Option<f64>,
    unit: Option<ProfitUnit>,
) -> (String, DeltaSign) {
    let decimals = match unit {
        Some(ProfitUnit::Quote(currency)) => currency.display_decimals(),
        Some(ProfitUnit::Percent) | None => 2,
    };
    let (amount, sign) = moon_core::util::fmt::signed_amount(value, decimals);
    let amount = match last {
        Some(last) => {
            let (last, _) = moon_core::util::fmt::signed_amount(last, decimals);
            format!("{amount}({last})")
        }
        None => amount,
    };
    let text = match unit {
        Some(ProfitUnit::Quote(currency)) => format!("{amount} {}", currency.ticker()),
        Some(ProfitUnit::Percent) => format!("{amount}%"),
        None => amount,
    };
    (text, sign)
}

/// Format a monitor trade count with the terminal's shared thousands grouping.
///
/// Args:
///     value: Closed-trade count.
///
/// Returns:
///     ASCII digits separated into space-grouped thousands.
pub(super) fn format_trade_count(value: i64) -> String {
    moon_core::util::fmt::group_thousands(&value.to_string())
}

/// Format win rate with the terminal's shared half-away-from-zero percentage rounding.
///
/// Args:
///     value: Win percentage in `0..=100`.
///
/// Returns:
///     Percentage with one decimal place.
pub(super) fn format_win_rate(value: f64) -> String {
    moon_core::util::fmt::pct(value, 1)
        .map(|(text, _)| text)
        .unwrap_or_else(|| "0.0%".to_string())
}

/// Format average order spend in the query's comparable quote unit.
///
/// Args:
///     value: Average positive spend.
///     unit: Exact query unit.
///
/// Returns:
///     Compact unsigned order size with a quote ticker when known.
pub(super) fn format_amount(value: f64, unit: Option<ProfitUnit>) -> String {
    match unit {
        Some(ProfitUnit::Quote(currency)) => format!(
            "{} {}",
            moon_core::util::fmt::compact(value, currency.display_decimals()),
            currency.ticker()
        ),
        Some(ProfitUnit::Percent) | None => moon_core::util::fmt::compact(value, 2),
    }
}
