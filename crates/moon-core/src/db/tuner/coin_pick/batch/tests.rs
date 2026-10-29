//! Synthetic regressions for the batch auto-pick: grouping, per-strategy lists, cancellation.
use super::*;
use crate::db::tuner::coin_pick::coin_pick_from_rows;

/// Settings with a blacklist search and a thirty-percent fresh share.
fn settings() -> PickSettings {
    PickSettings {
        min_trades: 5,
        holdout_pct: 30,
        black: true,
        white: false,
    }
}

/// One hundred trades of one strategy: `XALPHA` loses in the fitted part, `XBETA` wins.
fn strategy_trades(fresh_alpha: f64, offset: i64) -> Vec<CoinTrade> {
    (0..100)
        .map(|i| CoinTrade {
            token: if i % 2 == 0 { "XALPHA" } else { "XBETA" }.into(),
            close: offset + i,
            profit: if i % 2 != 0 {
                2.0
            } else if i < 70 {
                -10.0
            } else {
                fresh_alpha
            },
        })
        .collect()
}

/// Interleave two strategies' trades the way one unsorted scan returns them.
fn interleaved(a: &[CoinTrade], b: &[CoinTrade]) -> Vec<KeyedTrade> {
    let mut out = Vec::new();
    for (x, y) in a.iter().zip(b.iter()) {
        out.push(KeyedTrade {
            key: "1@7".into(),
            trade: x.clone(),
        });
        out.push(KeyedTrade {
            key: "2@7".into(),
            trade: y.clone(),
        });
    }
    out
}

/// A fingerprint of everything the card and the column read from a pick.
fn shape(p: &CoinPick) -> String {
    format!("{p:?}")
}

/// Mixing two strategies' rows into one fit, or handing one strategy another's lists, would
/// show a verdict on the column that pressing "Pick" on that strategy never gives.
#[test]
fn each_strategy_gets_the_pick_a_single_strategy_run_gives() {
    let a = strategy_trades(-10.0, 0);
    let b = strategy_trades(100.0, 1_000);
    let mut lists = HashMap::new();
    lists.insert(
        "2@7".to_string(),
        ManualLists {
            black: HashSet::from(["XBETA".to_string()]),
            white: HashSet::new(),
        },
    );
    let progress = BatchProgress::default();
    let picks = coin_picks_from_rows(interleaved(&a, &b), &lists, settings(), &progress)
        .expect("not cancelled");
    assert_eq!(picks.len(), 2);
    let single_a = coin_pick_from_rows(&a, &settings().params_for(None));
    let single_b = coin_pick_from_rows(&b, &settings().params_for(lists.get("2@7")));
    assert_eq!(shape(&picks["1@7"]), shape(&single_a));
    assert_eq!(shape(&picks["2@7"]), shape(&single_b));
    // The two strategies really do differ, so a swap above could not pass by accident.
    assert_ne!(shape(&single_a), shape(&single_b));
    assert_eq!(progress.counts(), (2, 2));
}

/// A strategy's saved blacklist is its manual baseline: dropping it would score the pick
/// against "as traded" and could label a coin the strategy already excludes as a finding.
#[test]
fn a_strategy_without_lists_is_fitted_against_none() {
    let a = strategy_trades(-10.0, 0);
    let rows: Vec<KeyedTrade> = a
        .iter()
        .map(|t| KeyedTrade {
            key: "1@7".into(),
            trade: t.clone(),
        })
        .collect();
    let mut listed = HashMap::new();
    listed.insert(
        "1@7".to_string(),
        ManualLists {
            black: HashSet::from(["XALPHA".to_string()]),
            white: HashSet::new(),
        },
    );
    let free = coin_picks_from_rows(
        rows.clone(),
        &HashMap::new(),
        settings(),
        &BatchProgress::default(),
    )
    .expect("not cancelled");
    let banned = coin_picks_from_rows(rows, &listed, settings(), &BatchProgress::default())
        .expect("not cancelled");
    assert_eq!(
        free["1@7"]
            .black
            .iter()
            .map(|c| c.token.as_str())
            .collect::<Vec<_>>(),
        ["XALPHA"]
    );
    assert!(banned["1@7"].black.is_empty());
}

/// A superseded batch must stop instead of finishing a fit nobody will publish.
#[test]
fn a_cancelled_batch_stops_and_returns_nothing() {
    let progress = BatchProgress::default();
    progress.cancel();
    let rows = interleaved(&strategy_trades(-10.0, 0), &strategy_trades(-10.0, 0));
    assert!(coin_picks_from_rows(rows, &HashMap::new(), settings(), &progress).is_none());
    assert_eq!(progress.counts().0, 0);
}

/// The scan source every in-memory test reads through, like the tuner's own.
const SRC: &str = "(SELECT * FROM trades WHERE closedate >= ?1 AND closedate < ?2) o";

/// A report of two strategies on one core plus one row with a textual strategy id, and a
/// strategy database giving strategy 2 a saved blacklist of `XBETA`.
fn seeded(versions: bool) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory report");
    conn.execute_batch(
        "CREATE TABLE trades(strategyid, core_uid INTEGER, coin TEXT, closedate INTEGER,
                             pnl REAL);
         ATTACH DATABASE ':memory:' AS strat;
         CREATE TABLE strat.strategies(core_uid INTEGER, strategy_id INTEGER, name TEXT,
                                       deleted INTEGER, checked INTEGER);
         INSERT INTO strat.strategies VALUES (7, 1, 'UNIT_01', 0, 1), (7, 2, 'UNIT_02', 0, 1);",
    )
    .expect("schema");
    if versions {
        conn.execute_batch(
            "CREATE TABLE strat.strategy_versions(core_uid INTEGER, strategy_id INTEGER,
                                                  raw_json TEXT, valid_to INTEGER);
             INSERT INTO strat.strategy_versions VALUES
                 (7, 1, '{}', NULL),
                 (7, 2, '{\"CoinsBlackList\":\"XBETA\"}', NULL);",
        )
        .expect("versions");
    }
    let mut insert = conn
        .prepare("INSERT INTO trades VALUES (?1, 7, ?2, ?3, ?4)")
        .expect("insert");
    let a = strategy_trades(-10.0, 0);
    let b = strategy_trades(100.0, 1_000);
    for (id, trades) in [(1_i64, &a), (2, &b)] {
        for t in trades {
            insert
                .execute(rusqlite::params![id, t.token, t.close, t.profit])
                .expect("trade");
        }
    }
    insert
        .execute(rusqlite::params!["CORE_A", "XALPHA", 5, -1.0])
        .expect("textual id trade");
    drop(insert);
    conn
}

/// The period every in-memory test reads.
fn whole_period() -> Query {
    Query {
        from: 0,
        to: i64::MAX,
        ..Default::default()
    }
}

/// Through the real SQL and metadata path: each strategy's batch pick equals the pick of its
/// own trades read alone with its own saved lists, and a textual strategy id gets a pick
/// without failing the whole batch.
#[test]
fn the_database_batch_equals_a_single_strategy_read() {
    let conn = seeded(true);
    let q = whole_period();
    let (rows, lists) = read_keyed(&conn, &q, SRC).expect("batch read");
    assert_eq!(
        lists["2@7"].black,
        HashSet::from(["XBETA".to_string()]),
        "strategy 2's saved blacklist is its baseline"
    );
    let picks = coin_picks_from_rows(rows, &lists, settings(), &BatchProgress::default())
        .expect("not cancelled");
    assert!(picks.contains_key("CORE_A@7"), "a textual id keeps its row");
    for (id, key) in [(1, "1@7"), (2, "2@7")] {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT o.coin, o.closedate, o.pnl FROM {SRC} WHERE o.strategyid = ?3"
            ))
            .expect("single read");
        let single: Vec<CoinTrade> = stmt
            .query_map(rusqlite::params![q.from, q.to, id], |r| {
                Ok(CoinTrade {
                    token: r.get(0)?,
                    close: r.get(1)?,
                    profit: r.get(2)?,
                })
            })
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        let alone = coin_pick_from_rows(&single, &settings().params_for(lists.get(key)));
        assert_eq!(shape(&picks[key]), shape(&alone), "{key}");
    }
}

/// A strategy database that answers heads but not versions must fail the batch, not fit every
/// strategy against an empty baseline that would pass for its real lists.
#[test]
fn an_unreadable_list_baseline_fails_the_batch() {
    let conn = seeded(false);
    assert!(read_keyed(&conn, &whole_period(), SRC).is_err());
}
