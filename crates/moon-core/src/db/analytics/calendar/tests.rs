//! Calendar aggregation and quote-scope regression tests.

use super::*;
use rusqlite::Connection;

/// In-memory `orders_rep` with closedate, core_uid, and profitbtc; `unified_from` builds
/// its branch from the columns that EXIST.
///
/// Args:
///     rows: Close time, core id, and raw profit fixtures.
///
/// Returns:
///     Seeded USDT report connection.
fn seed(rows: &[(i64, i64, f64)]) -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE orders_rep(
            closedate INTEGER, core_uid INTEGER, profitbtc REAL, basecurrency INTEGER
         );",
    )
    .unwrap();
    for (d, uid, p) in rows {
        c.execute(
            "INSERT INTO orders_rep(closedate, core_uid, profitbtc, basecurrency)
             VALUES (?1, ?2, ?3, 1)",
            rusqlite::params![d, uid, p],
        )
        .unwrap();
    }
    c
}

/// `orders_rep` carrying `spentbtc` too, so the Percent metric can form `profit / spent`.
///
/// Args:
///     rows: Close time, core id, raw profit, and spend fixtures.
///
/// Returns:
///     Seeded USDT report connection.
fn seed_spent(rows: &[(i64, i64, f64, f64)]) -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE orders_rep(
            closedate INTEGER, core_uid INTEGER, profitbtc REAL, spentbtc REAL,
            basecurrency INTEGER
         );",
    )
    .unwrap();
    for (d, uid, p, s) in rows {
        c.execute(
            "INSERT INTO orders_rep(
                closedate, core_uid, profitbtc, spentbtc, basecurrency
             ) VALUES (?1, ?2, ?3, ?4, 1)",
            rusqlite::params![d, uid, p, s],
        )
        .unwrap();
    }
    c
}

// 2021-01-01 00:00:00 UTC.
const D0: i64 = 1_609_459_200;

/// Removing dense UTC-day filling from `calendar_cells_from` would lose the empty middle day or
/// corrupt the independently asserted trade/win counts around it.
#[test]
fn buckets_by_utc_day_fills_gaps_and_counts_wins() {
    // Day 0: two profitable trades (+10, +5); day 1: empty; day 2: one loss (-3).
    let c = seed(&[
        (D0 + 3_600, 1, 10.0),
        (D0 + 7_200, 2, 5.0),
        (D0 + 2 * 86_400 + 100, 1, -3.0),
    ]);
    let q = Query {
        from: D0,
        to: D0 + 3 * 86_400,
        ..Default::default()
    };
    let days = calendar_cells_from(&c, &q, ProjectionMode::Native).unwrap();
    // Dense range: exactly three days, including empty day 1.
    assert_eq!(
        days.iter().map(|d| d.start).collect::<Vec<_>>(),
        vec![D0, D0 + 86_400, D0 + 2 * 86_400]
    );
    assert_eq!(
        (
            days[0].totals.trades,
            days[0].totals.wins,
            days[0].totals.profit
        ),
        (2, 2, 15.0)
    );
    assert_eq!((days[1].totals.trades, days[1].totals.profit), (0, 0.0)); // The gap is filled.
    assert_eq!(
        (
            days[2].totals.trades,
            days[2].totals.wins,
            days[2].totals.profit
        ),
        (1, 0, -3.0)
    );
}

/// Replacing `mt_local_bucket` with integer UTC-day division would place this trade on March 28,
/// although the selected Warsaw calendar already shows March 29.
#[test]
fn calendar_bucket_uses_the_selected_civil_day() {
    let close = 1_774_740_600; // 2026-03-28 23:30 UTC = 2026-03-29 00:30 Warsaw.
    let c = seed(&[(close, 1, 7.0)]);
    let q = Query {
        axis: crate::db::ReportAxis::from_measured(Default::default(), chrono_tz::Europe::Warsaw),
        from: 1_774_735_200,
        to: 1_774_828_800,
        ..Default::default()
    };

    let days = calendar_cells_from(&c, &q, ProjectionMode::Native).expect("calendar reads");

    let populated: Vec<_> = days.iter().filter(|day| day.totals.trades != 0).collect();
    assert_eq!(populated.len(), 1);
    assert_eq!(populated[0].start, 1_774_738_800); // Warsaw midnight, 23:00 UTC.
    assert_eq!(populated[0].totals.profit, 7.0);
}

/// `calendar/mod.rs:calendar_cells_from` must return `Ok(empty)` for an initialized source with no
/// trades; treating it as a read failure leaves a new Calendar tab in a retry loop.
#[test]
fn empty_period_is_successful_empty() {
    let c = seed(&[]);
    let q = Query {
        from: D0,
        to: D0 + 86_400,
        ..Default::default()
    };
    // A schema with no trades yields a successful empty calendar, not an infinite fill.
    assert_eq!(
        calendar_cells_from(&c, &q, ProjectionMode::Native)
            .unwrap()
            .len(),
        0
    );
}

/// Changing the period predicate from half-open to inclusive would admit the independently seeded
/// trade exactly beyond `to` and make the three-day result contain its 99-unit profit.
#[test]
fn respects_period_bounds_excluding_to() {
    // The day-3 trade is outside [from, to), while the trailing empty day 2 is present.
    let c = seed(&[(D0 + 100, 1, 7.0), (D0 + 3 * 86_400 + 100, 1, 99.0)]);
    let q = Query {
        from: D0,
        to: D0 + 3 * 86_400,
        ..Default::default()
    };
    let days = calendar_cells_from(&c, &q, ProjectionMode::Native).unwrap();
    assert_eq!(days.len(), 3);
    assert_eq!(days[0].totals.trades, 1);
    assert!(days.iter().all(|d| (d.totals.profit - 99.0).abs() > 1e-9)); // Day 3 is excluded.
}

/// `calendar/mod.rs:calendar_period_from` must query the supplied comparison period on the same
/// connection; accidentally reusing the current query makes the Month KPI compare a month with
/// itself and report a false zero delta.
#[test]
fn calendar_period_keeps_current_and_comparison_scopes_distinct() {
    let c = seed(&[
        (D0 - 86_400 + 100, 1, -5.0),
        (D0 + 100, 1, 7.0),
        (D0 + 200, 1, 2.0),
    ]);
    let current = Query {
        from: D0,
        to: D0 + 86_400,
        ..Default::default()
    };
    let previous = Query {
        from: D0 - 86_400,
        to: D0,
        ..Default::default()
    };

    let period_scope = calendar_period_from(&c, &current, Some(&previous), false).unwrap();
    let period = period_scope
        .data()
        .expect("single-quote Calendar periods are comparable");

    assert_eq!(period.current.len(), 1);
    assert_eq!(
        (
            period.current[0].totals.profit,
            period.current[0].totals.trades,
            period.current[0].totals.wins,
        ),
        (9.0, 2, 2)
    );
    let previous = period.previous.expect("comparison period was requested");
    assert_eq!(
        (previous.profit, previous.trades, previous.wins),
        (-5.0, 1, 0)
    );
}

/// Adding daily `GROUP BY` back to `calendar/mod.rs:calendar_total_from` makes `query_row` return
/// only the first seeded day and turns the visible previous-period KPI from `(6, 3, 2)` into
/// `(4, 1, 1)`.
#[test]
fn previous_period_total_aggregates_multiple_days_directly() {
    let conn = seed(&[
        (D0 - 3 * 86_400 + 100, 1, 4.0),
        (D0 - 2 * 86_400 + 100, 1, -3.0),
        (D0 - 86_400 + 100, 1, 5.0),
    ]);
    let previous = Query {
        from: D0 - 3 * 86_400,
        to: D0,
        ..Default::default()
    };

    let total = calendar_total_from(&conn, &previous, ProjectionMode::Native)
        .expect("direct previous-period total");
    assert_eq!((total.profit, total.trades, total.wins), (6.0, 3, 2));
}

/// Calendar raw money uses the same split boundary as Summary, without blocking Percent mode.
///
/// Removing the preflight in `calendar_period_from` exposes a scalar 15.0 across USDT and USDC;
/// applying the raw-money rule to Percent would incorrectly hide valid dimensionless returns.
#[test]
fn mixed_quote_calendar_splits_raw_money_but_keeps_percent() {
    let c = seed_spent(&[(D0 + 100, 1, 10.0, 100.0), (D0 + 200, 1, 5.0, 100.0)]);
    c.execute(
        "UPDATE orders_rep SET basecurrency = 8 WHERE closedate = ?1",
        [D0 + 200],
    )
    .expect("mark second row as USDC");
    let raw_query = Query {
        from: D0,
        to: D0 + 86_400,
        ..Default::default()
    };

    let raw = calendar_period_from(&c, &raw_query, None, false).expect("raw Calendar");
    let split = raw.split().expect("mixed Calendar money must be split");
    assert_eq!((split.orders, split.totals.len()), (2, 2));
    assert!(raw.data().is_none());

    let percent = calendar_period_from(
        &c,
        &Query {
            metric: crate::db::ProfitMetric::Percent,
            ..raw_query
        },
        None,
        false,
    )
    .expect("percent Calendar");
    assert!(matches!(
        percent,
        ProfitScope::Comparable {
            unit: ProfitUnit::Percent,
            ..
        }
    ));
}

/// Calendar comparison data is omitted when the previous period has another quote.
///
/// Reusing only the current period's quote decision in `calendar_period_from` makes this expose
/// the previous USDT aggregate beside the current USDC aggregate as though their delta were valid.
#[test]
fn calendar_omits_previous_when_quote_changes() {
    let c = seed(&[(D0 - 100, 1, 4.0), (D0 + 100, 1, 7.0)]);
    c.execute(
        "UPDATE orders_rep SET basecurrency = 8 WHERE closedate >= ?1",
        [D0],
    )
    .expect("mark current row as USDC");
    let current = Query {
        from: D0,
        to: D0 + 86_400,
        ..Default::default()
    };
    let previous = Query {
        from: D0 - 86_400,
        to: D0,
        ..Default::default()
    };

    let scoped = calendar_period_from(&c, &current, Some(&previous), false).expect("Calendar");
    let period = scoped.data().expect("current USDC Calendar is comparable");
    assert!(
        period.previous.is_none(),
        "USDT previous must not compare with USDC"
    );
}

/// Replacing Percent projection with native profit in `calendar_cells_from` would yield 7 instead
/// of the independently calculated sum of +5% and -5% while changing neither win count nor rows.
#[test]
fn percent_metric_is_profit_over_spent() {
    // Same day, two trades: +10 on 200 spent = +5%, -3 on 60 spent = -5%.
    let c = seed_spent(&[(D0 + 3_600, 1, 10.0, 200.0), (D0 + 7_200, 1, -3.0, 60.0)]);
    let base = Query {
        from: D0,
        to: D0 + 86_400,
        ..Default::default()
    };
    // USDT (default): raw money, 10 - 3 = 7.
    let usd = calendar_cells_from(&c, &base, ProjectionMode::Native).unwrap();
    assert!(
        (usd[0].totals.profit - 7.0).abs() < 1e-9,
        "usd={}",
        usd[0].totals.profit
    );
    // Percent: each trade as profit/spent*100, summed: +5 + (-5) = 0.
    let pct = calendar_cells_from(
        &c,
        &Query {
            metric: crate::db::ProfitMetric::Percent,
            ..base.clone()
        },
        ProjectionMode::Percent,
    )
    .unwrap();
    assert!(
        (pct[0].totals.profit - 0.0).abs() < 1e-9,
        "pct={}",
        pct[0].totals.profit
    );
    // Sign is preserved, so win/loss classification is unchanged by the metric.
    assert_eq!((pct[0].totals.trades, pct[0].totals.wins), (2, 1));
}

/// Percent aggregation excludes a zero-spent row instead of inventing a zero-percent trade.
///
/// Removing the positive-spent predicate from the Percent source makes the independent trade
/// count include the second fixture row and skews Calendar win rate.
#[test]
fn percent_metric_excludes_zero_spent() {
    // A trade with no spent has no percent, so in percent mode it is EXCLUDED entirely — never
    // a divide-by-zero, and never a phantom zero-profit trade that would skew count/winrate.
    let c = seed_spent(&[(D0 + 3_600, 1, 10.0, 200.0), (D0 + 7_200, 1, 4.0, 0.0)]);
    let pct = calendar_cells_from(
        &c,
        &Query {
            from: D0,
            to: D0 + 86_400,
            metric: crate::db::ProfitMetric::Percent,
            ..Default::default()
        },
        ProjectionMode::Percent,
    )
    .unwrap();
    // Only the +5% trade survives: one trade, one win, +5% — the zero-spent row is gone from
    // COUNT and SUM alike, so the two agree.
    assert!(
        (pct[0].totals.profit - 5.0).abs() < 1e-9,
        "pct={}",
        pct[0].totals.profit
    );
    assert_eq!((pct[0].totals.trades, pct[0].totals.wins), (1, 1));

    // In raw quote mode the same zero-spent row is still counted (no spent filter there).
    let usd = calendar_cells_from(
        &c,
        &Query {
            from: D0,
            to: D0 + 86_400,
            ..Default::default()
        },
        ProjectionMode::Native,
    )
    .unwrap();
    assert_eq!(usd[0].totals.trades, 2);
}

/// Changing `hour_profile_one` to group by complete calendar timestamp instead of UTC hour would
/// split the independently seeded hour-one trades across days and lose the expected aggregate.
#[test]
fn hour_profile_buckets_by_hour_of_day_across_days() {
    // Hour 1: +10 and -4 on day 0, plus +3 on day 1, aggregated by hour of day.
    // Hour 22: +7 on day 0. All other hours are empty.
    let c = seed(&[
        (D0 + 3_600 + 60, 1, 10.0),
        (D0 + 3_600 + 120, 1, -4.0),
        (D0 + 22 * 3_600, 1, 7.0),
        (D0 + 86_400 + 3_600, 1, 3.0),
    ]);
    let prof = hour_profile_one(&c, &Query::default(), D0, D0 + 3 * 86_400).unwrap();
    // Hour 1 combines both days: profit 10 - 4 + 3 = 9, three trades, two wins.
    assert_eq!((prof[1].trades, prof[1].wins), (3, 2));
    assert!(
        (prof[1].profit - 9.0).abs() < 1e-9,
        "profit={}",
        prof[1].profit
    );
    // Hour 22: one +7 trade.
    assert_eq!((prof[22].trades, prof[22].wins), (1, 1));
    assert!((prof[22].profit - 7.0).abs() < 1e-9);
    // An hour without trades is all zeroes in the dense 24-element array.
    assert_eq!((prof[0].trades, prof[0].profit), (0, 0.0));
    // A trade outside [from, to) does not enter the profile, so the fresh period is empty.
    let empty = hour_profile_one(&c, &Query::default(), D0 - 5 * 86_400, D0 - 4 * 86_400).unwrap();
    assert!(empty.iter().all(|h| h.trades == 0));
}

// ── Turnover, execution cost, and what counts as a trade ────────────────────

/// One report row, with defaults that form a clean 1000-unit notional trade.
///
/// Named fields because the aggregate reads eight of them and a positional tuple of that width
/// makes a swapped price silently plausible.
#[derive(Clone)]
struct Exec {
    close: i64,
    core: i64,
    profit: f64,
    spent: f64,
    qty: f64,
    buy: f64,
    sell: f64,
    lev: i64,
    short: bool,
    reason: &'static str,
    quote: i64,
    /// Contract part of the market name, which the COIN-M rule shape-matches.
    coin: &'static str,
    /// Full market spelling: the one per-row fact separating COIN-M from USD-M.
    fname: &'static str,
}

impl Default for Exec {
    fn default() -> Self {
        Self {
            close: D0 + 100,
            core: 1,
            profit: 0.0,
            spent: 1000.0,
            qty: 100.0,
            buy: 10.0,
            sell: 10.0,
            lev: 1,
            short: false,
            reason: "Sell Price",
            quote: 1,
            coin: "BTC",
            fname: "Pump_USDT-BTC_x",
        }
    }
}

/// A trade whose venue charged `rate` per side, priced so the fee is exactly recoverable.
///
/// Args:
///     qty: Base quantity bought.
///     buy: Entry price.
///     sell: Exit price.
///     rate: One-sided commission as a fraction.
///
/// Returns:
///     Row whose money columns are net of that commission, as a core writes them.
fn charged(qty: f64, buy: f64, sell: f64, rate: f64) -> Exec {
    let spent = qty * buy * (1.0 + rate);
    let gained = qty * sell * (1.0 - rate);
    Exec {
        qty,
        buy,
        sell,
        spent,
        profit: gained - spent,
        ..Default::default()
    }
}

/// In-memory replica carrying every execution column the aggregate reads.
///
/// Args:
///     rows: Execution fixtures.
///
/// Returns:
///     Seeded report connection.
fn seed_exec(rows: &[Exec]) -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE orders_rep(
            newrecid INTEGER PRIMARY KEY AUTOINCREMENT,
            closedate INTEGER, buydate INTEGER, core_uid INTEGER, profitbtc REAL, spentbtc REAL,
            boughtq REAL, buyprice REAL, sellprice REAL, lev INTEGER, isshort INTEGER,
            sellreason TEXT, basecurrency INTEGER, deleted INTEGER,
            coin TEXT, fname TEXT
         );",
    )
    .unwrap();
    for r in rows {
        c.execute(
            "INSERT INTO orders_rep(
                closedate, buydate, core_uid, profitbtc, spentbtc, boughtq, buyprice, sellprice,
                lev, isshort, sellreason, basecurrency, deleted, coin, fname
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, ?13, ?14)",
            rusqlite::params![
                r.close,
                r.close - 60,
                r.core,
                r.profit,
                r.spent,
                r.qty,
                r.buy,
                r.sell,
                r.lev,
                i64::from(r.short),
                r.reason,
                r.quote,
                r.coin,
                r.fname,
            ],
        )
        .unwrap();
    }
    c
}

/// Read the single seeded day under one projection.
fn one_day(c: &Connection, projection: ProjectionMode) -> CellTotals {
    let q = Query {
        from: D0,
        to: D0 + 86_400,
        ..Default::default()
    };
    let days = calendar_cells_from(c, &q, projection).expect("calendar reads");
    assert_eq!(days.len(), 1, "fixture seeds exactly one day");
    days[0].totals
}

/// Funding is money without a trade. Counting it would inflate the trade count, dilute win rate,
/// and add the funded position to turnover — while dropping its profit would stop the day's PnL
/// matching the Report over the same period.
#[test]
fn funding_keeps_its_money_and_stays_out_of_every_trade_figure() {
    let c = seed_exec(&[
        charged(100.0, 10.0, 11.0, 0.0004),
        // Funding, exactly as a core books it: no spread, spend equal to the position.
        Exec {
            profit: 2.5,
            spent: 350.0,
            qty: 350.0,
            buy: 0.5,
            sell: 0.5,
            reason: "Funding",
            ..Default::default()
        },
    ]);
    let totals = one_day(&c, ProjectionMode::Native);

    assert_eq!(
        (totals.trades, totals.wins),
        (1, 1),
        "funding is not a trade"
    );
    // Profit keeps both: 100 x (11 - 10) less the round-trip fee, plus the funding credit.
    let traded = charged(100.0, 10.0, 11.0, 0.0004).profit;
    assert!(
        (totals.profit - (traded + 2.5)).abs() < 1e-9,
        "profit={} expected={}",
        totals.profit,
        traded + 2.5
    );
    // Turnover and fee see the trade alone: 1000 notional plus its fee, not 350 more.
    let volume = totals.volume.expect("native money is summable");
    assert!((volume - 1000.4).abs() < 1e-9, "volume={volume}");
    assert_eq!(totals.fee_trades, 1);
}

/// The recovered fee must equal the round trip the venue actually charged, on both sides of the
/// book. A sign flip on shorts would report a negative cost and inflate the day's turnover story.
#[test]
fn fee_recovers_the_round_trip_on_both_sides() {
    // Long: 0.04% per side on a 1000 entry that exits at 1100.
    let long = one_day(
        &seed_exec(&[charged(100.0, 10.0, 11.0, 0.0004)]),
        ProjectionMode::Native,
    );
    let expected_long = 100.0 * 10.0 * 0.0004 + 100.0 * 11.0 * 0.0004;
    let fee_long = long.fee.expect("native money is summable");
    assert!(
        (fee_long - expected_long).abs() < 1e-9,
        "fee={fee_long} expected={expected_long}"
    );

    // Short: entry sells at 11, exit buys at 10, so the same rate applies to the mirrored legs.
    let rate = 0.0004;
    let (qty, entry, exit) = (100.0, 11.0, 10.0);
    let short = Exec {
        short: true,
        qty,
        buy: entry,
        sell: exit,
        spent: qty * entry * (1.0 - rate),
        profit: qty * entry * (1.0 - rate) - qty * exit * (1.0 + rate),
        ..Default::default()
    };
    let short_totals = one_day(&seed_exec(&[short]), ProjectionMode::Native);
    let expected_short = qty * entry * rate + qty * exit * rate;
    let fee_short = short_totals.fee.expect("native money is summable");
    assert!(
        (fee_short - expected_short).abs() < 1e-9,
        "fee={fee_short} expected={expected_short}"
    );
    assert!(fee_short > 0.0, "a cost is positive on a short too");
}

/// A margin core books `notional / leverage`, and leverage differs per trade. Summing the column
/// as-is understated one live month's turnover by 26%.
#[test]
fn turnover_rebuilds_the_notional_of_a_margin_core() {
    // Core 1 posts margin at two different leverages; both rebuild a 1000 notional.
    let margin = |lev: i64| Exec {
        lev,
        spent: 1000.0 / lev as f64,
        qty: 100.0,
        buy: 10.0,
        sell: 10.0,
        ..Default::default()
    };
    let totals = one_day(
        &seed_exec(&[margin(10), margin(25)]),
        ProjectionMode::Native,
    );
    let volume = totals.volume.expect("native money is summable");
    assert!((volume - 2000.0).abs() < 1e-9, "volume={volume}");
}

/// The same aggregate must NOT multiply a notional core, whose spend is already the traded value.
#[test]
fn turnover_leaves_a_notional_core_alone() {
    let notional = |lev: i64| Exec {
        lev,
        spent: 1000.0,
        qty: 100.0,
        buy: 10.0,
        sell: 10.0,
        ..Default::default()
    };
    let totals = one_day(
        &seed_exec(&[notional(10), notional(25)]),
        ProjectionMode::Native,
    );
    let volume = totals.volume.expect("native money is summable");
    assert!((volume - 2000.0).abs() < 1e-9, "volume={volume}");
}

/// Percent measures each trade against its own spend, so trades in different quotes share a scale.
/// Turnover and fee have none — publishing them would add USDT to USDC to BTC under one label.
#[test]
fn percent_projection_withholds_money_figures_but_keeps_counts() {
    let totals = one_day(
        &seed_exec(&[charged(100.0, 10.0, 11.0, 0.0004)]),
        ProjectionMode::Percent,
    );
    assert_eq!(totals.trades, 1, "counting still works in percent mode");
    assert!(totals.volume.is_none(), "turnover has no percent scale");
    assert!(totals.fee.is_none(), "cost has no percent scale");
    assert!(!totals.fee_is_complete());
}

/// A source without execution prices contributes no fee. The count of contributing trades is what
/// lets the UI label the figure approximate instead of presenting a silent undercount as exact.
#[test]
fn fee_coverage_reports_trades_without_prices() {
    // `seed_spent` has no boughtq/buyprice/sellprice at all, so every fee input projects NULL.
    let c = seed_spent(&[(D0 + 100, 1, 7.0, 100.0), (D0 + 200, 1, -2.0, 50.0)]);
    let totals = one_day(&c, ProjectionMode::Native);
    assert_eq!(totals.trades, 2);
    assert_eq!(totals.fee_trades, 0, "no row could form a fee");
    assert!(
        !totals.fee_is_complete(),
        "an undercount must not read as exact"
    );
    // Turnover needs only the spend column, so it stays available.
    assert_eq!(totals.volume, Some(150.0));
}

/// The card shows an average, so the aggregate must divide by trades — funding rows carry no
/// holding time and must not enter either side of that ratio.
#[test]
fn average_duration_covers_trades_only() {
    let held = |secs: i64| Exec {
        close: D0 + 10_000 + secs,
        ..Default::default()
    };
    let c = seed_exec(&[
        Exec {
            close: D0 + 10_000,
            ..held(0)
        },
        Exec {
            close: D0 + 20_000,
            ..Default::default()
        },
        Exec {
            close: D0 + 30_000,
            reason: "Funding",
            spent: 350.0,
            qty: 350.0,
            ..Default::default()
        },
    ]);
    let totals = one_day(&c, ProjectionMode::Native);
    // `seed_exec` opens every row 60 s before it closes; only the two trades count.
    assert_eq!(totals.trades, 2);
    assert_eq!(totals.duration_secs, 120);
    assert_eq!(totals.avg_duration_secs(), Some(60.0));
}

/// Turnover and cost must arrive in ONE currency. The projection converts `spentbtc` and
/// `profitbtc`, but the fee's gross leg is built from raw prices that nothing converts — so
/// without the projected `quote_rate` a USDC trade would contribute a native-currency cost to a
/// USDT total, and the error would be invisible at a 0.999 rate.
#[test]
fn mixed_quotes_convert_turnover_and_cost_to_one_currency() {
    const RATE: f64 = 0.999;
    let usdt = charged(100.0, 10.0, 11.0, 0.0004);
    let usdc = Exec {
        close: D0 + 200,
        quote: 8,
        ..charged(50.0, 20.0, 21.0, 0.0004)
    };
    let c = seed_exec(&[usdt.clone(), usdc.clone()]);
    c.execute_batch(
        "ATTACH ':memory:' AS valuation;
         CREATE TABLE valuation.rates (
             algorithm_version INTEGER, quote_ordinal INTEGER, minute_utc INTEGER,
             resolved_minute_utc INTEGER, rate_usdt REAL, price_basis INTEGER,
             provider TEXT, symbol TEXT, orientation INTEGER,
             candle_open_ms INTEGER, candle_close_ms INTEGER, leg1_rate REAL,
             leg2_provider TEXT, leg2_symbol TEXT, leg2_orientation INTEGER,
             leg2_rate REAL, fetched_at_ms INTEGER,
             PRIMARY KEY (algorithm_version, quote_ordinal, minute_utc)
         );
         CREATE TABLE valuation.trade_values (
             source_kind INTEGER, core_uid INTEGER, row_id INTEGER,
             algorithm_version INTEGER, closedate INTEGER, quote_ordinal INTEGER,
             profit_quote REAL, spent_quote REAL, rate_minute_utc INTEGER,
             rate_usdt REAL, profit_usdt REAL, spent_usdt REAL, valued_at_ms INTEGER,
             PRIMARY KEY (source_kind, core_uid, row_id)
         );",
    )
    .expect("attach valuation cache");
    c.execute(
        "INSERT INTO valuation.trade_values (
             source_kind, core_uid, row_id, algorithm_version, closedate, quote_ordinal,
             profit_quote, spent_quote, rate_minute_utc, rate_usdt,
             profit_usdt, spent_usdt, valued_at_ms
         )
         SELECT 0, core_uid, newrecid, ?1, closedate, basecurrency,
                profitbtc, spentbtc, (closedate/60)*60, ?2,
                profitbtc*?2, spentbtc*?2, 1780001000000
         FROM orders_rep WHERE basecurrency = 8",
        rusqlite::params![super::super::super::valuation::ALGORITHM_VERSION, RATE],
    )
    .expect("prepare the USDC valuation");

    let totals = one_day(&c, ProjectionMode::Usdt);

    assert_eq!(totals.trades, 2);
    // Turnover: the USDT spend as-is, the USDC spend at its rate.
    let expected_volume = usdt.spent + usdc.spent * RATE;
    let volume = totals.volume.expect("a converted scope is summable");
    assert!(
        (volume - expected_volume).abs() < 1e-9,
        "volume={volume} expected={expected_volume}"
    );
    // Cost: each leg's own round trip, the USDC one converted at the SAME rate as its money.
    let fee_of = |e: &Exec| e.qty * (e.buy + e.sell) * 0.0004;
    let expected_fee = fee_of(&usdt) + fee_of(&usdc) * RATE;
    let fee = totals.fee.expect("a converted scope is summable");
    assert!(
        (fee - expected_fee).abs() < 1e-9,
        "fee={fee} expected={expected_fee}"
    );
    assert!(totals.fee_is_complete(), "both trades priced");
}

/// Attach a valuation cache that values every seeded row at `rate`.
fn value_every_row(c: &Connection, rate: f64) {
    c.execute_batch(
        "ATTACH ':memory:' AS valuation;
         CREATE TABLE valuation.rates (
             algorithm_version INTEGER, quote_ordinal INTEGER, minute_utc INTEGER,
             resolved_minute_utc INTEGER, rate_usdt REAL, price_basis INTEGER,
             provider TEXT, symbol TEXT, orientation INTEGER,
             candle_open_ms INTEGER, candle_close_ms INTEGER, leg1_rate REAL,
             leg2_provider TEXT, leg2_symbol TEXT, leg2_orientation INTEGER,
             leg2_rate REAL, fetched_at_ms INTEGER,
             PRIMARY KEY (algorithm_version, quote_ordinal, minute_utc)
         );
         CREATE TABLE valuation.trade_values (
             source_kind INTEGER, core_uid INTEGER, row_id INTEGER,
             algorithm_version INTEGER, closedate INTEGER, quote_ordinal INTEGER,
             profit_quote REAL, spent_quote REAL, rate_minute_utc INTEGER,
             rate_usdt REAL, profit_usdt REAL, spent_usdt REAL, valued_at_ms INTEGER,
             PRIMARY KEY (source_kind, core_uid, row_id)
         );",
    )
    .expect("attach valuation cache");
    c.execute(
        "INSERT INTO valuation.trade_values (
             source_kind, core_uid, row_id, algorithm_version, closedate, quote_ordinal,
             profit_quote, spent_quote, rate_minute_utc, rate_usdt,
             profit_usdt, spent_usdt, valued_at_ms
         )
         SELECT 0, core_uid, newrecid, ?1, closedate, basecurrency,
                profitbtc, spentbtc, (closedate/60)*60, ?2,
                profitbtc*?2, spentbtc*?2, 1780001000000
         FROM orders_rep",
        rusqlite::params![super::super::super::valuation::ALGORITHM_VERSION, rate],
    )
    .expect("prepare valuations");
}

/// Without an explicit choice the unit follows whatever the PERIOD holds, which is why one
/// BTC-quoted core reads in BTC for a narrow range and in USDT for a wider one that also caught a
/// USDT trade. `prefer_usdt` is what makes the two ranges share a scale.
#[test]
fn prefer_usdt_pins_a_single_quote_scope_to_the_converted_unit() {
    const RATE: f64 = 60_000.0; // One BTC in USDT.
    // A BTC-quoted core, alone in the period: the scope has exactly one quote.
    let btc = Exec {
        quote: 0,
        ..charged(2.0, 1.0, 1.5, 0.0004)
    };
    let c = seed_exec(std::slice::from_ref(&btc));
    value_every_row(&c, RATE);
    let base = Query {
        from: D0,
        to: D0 + 86_400,
        ..Default::default()
    };

    // Default: the scope keeps its own quote, and the money stays unconverted.
    let native = calendar_period_from(&c, &base, None, false).expect("native scope reads");
    let ProfitScope::Comparable { unit, data } = &native else {
        panic!("a single-quote scope is comparable, got {native:?}");
    };
    assert_eq!(*unit, ProfitUnit::Quote(crate::db::QuoteCurrency::btc()));
    let native_volume = data.current[0].totals.volume.expect("native money sums");
    assert!(
        (native_volume - btc.spent).abs() < 1e-9,
        "volume={native_volume} expected={}",
        btc.spent
    );

    // Chosen: the same scope is reported in USDT, and every figure is converted with it.
    let pinned = calendar_period_from(
        &c,
        &Query {
            prefer_usdt: true,
            ..base
        },
        None,
        false,
    )
    .expect("pinned scope reads");
    let ProfitScope::Comparable { unit, data } = &pinned else {
        panic!("a valued scope stays comparable, got {pinned:?}");
    };
    assert_eq!(*unit, ProfitUnit::Quote(crate::db::QuoteCurrency::usdt()));
    let usdt_volume = data.current[0].totals.volume.expect("converted money sums");
    assert!(
        (usdt_volume - btc.spent * RATE).abs() < 1e-6,
        "volume={usdt_volume} expected={}",
        btc.spent * RATE
    );
    // The cost rides the same rate, or the card would mix a BTC fee into a USDT total.
    let usdt_fee = data.current[0].totals.fee.expect("converted money sums");
    let expected_fee = btc.qty * (btc.buy + btc.sell) * 0.0004 * RATE;
    assert!(
        (usdt_fee - expected_fee).abs() < 1e-6,
        "fee={usdt_fee} expected={expected_fee}"
    );
}

/// A USDT scope must not be routed through valuation to become itself: the conversion would gate
/// an already-exact scope behind rate coverage it does not need.
#[test]
fn prefer_usdt_leaves_a_usdt_scope_on_the_native_path() {
    // No valuation cache is attached at all, so a conversion attempt could not be satisfied.
    let c = seed_exec(&[charged(100.0, 10.0, 11.0, 0.0004)]);
    let scope = calendar_period_from(
        &c,
        &Query {
            from: D0,
            to: D0 + 86_400,
            prefer_usdt: true,
            ..Default::default()
        },
        None,
        false,
    )
    .expect("USDT scope reads without valuation");
    let ProfitScope::Comparable { unit, .. } = &scope else {
        panic!("a USDT scope is comparable, got {scope:?}");
    };
    assert_eq!(*unit, ProfitUnit::Quote(crate::db::QuoteCurrency::usdt()));
}

/// An inverse COIN-M row counts CONTRACTS against USD prices while settling in coin, so its gross
/// leg is not even in the row's own currency. Measured on the live replica: 100 such rows produced
/// 12.8M of "cost" against a real monthly turnover of 2.3M. It must leave the fee alone — and say
/// so through the coverage count, not by contributing a plausible-looking number.
#[test]
fn an_inverse_contract_contributes_no_cost() {
    // `DENOMINATION_RULES` relabels a COIN-M row: labeled USDT, denominated BTC. The fixture uses
    // the market spelling that rule keys on, so the projection marks its prices as foreign.
    let c = seed_exec(&[
        Exec {
            // COIN-M spells its markets `USD-<COIN>`; a USD-M core writes `USDT-` for the same
            // dated contract, which is the veto that keeps ordinary rows out of the rule.
            coin: "BTC_1226",
            fname: "Pump_USD-BTC_1226_20260101",
            quote: 1,
            qty: 2537.0,
            buy: 128_146.0,
            sell: 127_443.0,
            spent: 2.05,
            profit: 0.0106,
            ..Default::default()
        },
        // An ordinary trade beside it, so the cell still has a cost to report.
        Exec {
            close: D0 + 300,
            ..charged(100.0, 10.0, 11.0, 0.0004)
        },
    ]);
    let totals = one_day(&c, ProjectionMode::Native);

    assert_eq!(totals.trades, 2, "both rows are trades");
    assert_eq!(totals.fee_trades, 1, "only the priceable one has a cost");
    assert!(
        !totals.fee_is_complete(),
        "a partial cost must announce itself"
    );
    let fee = totals.fee.expect("native money is summable");
    let expected = 100.0 * (10.0 + 11.0) * 0.0004;
    assert!(
        (fee - expected).abs() < 1e-9,
        "fee={fee} expected={expected} — the contract row leaked in"
    );
}

/// Twelve live rows close with no exit price, and their gross leg alone subtracted 71.5 from a
/// month's cost. A trade that cannot be priced contributes nothing and lowers the coverage.
#[test]
fn a_trade_without_an_exit_price_contributes_no_cost() {
    let c = seed_exec(&[
        Exec {
            sell: 0.0,
            profit: -3.0,
            ..Default::default()
        },
        Exec {
            close: D0 + 300,
            ..charged(100.0, 10.0, 11.0, 0.0004)
        },
    ]);
    let totals = one_day(&c, ProjectionMode::Native);

    assert_eq!(totals.trades, 2);
    assert_eq!(totals.fee_trades, 1);
    let fee = totals.fee.expect("native money is summable");
    let expected = 100.0 * (10.0 + 11.0) * 0.0004;
    assert!(
        (fee - expected).abs() < 1e-9,
        "fee={fee} expected={expected}"
    );
    // Turnover needs only the spend column, so the unpriced trade still counts there.
    assert_eq!(
        totals.volume,
        Some(1000.0 + charged(100.0, 10.0, 11.0, 0.0004).spent)
    );
}

/// MoonBot delivers one funding accrual to EVERY bot on the account, and its own report warns
/// about the duplication. The replica keys rows per core, so both copies are stored — counting
/// both would inflate the day's profit by the whole accrual.
#[test]
fn a_funding_accrual_delivered_to_two_bots_is_counted_once() {
    let accrual = Exec {
        profit: 2.5,
        spent: 350.0,
        qty: 350.0,
        buy: 0.5,
        sell: 0.5,
        reason: "Funding",
        coin: "BNB",
        ..Default::default()
    };
    let c = seed_exec(&[
        // The same accrual as two cores saw it, seconds apart, plus one ordinary trade.
        Exec {
            core: 1,
            ..accrual.clone()
        },
        Exec {
            core: 2,
            close: accrual.close + 3,
            ..accrual.clone()
        },
        charged(100.0, 10.0, 11.0, 0.0004),
    ]);
    let totals = one_day(&c, ProjectionMode::Native);

    assert_eq!(totals.funding, Some(2.5), "the redelivered copy is dropped");
    let traded = charged(100.0, 10.0, 11.0, 0.0004).profit;
    assert!(
        (totals.profit - (traded + 2.5)).abs() < 1e-9,
        "profit={} expected={}",
        totals.profit,
        traded + 2.5
    );
    assert_eq!(totals.trades, 1, "neither copy is a trade");
}

/// Two ACCOUNTS are charged in the same second — funding fires on a fixed schedule — but for
/// different positions. Collapsing those would erase real money, so the amount is what separates
/// a redelivery from a coincidence.
#[test]
fn two_accounts_charged_at_the_same_moment_both_count() {
    let base = Exec {
        spent: 350.0,
        qty: 350.0,
        buy: 0.5,
        sell: 0.5,
        reason: "Funding",
        coin: "BNB",
        ..Default::default()
    };
    let c = seed_exec(&[
        Exec {
            core: 1,
            profit: -0.00024915,
            ..base.clone()
        },
        Exec {
            core: 2,
            profit: -0.00049860,
            ..base.clone()
        },
    ]);
    let totals = one_day(&c, ProjectionMode::Native);

    let both = -0.00024915 + -0.00049860;
    let funding = totals.funding.expect("native money is summable");
    assert!(
        (funding - both).abs() < 1e-12,
        "funding={funding} expected={both} — different amounts are different accruals"
    );
}

/// The same accrual on the same core is one row, so a same-core repeat is a redelivery too; and a
/// position of a different size is a different accrual even at the same price.
#[test]
fn dedup_keys_on_the_position_as_well_as_the_amount() {
    let base = Exec {
        profit: 2.5,
        spent: 350.0,
        buy: 0.5,
        sell: 0.5,
        reason: "Funding",
        coin: "BNB",
        ..Default::default()
    };
    let c = seed_exec(&[
        Exec {
            core: 1,
            qty: 350.0,
            ..base.clone()
        },
        // Same amount, DIFFERENT position: a second account that happens to owe the same.
        Exec {
            core: 2,
            qty: 700.0,
            ..base.clone()
        },
    ]);
    let totals = one_day(&c, ProjectionMode::Native);
    assert_eq!(
        totals.funding,
        Some(5.0),
        "different sizes are different accruals"
    );
}

/// Percent measures a return on spent capital, and funding has no spend to measure against.
#[test]
fn percent_projection_reports_no_funding() {
    let c = seed_exec(&[Exec {
        profit: 2.5,
        spent: 350.0,
        qty: 350.0,
        buy: 0.5,
        sell: 0.5,
        reason: "Funding",
        ..Default::default()
    }]);
    assert!(one_day(&c, ProjectionMode::Percent).funding.is_none());
}

/// A day whose only row is funding still moved money, so the grid must not treat it as empty.
#[test]
fn a_funding_only_day_is_not_empty() {
    let c = seed_exec(&[Exec {
        profit: 2.5,
        spent: 350.0,
        qty: 350.0,
        reason: "Funding",
        ..Default::default()
    }]);
    let q = Query {
        from: D0,
        to: D0 + 86_400,
        ..Default::default()
    };
    let days = calendar_cells_from(&c, &q, ProjectionMode::Native).unwrap();
    assert_eq!(days.len(), 1);
    assert_eq!(days[0].totals.trades, 0);
    assert!(
        days[0].has_activity(),
        "money without a trade is still activity"
    );
}
