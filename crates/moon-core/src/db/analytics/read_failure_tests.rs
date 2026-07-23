use super::super::test_support::{
    build_replica, corrupt_leaf_page, remove_db, spread_rows, temp_db,
};
use super::*;

/// Build the minimal real-trade query used by analytics regression tests.
fn q(from: i64, to: i64) -> Query {
    Query {
        from,
        to,
        cores: Vec::new(),
        side: SideFilter::All,
        emulator: Some(false),
        strategies: Vec::new(),
        attribute_liq: false,
        metric: Default::default(),
    }
}

/// A single-day period switches the series grid to HOURS and reports the profit split
/// by strategy type — and that split must reconcile with the period total, or the two
/// charts on screen contradict each other.
#[test]
fn single_day_uses_hourly_grid_and_kinds_reconcile() {
    let path = temp_db("oneday");
    let day = 1_780_000_000i64 / 86_400 * 86_400;
    // Four trades spread across three different hours of ONE day.
    let conn = build_replica(
        &path,
        &[
            (day + 3_600, 10.0, "BTCUSDT"),
            (day + 3_700, -4.0, "BTCUSDT"),
            (day + 7_200, 6.0, "ETHUSDT"),
            (day + 18_000, -2.0, "ETHUSDT"),
        ],
    );

    let s = summary_on(&conn, &q(day, day + 86_400), false).expect("healthy DB reads");
    assert_eq!(s.bucket_secs, 3_600, "a day or less → hourly grid");
    // The grid steps by exactly an hour with no holes (empty hours are zero-filled —
    // a chart's X axis has to be continuous), and three of them hold the trades.
    assert!(
        s.days.windows(2).all(|w| w[1].start - w[0].start == 3_600),
        "grid step must be one hour: {:?}",
        s.days
    );
    assert_eq!(
        s.days.iter().filter(|d| d.trades > 0).count(),
        3,
        "three hours with trades: {:?}",
        s.days
    );

    // Without the strategies DB every trade folds into ONE unnamed type — the chart
    // still has something to draw instead of vanishing.
    assert_eq!(s.kinds.len(), 1, "no strategies DB → a single group");
    assert_eq!(s.kinds[0].kind, "");

    // The split must add up to the period total, both in money and in count.
    let ksum: f64 = s.kinds.iter().map(|k| k.profit).sum();
    let kn: i64 = s.kinds.iter().map(|k| k.trades).sum();
    assert!(
        (ksum - s.cur.profit).abs() < 1e-9,
        "Σ of types {ksum} != period total {}",
        s.cur.profit
    );
    assert_eq!(kn, s.cur.n, "Σ of type trades != the period's n");

    // And inside a type, the per-core rows the popup lists must add up to its own bar.
    let csum: f64 = s.kinds[0].cores.iter().map(|c| c.profit).sum();
    assert!(
        (csum - s.kinds[0].profit).abs() < 1e-9,
        "Σ of cores {csum} != the type's profit {}",
        s.kinds[0].profit
    );

    // A longer period keeps the daily grid and computes no type split at all.
    let wide = summary_on(&conn, &q(day - 86_400, day + 86_400), false).expect("reads");
    assert_eq!(wide.bucket_secs, 86_400, "two days → daily grid");
    assert!(wide.kinds.is_empty(), "no type split on a long period");

    drop(conn);
    remove_db(&path);
}

/// A healthy replica retains exact summary metrics and empty-period semantics.
#[test]
fn healthy_summary_exact_values() {
    let path = temp_db("healthy");
    // Four same-day trades: +10, -4, +6, -2 => profit 10 and two wins.
    let day = 1_780_000_000i64 / 86_400 * 86_400 + 3_600;
    let conn = build_replica(
        &path,
        &[
            (day, 10.0, "BTCUSDT"),
            (day + 60, -4.0, "BTCUSDT"),
            (day + 120, 6.0, "ETHUSDT"),
            (day + 180, -2.0, "ETHUSDT"),
        ],
    );

    let s = summary_on(&conn, &q(day - 86_400, day + 86_400), false)
        .expect("здоровая БД должна читаться");
    assert_eq!(s.cur.n, 4);
    assert_eq!(s.cur.wins, 2);
    assert_eq!(s.cur.losses, 2);
    assert!(
        (s.cur.profit - 10.0).abs() < 1e-9,
        "profit={}",
        s.cur.profit
    );
    assert!((s.cur.winrate() - 50.0).abs() < 1e-9);
    // Profit factor is total wins divided by total losses: 16 / 6.
    assert!((s.cur.pf - 16.0 / 6.0).abs() < 1e-9, "pf={}", s.cur.pf);
    // Cumulative profit 10 -> 6 -> 12 -> 10 has a maximum drawdown of 4.
    assert!((s.cur.max_dd - 4.0).abs() < 1e-9, "max_dd={}", s.cur.max_dd);
    assert!((s.cur.avg - 2.5).abs() < 1e-9);
    assert_eq!(s.coins.len(), 2, "две монеты");
    assert_eq!(s.cores, vec![(1u64, "CORE-A".to_string())]);
    assert_eq!(s.best.len(), 4);

    // A genuinely empty period succeeds with zero counters.
    let empty = summary_on(&conn, &q(day - 10 * 86_400, day - 9 * 86_400), false)
        .expect("пустой период — успешное чтение");
    assert_eq!(empty.cur.n, 0);

    drop(conn);
    remove_db(&path);
}

/// Index-page corruption surfaces as an error rather than an empty period.
#[test]
fn corrupt_replica_surfaces_error_not_empty() {
    let path = temp_db("corrupt");
    // Enough rows keep the target index leaf away from the header page so
    // corruption surfaces during the period scan rather than file opening.
    let day = 1_780_000_000i64 / 86_400 * 86_400;
    let conn = build_replica(&path, &spread_rows(day, 2000));

    // Prove the fixture is healthy before introducing damage.
    let before = summary_on(&conn, &q(day - 86_400, day + 10 * 86_400), false)
        .expect("до порчи БД читается");
    assert_eq!(before.cur.n, 2000);

    // The scan must use the index whose leaf page is about to be damaged.
    let plan: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT closedate FROM orders_rep
             WHERE closedate >= 1 AND closedate < 2 AND closedate > 0",
            [],
            |r| r.get(3),
        )
        .unwrap();
    assert!(
        plan.contains("idx_rep_closedate"),
        "план без индекса: {plan}"
    );

    corrupt_leaf_page(conn, &path, "idx_rep_closedate");

    // The intact header allows opening; the period read reaches the damage.
    let conn = Connection::open(&path).expect("битая БД всё ещё открывается");
    let wide = q(day - 86_400, day + 10 * 86_400);

    // Pin the period scan itself so another query cannot mask skipped rows
    // by failing later in the summary pipeline.
    let src = unified_from(&conn, &wide)
        .expect("схема читается")
        .expect("источник есть");
    assert!(
        scan_period(&conn, &src, wide.from, wide.to, 86_400).is_err(),
        "скан периода обязан вернуть ошибку, а не усечённую статистику"
    );

    let res = summary_on(&conn, &wide, false);

    assert!(
        !matches!(res, Ok(_)),
        "ошибка чтения не должна превращаться в успешный — в том числе \
         пустой или частичный — период: это и есть чинимый баг"
    );
    match res {
        Err(ReadFail::Failed { kind, .. }) => assert_eq!(
            kind,
            super::super::FailKind::Corrupt,
            "порча должна классифицироваться как Corrupt"
        ),
        Err(ReadFail::NotReady) => {
            panic!("порча не должна выглядеть как «реплика не готова»")
        }
        Ok(_) => unreachable!("уже проверено выше"),
    }

    remove_db(&path);
}
