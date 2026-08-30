//! Unit checks for current-rate SQL coverage, freshness, provenance, and mode persistence codes.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use rusqlite::Connection;

use super::{CurrentRate, CurrentRates, FRESHNESS_MS, ValuationMode, current_rate_sql};

/// Wall clock the fixtures treat as "now".
const NOW_MS: i64 = 1_800_000_000_000;

/// Build the column set of a typed report source.
///
/// Returns:
///     Every source column used by the current-rate SQL fixtures.
fn typed_columns() -> HashSet<String> {
    [
        "core_uid",
        "closedate",
        "basecurrency",
        "profitbtc",
        "spentbtc",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Build a rate snapshot from `(ordinal, rate, age_ms)` triples plus unroutable ordinals.
///
/// Args:
///     entries: Quote ordinal, USDT rate, and age for each available rate.
///     missing: Quote ordinals whose routes are permanently absent.
///
/// Returns:
///     A snapshot judged against [`NOW_MS`].
fn snapshot(entries: &[(i64, f64, i64)], missing: &[i64]) -> CurrentRates {
    let rates = entries
        .iter()
        .map(|&(ordinal, rate_usdt, age_ms)| {
            (
                ordinal,
                CurrentRate {
                    rate_usdt,
                    provider: "binance_spot".to_string(),
                    symbol: format!("Q{ordinal}USDT"),
                    fetched_at_ms: NOW_MS - age_ms,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    CurrentRates::new(rates, missing.iter().copied().collect::<BTreeSet<_>>())
}

/// Seed one in-memory report table with `(basecurrency, profitbtc, spentbtc)` rows.
///
/// Args:
///     rows: Quote ordinal, profit, and spend for each report row.
///
/// Returns:
///     An open in-memory connection containing the seeded typed source.
fn seed(rows: &[(i64, f64, f64)]) -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, closedate INTEGER,
                                  basecurrency INTEGER, profitbtc REAL, spentbtc REAL)",
    )
    .expect("schema");
    for (quote, profit, spent) in rows {
        conn.execute(
            "INSERT INTO orders_rep VALUES (1, 1000, ?1, ?2, ?3)",
            rusqlite::params![quote, profit, spent],
        )
        .expect("row");
    }
    conn
}

/// Run the coverage aggregate and return `(eligible, valued, unavailable, profit, spent)`.
///
/// Args:
///     conn: Seeded report connection.
///     parts: Current-rate SQL fragments to aggregate.
///
/// Returns:
///     The five coverage values asserted by these tests; the internal spent-row count is omitted.
fn aggregate(conn: &Connection, parts: &super::CoverageSql) -> (i64, i64, i64, f64, f64) {
    let sql = format!("SELECT {} FROM orders_rep r", parts.aggregate_columns());
    conn.query_row(&sql, [], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })
    .expect("aggregate")
}

/// Identity USDT and a routed quote must aggregate under the rates supplied independently here.
///
/// Breakage: `current.rs::current_rate_sql` omitting the identity arm or applying the wrong rate to
/// profit or spend would make the hand-calculated totals diverge.
#[test]
fn a_scope_is_valued_at_the_published_rates_and_identity_usdt_needs_none() {
    // Oracle is hand arithmetic done here, not by the code under test:
    // 10 USDT stays 10; 3 units of ordinal 2 at 2000 USDT each is 6000; the total is 6010.
    let conn = seed(&[(1, 10.0, 100.0), (2, 3.0, 4.0)]);
    let parts = current_rate_sql(
        "r",
        &typed_columns(),
        &snapshot(&[(2, 2000.0, 0)], &[]),
        NOW_MS,
    );
    let (eligible, valued, unavailable, profit, spent) = aggregate(&conn, &parts);
    assert_eq!((eligible, valued, unavailable), (2, 2, 0));
    assert!((profit - 6010.0).abs() < 1e-9, "profit was {profit}");
    // 100 USDT of spend stays 100; 4 units at 2000 is 8000.
    assert!((spent - 8100.0).abs() < 1e-9, "spend was {spent}");
}

/// A quote absent from both rate maps is pending, not permanently unavailable.
///
/// Breakage: `current.rs::current_rate_sql` treating every missing rate as unavailable would show a
/// permanent-gap warning before the worker has attempted the quote.
#[test]
fn a_quote_with_no_rate_yet_is_pending_rather_than_unavailable() {
    // Breakage: `current.rs::current_rate_sql` folding the not-fetched-yet case into
    // `unavailable` — the footer would flash a permanent-gap count while the first refresh pass
    // is still running, before any rate has been fetched at all.
    let conn = seed(&[(8, 5.0, 6.0)]);
    let parts = current_rate_sql("r", &typed_columns(), &snapshot(&[], &[]), NOW_MS);
    let (eligible, valued, unavailable, profit, _) = aggregate(&conn, &parts);
    assert_eq!((eligible, valued, unavailable), (1, 0, 0));
    assert_eq!(profit, 0.0);
}

/// A quote whose routes are permanently absent must count as unavailable.
///
/// Breakage: `current.rs::current_rate_sql` ignoring `CurrentRates::missing` would leave a permanent
/// gap looking like refresh progress forever.
#[test]
fn an_unroutable_quote_is_counted_unavailable_not_pending() {
    let conn = seed(&[(13, 5.0, 6.0)]);
    let parts = current_rate_sql("r", &typed_columns(), &snapshot(&[], &[13]), NOW_MS);
    let (eligible, valued, unavailable, _, _) = aggregate(&conn, &parts);
    assert_eq!((eligible, valued, unavailable), (1, 0, 1));
}

/// A rate is usable only before, never at, the freshness boundary.
///
/// Breakage: changing `CurrentRate::is_fresh` from `< FRESHNESS_MS` to `<= FRESHNESS_MS` would keep a
/// rate current for one interval beyond the documented cutoff.
#[test]
fn a_rate_past_the_freshness_cutoff_stops_being_current() {
    // Breakage: `current.rs::CurrentRate::is_fresh` dropping the `fetched_at_ms` comparison —
    // a rate the provider stopped refreshing hours ago would keep rendering as "current",
    // which is a wrong number wearing a right number's label.
    let conn = seed(&[(2, 3.0, 4.0)]);
    let fresh = current_rate_sql(
        "r",
        &typed_columns(),
        &snapshot(&[(2, 2000.0, FRESHNESS_MS - 1)], &[]),
        NOW_MS,
    );
    assert_eq!(
        aggregate(&conn, &fresh).1,
        1,
        "one millisecond inside the window"
    );
    let stale = current_rate_sql(
        "r",
        &typed_columns(),
        &snapshot(&[(2, 2000.0, FRESHNESS_MS)], &[]),
        NOW_MS,
    );
    assert_eq!(aggregate(&conn, &stale).1, 0, "exactly at the window edge");
}

/// A source without `spentbtc` must still prepare and value profit.
///
/// Breakage: `current.rs::current_rate_sql` naming the absent column would fail SQLite preparation
/// and blank the entire legacy read rather than only its spend value.
#[test]
fn a_legacy_source_without_a_spend_column_still_prepares() {
    // Breakage: `current.rs::current_rate_sql` naming `spentbtc` unconditionally — SQLite
    // resolves column references at prepare time, so the whole legacy read would fail rather
    // than yield a NULL spend, blanking the report instead of one column.
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, closedate INTEGER,
                                  basecurrency INTEGER, profitbtc REAL);
         INSERT INTO orders_rep VALUES (1, 1000, 2, 3.0)",
    )
    .expect("schema");
    let columns: HashSet<String> = ["core_uid", "closedate", "basecurrency", "profitbtc"]
        .into_iter()
        .map(str::to_string)
        .collect();
    let parts = current_rate_sql("r", &columns, &snapshot(&[(2, 2000.0, 0)], &[]), NOW_MS);
    let (_, valued, _, profit, spent) = aggregate(&conn, &parts);
    assert_eq!(valued, 1);
    assert!((profit - 6000.0).abs() < 1e-9, "profit was {profit}");
    assert_eq!(spent, 0.0, "an absent spend column sums to nothing");
}

/// An embedded floating-point rate must survive SQLite parsing exactly.
///
/// Breakage: `current.rs::rate_literal` using fixed precision would round the provider rate before
/// multiplication and drift large totals.
#[test]
fn a_rate_survives_the_round_trip_through_the_sql_literal() {
    // Breakage: `current.rs::rate_literal` switching to a fixed-precision format — every BTC
    // rate would silently round and a large total would drift by thousands.
    let conn = seed(&[(0, 1.0, 0.0)]);
    let rate = 118_437.123_456_789_1_f64;
    let parts = current_rate_sql(
        "r",
        &typed_columns(),
        &snapshot(&[(0, rate, 0)], &[]),
        NOW_MS,
    );
    let (_, _, _, profit, _) = aggregate(&conn, &parts);
    assert_eq!(profit, rate, "the literal must round-trip bit for bit");
}

/// Persisted historical and current codes must decode to their original modes.
///
/// Breakage: changing only `ValuationMode::code` or `from_code` would make a saved layout restore a
/// different mode or silently fall back to historical valuation.
#[test]
fn a_mode_code_round_trips() {
    for mode in [ValuationMode::Historical, ValuationMode::Current] {
        assert_eq!(ValuationMode::from_code(mode.code()), Some(mode));
    }
    assert_eq!(ValuationMode::from_code("usdt"), None);
}
