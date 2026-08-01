//! Filter-tuner query, snapshot, and predicate regression tests.

use super::*;

/// Raw tuner scans reject mixed quotes before optimization, while Percent scans remain valid.
///
/// Removing the guard from `tuner_source_on` lets threshold and variant optimizers rank a scalar
/// whose operands are USDT and USDC. Applying the same guard to Percent breaks the second branch.
#[test]
fn mixed_quote_tuner_rejects_raw_money_but_accepts_percent() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE orders_rep(
            closedate INTEGER, core_uid INTEGER, profitbtc REAL, spentbtc REAL,
            basecurrency INTEGER
         );
         INSERT INTO orders_rep VALUES (100, 1, 10.0, 100.0, 1);
         INSERT INTO orders_rep VALUES (200, 1, 5.0, 100.0, 8);",
    )
    .expect("mixed quote fixture");
    let raw = Query {
        from: 1,
        to: 300,
        ..Default::default()
    };

    assert!(matches!(
        tuner_source_on(&conn, &raw),
        Err(ReadFail::IncomparableQuote)
    ));

    let percent = Query {
        metric: crate::db::ProfitMetric::Percent,
        ..raw
    };
    assert!(tuner_source_on(&conn, &percent).is_ok());
}

/// Quote preflight and tuner row materialization observe one WAL snapshot.
///
/// Replacing `tuner_read_on`'s `with_read_snapshot` call with a direct callback lets the second
/// query observe the USDC commit below after preflight accepted the original USDT-only scope.
#[test]
fn tuner_materialization_cannot_cross_the_quote_preflight_snapshot() {
    let path = super::super::test_support::temp_db("tuner-quote-snapshot");
    let writer = Connection::open(&path).expect("open writer");
    writer
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE orders_rep(
                closedate INTEGER, core_uid INTEGER, profitbtc REAL, spentbtc REAL,
                basecurrency INTEGER
             );
             INSERT INTO orders_rep VALUES (100, 1, 10.0, 100.0, 1);",
        )
        .expect("seed WAL fixture");
    let reader = Connection::open(&path).expect("open reader");
    let query = Query {
        from: 1,
        to: 300,
        ..Default::default()
    };

    let rows = tuner_read_on(&reader, &query, |snapshot, query, source| {
        writer
            .execute("INSERT INTO orders_rep VALUES (200, 1, 5.0, 100.0, 8)", [])
            .expect("commit USDC row after quote preflight");
        snapshot
            .query_row(
                &format!("SELECT COUNT(*) FROM {source}"),
                rusqlite::params![query.from, query.to],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| read_fail("test: tuner snapshot materialization", error))
    })
    .expect("materialize pinned tuner rows");

    assert_eq!(rows, 1);
    drop(reader);
    drop(writer);
    super::super::test_support::remove_db(&path);
}

#[test]
fn variant_where_whitelists_fields() {
    let v = Variant {
        bounds: vec![
            Bound {
                field: "d1h".into(),
                from: Some(1.5),
                to: Some(10.0),
            },
            Bound {
                field: "evil\"; DROP TABLE x;--".into(),
                from: Some(1.0),
                to: None,
            },
            Bound {
                field: "hvol".into(),
                from: None,
                to: None,
            },
        ],
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("COALESCE(o.\"d1h\",0) >= 1.5"));
    assert!(w.contains("<= 10"));
    assert!(!w.contains("DROP"));
    assert!(!w.contains("hvol"), "пустые границы не добавляют условий");
}

/// The variant's only STRING condition: a coin name reaches SQL as a literal,
/// so its apostrophe must be doubled — otherwise one such coin breaks the whole
/// WHERE and "Fact vs v1" quietly scores a different set of trades.
#[test]
fn variant_coins_quote_is_escaped() {
    let v = Variant {
        coins_in: Some(vec!["BTC".into(), "O'BRIEN' OR 1=1--".into()]),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("IN ('BTC','O''BRIEN'' OR 1=1--')"), "w={w}");
    assert!(!v.is_empty(), "a coin variant is not the fact");
    // No dangling quote: their count in the condition stays even.
    assert_eq!(w.matches('\'').count() % 2, 0, "unbalanced quote: {w}");
    assert!(
        Variant::default().where_sql().is_empty(),
        "an empty coin list adds no condition"
    );
    // The blacklist side EXCLUDES, and both sides may apply at once.
    let both = Variant {
        coins_in: Some(vec!["BTC".into()]),
        coins_out: vec!["ETH".into()],
        ..Default::default()
    };
    let w = both.where_sql();
    assert!(w.contains("IN ('BTC')"), "w={w}");
    assert!(w.contains("NOT IN ('ETH')"), "w={w}");

    // A whitelist that no traded coin satisfies keeps NOTHING — it must not quietly
    // degrade into "no whitelist", which would score the untouched fact as the plan.
    let unmatched = Variant {
        coins_in: Some(Vec::new()),
        ..Default::default()
    };
    assert!(unmatched.where_sql().contains("0=1"));
    assert!(
        !unmatched.is_empty(),
        "an unmatched whitelist is not the fact"
    );
}

#[test]
fn variant_week_span_predicate() {
    // Mon 00:00 -> Sat 23:59 (week minutes 0..8639): continuous -> BETWEEN, excluding Sun.
    let v = Variant {
        week_span: Some((0, 8639)),
        ..Default::default()
    };
    let w = v.where_sql();
    // Use OPEN_TS (buydate with a closedate fallback), not closedate alone.
    assert!(
        w.contains("BETWEEN 0 AND 8639") && w.contains("buydate"),
        "w={w}"
    );
    assert!(!v.is_empty(), "week_span-вариант не равен «Факту»");

    // Wrap Sun -> Mon (from > to): Sat 12:00 (8640-720=7920) -> Mon 12:00 (720).
    let v = Variant {
        week_span: Some((7920, 720)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(
        w.contains("<= 720 OR"),
        "через воскресенье — до Пн 12:00: {w}"
    );
    assert!(
        w.contains(">= 7920"),
        "через воскресенье — от Сб 12:00: {w}"
    );
    assert!(!w.contains("BETWEEN"), "перевёрнутое окно не BETWEEN: {w}");
}

#[test]
fn variant_time_window_predicate() {
    // WorkingTime `Day` 09:00-21:00 -> minute-of-day BETWEEN.
    let v = Variant {
        tod: Some(TimeWindow::Day(9 * 60, 21 * 60)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(
        w.contains("BETWEEN 540 AND 1260") && w.contains("buydate"),
        "w={w}"
    );
    assert!(!v.is_empty());

    // `Day` wrapping past midnight (22:00-06:00) -> `<= 360 OR >= 1320`.
    let v = Variant {
        tod: Some(TimeWindow::Day(22 * 60, 6 * 60)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("<= 360 OR"), "через полночь до 06:00: {w}");
    assert!(w.contains(">= 1320"), "через полночь от 22:00: {w}");

    // WorkingTime `Hour` 1-50 -> minute-within-hour (mod 60) BETWEEN.
    let v = Variant {
        tod: Some(TimeWindow::Hour(1, 50)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("% 60) BETWEEN 1 AND 50"), "w={w}");

    // `week_span` AND `tod` combine into one WHERE clause (both axes).
    let v = Variant {
        week_span: Some((0, 8639)),
        tod: Some(TimeWindow::Day(1, 1430)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("BETWEEN 0 AND 8639"));
    assert!(w.contains("BETWEEN 1 AND 1430"));
}

#[test]
fn best_range_skips_noop_full_range() {
    // Every trade is profitable: no subrange beats no filter, so a no-op min/max pair
    // must not be suggested.
    let mut vals: Vec<(f64, f64)> = (0..100).map(|i| (i as f64, 1.0)).collect();
    assert!(best_range(&mut vals, 1, 16, false).is_none());
    // The lower third loses: suggest a range that is NOT the full extent.
    let mut vals: Vec<(f64, f64)> = (0..99)
        .map(|i| (i as f64, if i < 33 { -1.0 } else { 1.0 }))
        .collect();
    let s = best_range(&mut vals, 1, 16, false).expect("фильтр должен найтись");
    assert!(
        s.from.unwrap() > 0.0,
        "нижняя граница должна отрезать минус"
    );
    assert_eq!(s.to.unwrap(), 98.0, "верхняя пара — фактический max данных");
}

#[test]
fn round_falls_back_when_pair_stops_cutting() {
    // The range cuts only two losing tail values at the data extremes. Outward rounding would
    // move BOTH bounds beyond min/max and make a no-op, so keep the raw filtering values.
    let mut vals: Vec<(f64, f64)> = (0..100)
        .map(|i| (1000.0 + i as f64, if i < 2 { -5.0 } else { 1.0 }))
        .collect();
    let s = best_range(&mut vals, 1, 50, true).expect("фильтр должен найтись");
    let from = s.from.unwrap();
    assert!(from > 1000.0, "граница обязана резать данные, got {from}");
}

#[test]
fn empty_variant_is_fact() {
    assert!(Variant::default().is_empty());
    let v = Variant {
        bounds: vec![Bound {
            field: "d1h".into(),
            from: Some(0.0),
            to: None,
        }],
        ..Default::default()
    };
    assert!(!v.is_empty());
}
