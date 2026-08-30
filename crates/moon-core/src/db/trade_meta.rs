//! What ONE closed trade was, beyond its prices: the detect line that opened it, the strategy that
//! owns it, and the reason it closed.
//!
//! # Why this is not part of [`super::ChartTradeRecord`]
//!
//! That record is read a THOUSAND rows at a time to draw the chart's trade markers, and every one
//! of its fields is a number. The detect line is prose — routinely two hundred characters of it —
//! and a chart that draws a thousand arrows needs none of it. So it is read separately, for the ONE
//! trade a reader opened, and the bounded history stays as cheap as it was.
//!
//! # Addressing the right row, and only it
//!
//! A record id is not unique across the two report sources: the typed replica counts `newrecid`
//! from one and the legacy table counts its own `db_id` from one, and while a core is mid-migration
//! BOTH hold rows. So the lookup takes the whole [`super::ChartTradeRecord`] rather than its id: the
//! id is matched through the same expression that minted it
//! ([`super::report_read::record_identity_expr`]), and the trade's own coin and timestamps are
//! matched beside it. A row from the other table that happens to share the number fails that test
//! and the search moves on, instead of printing one trade's prose over another's chart.
//!
//! # Where the values come from
//!
//! The core writes its detect line into `comment`, together with a diagnostic tail of its own that
//! nobody reading a chart wants beside the candles — see [`detect_text`]. `strategyid` is the
//! Delphi-signed identity the Report already resolves to a name through the session's strategy
//! store, so this layer carries the ID and lets the UI name it: a terminal reading a replica of a
//! core it is no longer connected to has no name to give, and a number the reader can still match
//! against their own strategy list is a better answer than an empty caption.

use rusqlite::Connection;

use super::read_fail::read_fail;
use super::report_read::{record_identity_expr, report_value_i64, report_value_text};
use super::{ChartTradeRecord, ReadResult, read_sources_res};

#[cfg(test)]
mod tests;

/// Everything a chart can state about ONE closed trade that its prices do not already say.
///
/// Every field is optional at the SOURCE and therefore possibly empty here: a replica whose table
/// predates a column, a core that wrote nothing into it, and a legacy source that never had it all
/// arrive as the same absence. A caption prints nothing rather than a placeholder, which is what
/// makes "this trade carried no detect line" and "this build cannot read one" look the same to the
/// reader — deliberately, because neither is something they can act on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TradeMeta {
    /// The detect line as the core wrote it, with its diagnostic tail removed.
    pub detect: String,
    /// Delphi-signed strategy identity that owns the trade, or `None` when the row carries none.
    ///
    /// Zero is not one: the replica stores it for "no strategy", and a caption naming strategy `0`
    /// would be a number that matches nothing in the reader's list.
    pub strategy_id: Option<i64>,
    /// Why the position closed, as the core stated it: `Auto Price Down`, `StopLoss …`, `Sell Price`.
    pub sell_reason: String,
}

impl TradeMeta {
    /// Whether this carries nothing worth printing.
    ///
    /// The reader's own continue-or-stop test: a source that HOLDS the trade but predates these
    /// columns answers all-empty, and stopping there would leave the source that does carry the
    /// prose unasked. See [`query_trade_meta`].
    ///
    /// Returns:
    ///     `true` when every field is absent.
    pub fn is_empty(&self) -> bool {
        self.detect.is_empty() && self.strategy_id.is_none() && self.sell_reason.is_empty()
    }
}

/// Prefixes of the DIAGNOSTIC lines the core appends under its detect line.
///
/// The core writes its own health beside every trade — CPU load, API budget, latency, ping — and it
/// is a fact about the BOT at that moment, not about the trade. Over candles it is two lines of
/// noise around one line of signal, so it is dropped here, where the rule is stated once, rather
/// than in the caption that draws it.
///
/// Exactly TWO prefixes, and both measured rather than guessed: over 40 000 real comments the core
/// starts a line with `CPU:` or `Latency:` and with nothing else — the API budget and the ping ride
/// INSIDE those two lines. Matching more (`API `, `Ping:`) would buy nothing and would silently
/// swallow a detect line that happens to begin with one of those words.
///
/// Matched after trimming the line's leading spaces, because the core indents them by one.
const DIAGNOSTIC_PREFIXES: [&str; 2] = ["CPU:", "Latency:"];

/// Reduce a raw `comment` to the detect line a chart prints.
///
/// NOT "the first line": most detects are one line, but a handful are two, and taking only the
/// first would silently halve them. Every line that is not diagnostic is kept, joined by a single
/// space so the result is one caption — the drawing pass wraps prose itself and a hard newline
/// would fight its own wrapping.
///
/// A DIFFERENT rule from the terminal's own `detect_line`, which trims a trailing
/// `(strategy <NAME>)` off a LIVE detect line. That tail is trimmed there because the strategy is
/// printed beside it; here it is frequently the whole of what the core wrote — `MoonShot: (strategy
/// <LTC_2l>)` is a complete comment — and trimming it would leave the caption saying `MoonShot:`.
///
/// Args:
///     comment: Raw comment column, as the core wrote it.
///
/// Returns:
///     The detect text, or an empty string when the comment holds nothing but diagnostics.
pub fn detect_text(comment: &str) -> String {
    let mut out = String::new();
    // `lines` handles the core's CRLF as well as a hand-edited LF, and drops the terminators.
    for line in comment.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if DIAGNOSTIC_PREFIXES
            .iter()
            .any(|prefix| line.starts_with(prefix))
        {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
    }
    out
}

/// Bind one value and append the predicate that names it.
///
/// The value is BOUND FIRST and the placeholder numbered from the vector afterwards, so a
/// placeholder cannot drift from the value it names when an absent column skips its check.
///
/// Args:
///     checks: Predicate text being accumulated.
///     params: Bound values, in placeholder order.
///     column: Column being matched, on the alias `r`.
///     value: Value it must equal.
///     collate: Whether to match case-insensitively.
///
/// Returns:
///     Nothing; both arguments are extended in place.
fn push_check(
    checks: &mut String,
    params: &mut Vec<rusqlite::types::Value>,
    column: &str,
    value: rusqlite::types::Value,
    collate: bool,
) {
    params.push(value);
    let at = params.len();
    let suffix = if collate { " COLLATE NOCASE" } else { "" };
    checks.push_str(&format!(" AND r.{column} = ?{at}{suffix}"));
}

/// Read the detect line, strategy and exit reason of one exact trade.
///
/// Takes the RECORD rather than its id: see the module docs for why an id alone cannot address a
/// row while both report sources are live. A source that cannot spell the identity, or that answers
/// with a row whose coin and timestamps do not match this trade, is skipped rather than trusted.
///
/// Every column is optional: a replica predating `comment`, `strategyid` or `sellreason` answers
/// with the fields it does have, and the captions built on the missing ones print nothing. A source
/// that answers with NOTHING AT ALL does not end the search — the other one may still carry the
/// prose.
///
/// Args:
///     conn: Open report reader or pinned snapshot.
///     record: The trade, exactly as the chart history handed it out.
///
/// Returns:
///     The metadata, or `None` when no source holds that row — or holds it with nothing in it.
///
/// Errors:
///     Propagates replica readiness, schema and SQL failures.
pub fn query_trade_meta(
    conn: &Connection,
    record: &ChartTradeRecord,
) -> ReadResult<Option<TradeMeta>> {
    const CONTEXT: &str = "reports: trade metadata";
    if record.record_id == 0 {
        // Zero is the projection for "this source cannot address rows", never a real identity, so
        // a query for it would match whichever rows happen to carry it.
        return Ok(None);
    }
    for source in read_sources_res(conn)? {
        // The columns the trade's own facts are matched on, and the same ones
        // `query_chart_trade_history` requires of a source before it mints an id from it. A source
        // that cannot be asked to CONFIRM the trade is skipped rather than trusted on the number
        // alone — the number is what collides between the two sources in the first place.
        if !["core_uid", "coin", "buydate", "closedate"]
            .iter()
            .all(|column| source.cols.contains(*column))
        {
            continue;
        }
        let column = |name: &str| {
            if source.cols.contains(name) {
                format!("r.{name}")
            } else {
                "NULL".to_string()
            }
        };
        // The trade's own facts, ANDed onto the identity so a colliding number cannot answer.
        // Unconditional: the gate above already refused any source that cannot state all three.
        let mut checks = String::new();
        let mut params: Vec<rusqlite::types::Value> = vec![
            rusqlite::types::Value::Integer(record.core_uid as i64),
            rusqlite::types::Value::Integer(record.record_id),
        ];
        // Case-insensitively for the coin, the way every other coin predicate in this layer
        // matches: the cores disagree on the spelling of their own tokens.
        let coin = rusqlite::types::Value::Text(record.coin.clone());
        push_check(&mut checks, &mut params, "coin", coin, true);
        let buy = rusqlite::types::Value::Integer(record.buy_date);
        push_check(&mut checks, &mut params, "buydate", buy, false);
        let close = rusqlite::types::Value::Integer(record.close_date);
        push_check(&mut checks, &mut params, "closedate", close, false);
        let sql = format!(
            "SELECT {}, {}, {} FROM {} r WHERE r.core_uid = ?1 AND {} = ?2{checks} LIMIT 1",
            column("comment"),
            column("strategyid"),
            column("sellreason"),
            source.table,
            record_identity_expr(&source),
        );
        let answer = conn
            .query_row(&sql, rusqlite::params_from_iter(params), |row| {
                // Decoded through `Value` and the REPORT LAYER's own converters — the same two the
                // record beside this metadata is read with, so a value that reached one reader
                // reaches the other identically. The replica is written by an UNTYPED upsert
                // (`rep::apply_upsert` stores whatever the core sent), so a column can hold a
                // storage class its declared type does not suggest; a typed `get` on one of them
                // fails the WHOLE row, taking the two perfectly readable fields down with it.
                Ok(TradeMeta {
                    detect: detect_text(
                        &report_value_text(&row.get::<_, rusqlite::types::Value>(0)?)
                            .unwrap_or_default(),
                    ),
                    // Filtered rather than carried: the replica stores 0 for "no strategy".
                    strategy_id: report_value_i64(&row.get::<_, rusqlite::types::Value>(1)?)
                        .filter(|id| *id != 0),
                    sell_reason: report_value_text(&row.get::<_, rusqlite::types::Value>(2)?)
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                })
            })
            .map(Some)
            .or_else(|error| match error {
                // A row this source does not hold is not a failure: the trade lives in the other
                // one, which is the ordinary state during a typed catch-up.
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(read_fail(CONTEXT, other)),
            })?;
        // An empty answer does not end the search: this source holds the trade but predates the
        // columns, and the other one may still carry the prose. The caller cannot tell an empty
        // answer from no answer — it reads both as "nothing to print" — so nothing is remembered.
        if let Some(meta) = answer {
            if !meta.is_empty() {
                return Ok(Some(meta));
            }
        }
    }
    Ok(None)
}
