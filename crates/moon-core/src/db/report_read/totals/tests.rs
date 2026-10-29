//! Every slice of a sliced totals read states exactly what its own `query_totals` states.

use rusqlite::Connection;

use super::super::{ReportFilter, RowScope, query_totals};
use super::{TotalsSlice, query_totals_sliced};

/// Day length the fixture's windows are cut on.
const DAY: i64 = 86_400;
/// First day of the fixture period, true UTC.
const START: i64 = 1_790_000_000 - 1_790_000_000 % DAY;

/// Measure the Telegram slice shape before changing its row-to-slice dispatch.
#[test]
#[ignore = "synthetic release benchmark"]
fn bench_sliced_totals_tg_shape() {
    let conn = fixture();
    conn.execute("DELETE FROM orders_rep", []).unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    {
        let mut insert = tx
            .prepare(
                "INSERT INTO orders_rep VALUES
                 (?1, 'core', ?2, 'COIN', '', 1, ?3, ?3 - 300,
                  1.0, 100.0, 1.0, 100.0, 101.0, 'Sell', 0, 0)",
            )
            .unwrap();
        for rec in 0..300_000_i64 {
            let close = START + rec * (365 * DAY) / 300_000;
            insert
                .execute(rusqlite::params![rec % 200 + 1, rec, close])
                .unwrap();
        }
    }
    tx.commit().unwrap();
    let to = START + 365 * DAY - 1;
    let whole = |core_uids| TotalsSlice {
        core_uids,
        date_from: Some(START),
        date_to: Some(to),
    };
    let mut slices = vec![whole(None)];
    // Four synthetic exchanges partition the 200 cores, as Telegram's exchange sections do.
    for exchange in 0..4_u64 {
        slices.push(whole(Some(
            (1..=200)
                .filter(|core| (core - 1) % 4 == exchange)
                .collect(),
        )));
    }
    for core in 1..=200 {
        slices.push(whole(Some(vec![core])));
    }
    for day in 0..365 {
        slices.push(TotalsSlice {
            core_uids: None,
            date_from: Some(START + day * DAY),
            date_to: Some(START + (day + 1) * DAY - 1),
        });
    }
    let base = ReportFilter {
        date_from: Some(START),
        date_to: Some(to),
        emulator: Some(false),
        rows: RowScope::Closed,
        ..ReportFilter::default()
    };
    let mut timings = Vec::with_capacity(5);
    assert!(super::one_pass_serves(&base, &slices));
    let pass = one_pass(&conn, &base, &slices).expect("benchmark must not fall back");
    assert_eq!(pass.len(), slices.len());
    assert!(
        pass.iter().all(Option::is_some),
        "benchmark must refuse no slice"
    );
    drop(pass);
    for _ in 0..5 {
        let start = std::time::Instant::now();
        let totals = query_totals_sliced(&conn, &base, &slices).unwrap();
        timings.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(totals.len(), 570);
        assert_eq!(totals[0].quotes.orders, 300_000);
        for total in &totals[1..5] {
            assert_eq!(total.quotes.orders, 75_000);
        }
        for total in &totals[5..205] {
            assert_eq!(total.quotes.orders, 1_500);
        }
        assert_eq!(
            totals[205..]
                .iter()
                .map(|total| total.quotes.orders)
                .sum::<i64>(),
            300_000
        );
        std::hint::black_box(totals);
    }
    timings.sort_by(f64::total_cmp);
    println!("[BENCH] sliced_totals median_ms={:.3}", timings[2]);
}

/// A replica with the money columns the totals read: three cores (two on measured clocks one
/// either side of UTC, one never measured), three quotes, reals across magnitudes and signs,
/// Funding and liquidation rows, and closes on both edges of every day.
fn fixture() -> Connection {
    let conn = Connection::open_in_memory().expect("open sliced fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, core_name TEXT, newrecid INTEGER NOT NULL,
             coin TEXT, fname TEXT, basecurrency INTEGER, closedate INTEGER, buydate INTEGER,
             profitbtc REAL, spentbtc REAL, boughtq REAL, buyprice REAL, sellprice REAL,
             sellreason TEXT, emulator INTEGER, deleted INTEGER,
             PRIMARY KEY (core_uid, newrecid)
         );
         CREATE INDEX idx_rep_closedate ON orders_rep(closedate);
         CREATE INDEX idx_rep_core_close ON orders_rep(core_uid, closedate);",
    )
    .expect("create sliced fixture");
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut insert = conn
        .prepare(
            "INSERT INTO orders_rep VALUES (?1, 'core', ?2, 'COIN', '', ?3, ?4, ?5, ?6, ?7, ?8,
                                            ?9, ?10, ?11, ?12, 0)",
        )
        .expect("prepare fixture insert");
    for rec in 0..3_000_i64 {
        let core = [1_i64, 2, 3][(next() % 3) as usize];
        // Offsets below are +3600 for core 1 and -7200 for core 2, so a close near midnight
        // lands on a different UTC day than its raw value says.
        let close = START - DAY + (next() % (9 * DAY as u64)) as i64;
        let magnitude = [1e-9, 1e-3, 1.0, 1e4, 1e9][(next() % 5) as usize];
        let profit = ((next() % 2_000_001) as f64 - 1_000_000.0) / 999_983.0 * magnitude;
        let spent = (next() % 5_000) as f64 / 7.0 * magnitude;
        let qty = (next() % 900 + 1) as f64 / 3.0;
        let buy = (next() % 50_000 + 1) as f64 / 1_000.0;
        let sell = buy * (1.0 + ((next() % 200) as f64 - 100.0) / 10_000.0);
        let reason = ["Sell", "Stop", "Funding", "LIQUIDATION", ""][(next() % 5) as usize];
        let quote = [0_i64, 1, 5][(next() % 3) as usize];
        let emulator = i64::from(next() % 10 == 0);
        insert
            .execute(rusqlite::params![
                core,
                rec,
                quote,
                close,
                close - 3_600,
                profit,
                spent,
                qty,
                buy,
                sell,
                reason,
                emulator
            ])
            .expect("insert fixture row");
    }
    drop(insert);
    conn
}

/// The fixture's clock axis: core 1 ahead of UTC, core 2 behind, core 3 never measured.
fn axis() -> crate::db::ReportAxis {
    let segment = |offset_secs| {
        vec![crate::db::OffsetSegment {
            from_utc: 0,
            offset_secs,
        }]
    };
    crate::db::ReportAxis::from_measured(
        std::collections::HashMap::from([(1, segment(3_600)), (2, segment(-7_200))]),
        chrono_tz::UTC,
    )
}

/// The slices the Mini App asks for: the period, core groups, single cores, and every day.
fn slices(from: i64, to: i64) -> Vec<TotalsSlice> {
    let mut slices = vec![TotalsSlice {
        core_uids: None,
        date_from: Some(from),
        date_to: Some(to),
    }];
    for cores in [vec![1, 2], vec![3], vec![1], vec![2], vec![2, 3]] {
        slices.push(TotalsSlice {
            core_uids: Some(cores),
            date_from: Some(from),
            date_to: Some(to),
        });
    }
    let mut day = from;
    while day <= to {
        slices.push(TotalsSlice {
            core_uids: None,
            date_from: Some(day),
            date_to: Some((day + DAY - 1).min(to)),
        });
        day += DAY;
    }
    slices
}

/// Owner and scoped reads, over a period with a partial last day: each slice's totals, printed
/// in full, equal the totals its own filter reads.
#[test]
fn every_slice_equals_its_own_totals_read() {
    let conn = fixture();
    let (from, to) = (START, START + 6 * DAY + 50_000);
    for scope in [vec![], vec![1, 2, 3], vec![1, 2]] {
        let base = ReportFilter {
            core_uids: scope.clone(),
            date_from: Some(from),
            date_to: Some(to),
            emulator: Some(false),
            rows: RowScope::Closed,
            axis: axis(),
            ..ReportFilter::default()
        };
        let slices = slices(from, to)
            .into_iter()
            .filter(|slice| {
                slice
                    .core_uids
                    .as_ref()
                    .is_none_or(|cores| scope.is_empty() || cores.iter().all(|c| scope.contains(c)))
            })
            .collect::<Vec<_>>();
        let pass = one_pass(&conn, &base, &slices).expect("clean data takes the single pass");
        assert!(
            pass.iter().all(Option::is_some),
            "clean data refuses no slice"
        );
        let sliced = query_totals_sliced(&conn, &base, &slices).expect("sliced read");
        assert_eq!(sliced.len(), slices.len());
        for (slice, totals) in slices.iter().zip(&sliced) {
            let own = query_totals(&conn, &slice.filter(&base)).expect("own read");
            assert!(own.quotes.orders > 0, "the fixture must populate {slice:?}");
            assert_eq!(
                format!("{totals:?}"),
                format!("{own:?}"),
                "scope {scope:?}, slice {slice:?}"
            );
        }
    }
}

/// A money cell SQLite would coerce from TEXT sends only the slices holding it down their own
/// statements, which still answer exactly; a base filter the single pass does not cover sends
/// every slice there.
#[test]
fn unrepresentable_values_and_shapes_fall_back_to_per_slice_reads() {
    let conn = fixture();
    conn.execute(
        "UPDATE orders_rep SET profitbtc = 'n/a', closedate = ?1 WHERE newrecid = 7",
        [START + DAY],
    )
    .expect("store a TEXT profit inside the window");
    let (from, to) = (START, START + 3 * DAY);
    for rows in [RowScope::Closed, RowScope::ClosedAndOpen] {
        let base = ReportFilter {
            date_from: Some(from),
            date_to: Some(to),
            rows,
            axis: axis(),
            ..ReportFilter::default()
        };
        let slices = slices(from, to);
        if rows == RowScope::Closed {
            let pass = one_pass(&conn, &base, &slices).expect("the pass still places every row");
            let refused = pass.iter().filter(|totals| totals.is_none()).count();
            assert!(
                refused > 0 && refused < slices.len(),
                "only the slices holding the TEXT cell are refused: {refused} of {}",
                slices.len()
            );
        } else {
            assert!(!super::one_pass_serves(&base, &slices));
        }
        let sliced = query_totals_sliced(&conn, &base, &slices).expect("sliced read");
        for (slice, totals) in slices.iter().zip(&sliced) {
            let own = query_totals(&conn, &slice.filter(&base)).expect("own read");
            assert_eq!(
                format!("{totals:?}"),
                format!("{own:?}"),
                "{rows:?} {slice:?}"
            );
        }
    }
}

/// A slice reaching outside the base filter is answered by its own read, never clipped to the
/// base pass.
#[test]
fn slices_outside_the_base_are_not_clipped() {
    let conn = fixture();
    let base = ReportFilter {
        core_uids: vec![1],
        date_from: Some(START + DAY),
        date_to: Some(START + 2 * DAY),
        rows: RowScope::Closed,
        axis: axis(),
        ..ReportFilter::default()
    };
    let slices = [
        TotalsSlice {
            core_uids: Some(vec![2]),
            date_from: base.date_from,
            date_to: base.date_to,
        },
        TotalsSlice {
            core_uids: None,
            date_from: Some(START),
            date_to: base.date_to,
        },
    ];
    assert!(!super::one_pass_serves(&base, &slices));
    let sliced = query_totals_sliced(&conn, &base, &slices).expect("sliced read");
    for (slice, totals) in slices.iter().zip(&sliced) {
        let own = query_totals(&conn, &slice.filter(&base)).expect("own read");
        assert!(own.quotes.orders > 0);
        assert_eq!(format!("{totals:?}"), format!("{own:?}"));
    }
}

/// The single pass alone, without the per-slice fallback behind it.
fn one_pass(
    conn: &Connection,
    base: &ReportFilter,
    slices: &[TotalsSlice],
) -> Option<Vec<Option<super::ReportTotals>>> {
    let sources = super::read_sources_res(conn).expect("discover sources");
    let now = crate::util::now_unix_ms_i64().div_euclid(1_000);
    let meta = super::super::report_strategy_meta(conn, base).expect("strategy meta");
    super::sliced_attempt(conn, base, slices, &sources, &meta, false, now).expect("single pass")
}

use super::*;

/// One slice's admission test over the base pass's rows.
struct FormerSlicePlan {
    /// Cores admitted, `None` for every core the base admits.
    cores: Option<HashSet<i64>>,
    date_from: Option<i64>,
    date_to: Option<i64>,
}

impl FormerSlicePlan {
    fn new(slice: &TotalsSlice) -> Self {
        Self {
            // Inlined the way `build_where` inlines them, so the comparison is the SQL's own.
            cores: slice
                .core_uids
                .as_ref()
                .filter(|cores| !cores.is_empty())
                .map(|cores| cores.iter().map(|&uid| uid as i64).collect()),
            date_from: slice.date_from,
            date_to: slice.date_to,
        }
    }

    /// Whether this slice's own statement would admit a row the base pass returned.
    ///
    /// The base pass already applied everything the slice shares with it; what is left is the
    /// slice's core list and its window, shifted onto the core's clock exactly as
    /// `append_row_scope` shifts it.
    ///
    /// Args:
    ///     core: The row's `core_uid`.
    ///     close: The row's `closedate`, `None` on a source without the column (its statement
    ///         applies no window at all).
    ///     offset: The core's clock offset at the read's instant.
    ///
    /// Returns:
    ///     `None` when the close date is not a number this test can compare exactly.
    fn admits(&self, core: i64, close: Option<&Value>, offset: i32) -> Option<bool> {
        if self
            .cores
            .as_ref()
            .is_some_and(|cores| !cores.contains(&core))
        {
            return Some(false);
        }
        let Some(close) = close else {
            return Some(true);
        };
        let at_least =
            |bound: i64| compare(close, ReportAxis::shift_bound(bound, offset)).map(|o| o.is_ge());
        let at_most =
            |bound: i64| compare(close, ReportAxis::shift_bound(bound, offset)).map(|o| o.is_le());
        let from_ok = match self.date_from {
            Some(bound) => at_least(bound)?,
            None => true,
        };
        let to_ok = match self.date_to {
            Some(bound) => at_most(bound)?,
            None => true,
        };
        Some(from_ok && to_ok)
    }
}

/// Frozen complete per-row fold before indexed slice admission.
fn former_sliced_attempt(
    conn: &Connection,
    base: &ReportFilter,
    slices: &[TotalsSlice],
    sources: &[ReadSource],
    meta: &StrategyMeta,
    include_valuation: bool,
    now: i64,
) -> rusqlite::Result<Option<Vec<Option<ReportTotals>>>> {
    let valuation_present = base.valuation == ValuationMode::Current || include_valuation;
    let plans = slices.iter().map(FormerSlicePlan::new).collect::<Vec<_>>();
    let passes = sources
        .iter()
        .map(|src| ClosedPass::new(src, base, include_valuation, meta))
        .collect::<Vec<_>>();
    let mut offsets: HashMap<i64, i32> = HashMap::new();
    // slice -> source -> quote key -> one fold per column
    let mut folds: Vec<Vec<BTreeMap<GroupKey, Vec<SqliteSum>>>> =
        vec![vec![BTreeMap::new(); passes.len()]; plans.len()];
    // A slice one of whose sums the fold refused; its own statement answers it instead.
    let mut refused = vec![false; plans.len()];
    // Same gate as the grouped read: only an empty core list has no cardinality of its own.
    let count_cores: Vec<bool> = slices
        .iter()
        .map(|slice| slice.filter(base).core_uids.is_empty())
        .collect();
    let mut counted_cores: Vec<HashSet<i64>> = vec![HashSet::new(); plans.len()];
    for (source, pass) in passes.iter().enumerate() {
        let has_close = pass.src.cols.contains("closedate");
        let mut stmt = conn.prepare(&pass.row_sql())?;
        let mut rows = stmt.query(pass.refs().as_slice())?;
        let width = pass.columns.len();
        let mut values = Vec::with_capacity(width);
        while let Some(row) = rows.next()? {
            let Some(key) = GroupKey::of(&row.get::<_, Value>(0)?) else {
                return Ok(None);
            };
            let Value::Integer(core) = row.get::<_, Value>(1)? else {
                return Ok(None);
            };
            let close = row.get::<_, Value>(2)?;
            values.clear();
            for index in 0..width {
                values.push(row.get::<_, Value>(3 + index)?);
            }
            let offset = *offsets
                .entry(core)
                .or_insert_with(|| core_offset(&base.axis, core, now));
            for (index, ((plan, slice_folds), refused)) in plans
                .iter()
                .zip(folds.iter_mut())
                .zip(refused.iter_mut())
                .enumerate()
            {
                match plan.admits(core, has_close.then_some(&close), offset) {
                    Some(true) => {}
                    Some(false) => continue,
                    None => return Ok(None),
                }
                if *refused {
                    continue;
                }
                let sums = slice_folds[source]
                    .entry(key)
                    .or_insert_with(|| vec![SqliteSum::default(); width]);
                let failed = sums
                    .iter_mut()
                    .zip(&values)
                    .any(|(sum, value)| sum.step(value).is_err());
                *refused = failed;
                // A refused slice is answered by its own statement, so its set is discarded.
                if !failed && count_cores[index] && row_counted(pass, &values) {
                    counted_cores[index].insert(core);
                }
            }
        }
    }
    // Each statement of the base pass read the clock on its own. Had any measured core's offset
    // moved between this read's instant and now -- a core with rows or one whose rows it shifted
    // out -- the base pass and the slices could disagree about its window. An unmeasured core is
    // the identity at every instant.
    let later = crate::util::now_unix_ms_i64().div_euclid(1_000);
    if base.axis.measured_cores().into_iter().any(|core| {
        core_offset(&base.axis, core as i64, now) != core_offset(&base.axis, core as i64, later)
    }) {
        return Ok(None);
    }
    let mut totals = Vec::with_capacity(folds.len());
    for ((slice_folds, refused), cores) in folds.into_iter().zip(refused).zip(counted_cores) {
        totals.push(if refused {
            None
        } else {
            finish_slice(&passes, slice_folds, valuation_present, cores.len() as u64)?
        });
    }
    Ok(Some(totals))
}

/// Compare both complete folds on the exact same fixture, avoiding feature or machine-load confounds.
#[test]
#[ignore = "synthetic reader timing probe"]
fn sliced_refresh_scale_probe() {
    use crate::db::report_read::tests::seeks::{replica, seed};
    let conn = replica();
    seed(&conn, 100_000, 10, 60, 1_000, 8);
    let base = ReportFilter {
        rows: RowScope::Closed,
        ..Default::default()
    };
    let slices = std::iter::once(TotalsSlice {
        core_uids: None,
        date_from: None,
        date_to: None,
    })
    .chain((0..31).map(|day| TotalsSlice {
        core_uids: None,
        date_from: Some(1_700_000_000 + day * DAY),
        date_to: Some(1_700_000_000 + (day + 1) * DAY - 1),
    }))
    .collect::<Vec<_>>();
    let sources = super::read_sources_res(&conn).unwrap();
    let meta = super::super::report_strategy_meta(&conn, &base).unwrap();
    let now = crate::util::now_unix_ms_i64().div_euclid(1_000);
    let old = former_sliced_attempt(&conn, &base, &slices, &sources, &meta, false, now).unwrap();
    let new = super::sliced_attempt(&conn, &base, &slices, &sources, &meta, false, now).unwrap();
    assert_eq!(format!("{old:?}"), format!("{new:?}"));
    for round in 0..6 {
        let before = round % 2 == 0;
        let start = std::time::Instant::now();
        let value = if before {
            former_sliced_attempt(&conn, &base, &slices, &sources, &meta, false, now)
        } else {
            super::sliced_attempt(&conn, &base, &slices, &sources, &meta, false, now)
        }
        .unwrap();
        std::hint::black_box(value);
        println!(
            "reader_refresh sliced former={before}: {:.3} ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
}
