//! Tuner fields the core writes into the report `comment` rather than into a column of their own.
//!
//! A DropsDetection deal's comment carries the detect's own numbers:
//! ` DropsDetection: <name> USDT-COIN  DailyVol: 27m PriceIsLow: false xPriceDelta: 3.0 \r\n...`.
//! `xPriceDelta` is the drop measured at fire time, in per cent (one decimal, the core's
//! rounding). It is parsed at READ time by a plain SQL expression, so nothing is stored and no
//! scalar function has to be installed on every connection that reads the unified source.
//!
//! `DailyVol` is the `dvol` column rounded to millions, and `PriceIsLow` is a boolean the
//! numeric min/max machinery cannot save back, so neither is a field here.

use std::collections::HashSet;

/// Tuner column holding the `xPriceDelta` of a DropsDetection comment; NULL for any other row.
pub(super) const DROPS_PRICE_DELTA: &str = "xpricedelta";

/// SQL projecting a comment-derived tuner field, or `None` when `col` is a real report column.
///
/// Args:
///     col: Tuner field name from `FIELDS`.
///     alias: Alias of the report table in the enclosing statement.
///     cols: Columns the report source actually has.
///
/// Returns:
///     The expression (without `AS`) for a comment-derived field: the parsed value, or NULL when
///     the source has no `comment`. `None` for every other field.
pub(in crate::db) fn derived_sql(col: &str, alias: &str, cols: &HashSet<String>) -> Option<String> {
    if col != DROPS_PRICE_DELTA {
        return None;
    }
    if !cols.contains("comment") {
        return Some("NULL".to_string());
    }
    Some(drops_price_delta_sql(&format!("{alias}.\"comment\"")))
}

/// One tuner field as read straight off `orders_rep` (no alias), for tests that load a raw
/// report snapshot without the unified source.
#[cfg(test)]
pub(in crate::db) fn raw_column_sql(col: &str) -> String {
    let cols: HashSet<String> = ["comment".to_string()].into();
    derived_sql(col, "orders_rep", &cols).unwrap_or_else(|| format!("\"{col}\""))
}

/// SQL reading `xPriceDelta` out of a DropsDetection comment held in `comment`.
///
/// The prefix test runs first and is cheap, so the other report rows of the unified source pay
/// one short `instr`. The token ends at the first space or line break; anything but digits with
/// at most one dot is malformed and yields NULL rather than a partial number.
fn drops_price_delta_sql(comment: &str) -> String {
    const TOKEN: &str = " xPriceDelta: ";
    let start = format!("instr({comment}, '{TOKEN}')");
    // The text after the token with line breaks turned into spaces, so one `instr` ends it.
    let rest = format!(
        "replace(replace(substr({comment}, {start} + {len}), char(13), ' '), char(10), ' ')",
        len = TOKEN.len()
    );
    let token = format!("substr({rest}, 1, instr({rest} || ' ', ' ') - 1)");
    format!(
        "CASE WHEN instr(substr({comment}, 1, 24), 'DropsDetection:') > 0 AND {start} > 0 \
         THEN (SELECT CASE WHEN t GLOB '*[0-9]*' AND t NOT GLOB '*[^0-9.]*' \
         AND t NOT GLOB '*.*.*' THEN CAST(t AS REAL) END FROM (SELECT {token} AS t)) END"
    )
}

#[cfg(test)]
mod tests;
