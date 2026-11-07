//! The merge of two observers' chains: order by content, never by clock, and nothing of this
//! store's history lost.

use serde_json::json;

use super::super::write::{apply_full_set, forget, forget_core, init};
use super::*;
use crate::config::storage::StrategiesStoreCfg;
use crate::station_api::{StrategyHead, StrategyVersion};
use crate::strat_db::StratDump;

const CORE: u64 = 7;
const STRATEGY: i64 = 42;

/// A chain of plan links, one per `(time, content)`.
fn links(chain: &[(i64, u32)]) -> Vec<Link> {
    chain
        .iter()
        .map(|&(time, content)| Link { time, content })
        .collect()
}

/// The contents of a plan, in order.
fn contents(planned: &[Planned]) -> Vec<u32> {
    planned.iter().map(|row| row.content).collect()
}

/// Regression target: comparing by clock instead of aligning by content. With one observer's clock
/// two hours off, a time-based rule doubled every version it had not seen overlap.
#[test]
fn a_shifted_clock_invents_no_version() {
    const HOURS_2: i64 = 2 * 3_600_000;
    let station = links(&[(1000, 1), (2000, 2), (3000, 3), (4000, 4)]);
    let terminal = links(&[
        (1000 + HOURS_2, 1),
        (2000 + HOURS_2, 2),
        (3000 + HOURS_2, 3),
        (4000 + HOURS_2, 4),
    ]);
    let merged = plan(&station, &terminal);
    assert_eq!(contents(&merged), [1, 2, 3, 4]);
    assert_eq!(merged[0].valid_from, 1000, "the earlier observation stands");
}

/// A terminal that was offline saw only the state at its reconnect; the station saw what led there.
#[test]
fn what_one_observer_missed_is_filled_in_order() {
    let station = links(&[(100, 1), (200, 2), (300, 3), (400, 4)]);
    let terminal = links(&[(100, 1), (450, 4)]);
    let merged = plan(&station, &terminal);
    assert_eq!(contents(&merged), [1, 2, 3, 4]);
    assert_eq!(merged[3].valid_from, 400, "its late reconnect does not win");
}

/// A return to an earlier state is real history (2699 of them on a real archive): matching by
/// content alone, without order, would fold A-B-A into one row or tie them crosswise.
#[test]
fn a_return_to_an_earlier_state_stays_a_version() {
    let station = links(&[(100, 1), (200, 2), (300, 1)]);
    let terminal = links(&[(110, 1), (210, 2), (310, 1)]);
    assert_eq!(contents(&plan(&station, &terminal)), [1, 2, 1]);
    // One observer missed the return; the merged history still holds it.
    let early = links(&[(110, 1), (210, 2)]);
    assert_eq!(contents(&plan(&station, &early)), [1, 2, 1]);
}

/// Both sides of an exchange must arrive at one history, or the summaries never settle.
#[test]
fn merging_again_changes_nothing() {
    let station = links(&[(100, 1), (200, 2), (300, 3)]);
    let terminal = links(&[(150, 1), (250, 5), (350, 3)]);
    let merged: Vec<Link> = plan(&station, &terminal)
        .iter()
        .map(|row| Link {
            time: row.valid_from,
            content: row.content,
        })
        .collect();
    let again = |first: &[Link], second: &[Link]| -> Vec<(i64, u32)> {
        plan(first, second)
            .iter()
            .map(|row| (row.valid_from, row.content))
            .collect()
    };
    let expected: Vec<(i64, u32)> = merged.iter().map(|l| (l.time, l.content)).collect();
    assert_eq!(again(&merged, &terminal), expected, "the terminal's view");
    assert_eq!(again(&station, &merged), expected, "the station's view");
}

/// A row only one side holds, with a time from a clock that disagrees, must not move aligned rows.
#[test]
fn a_time_outside_its_gap_takes_the_next_free_moment() {
    let station = links(&[(100, 1), (200, 3)]);
    // Content 2 sits between 1 and 3, stamped by a clock an hour ahead.
    let terminal = links(&[(100, 1), (3_600_100, 2), (3_600_200, 3)]);
    let merged = plan(&station, &terminal);
    assert_eq!(contents(&merged), [1, 2, 3]);
    assert_eq!(merged[1].valid_from, 101);
    assert_eq!(merged[2].valid_from, 200, "the aligned row keeps its time");
}

/// A stretch with nothing before it and a time past the first aligned row must land before it.
#[test]
fn a_leading_row_past_its_gap_lands_before_the_aligned_row() {
    let station = links(&[(1000, 2)]);
    let terminal = links(&[(5000, 1), (5001, 2)]);
    let merged = plan(&station, &terminal);
    assert_eq!(contents(&merged), [1, 2]);
    assert!(merged[0].valid_from < merged[1].valid_from);
    assert_eq!(merged[1].valid_from, 1000);
}

/// One side empty: the other's chain is the history, times untouched.
#[test]
fn an_empty_side_takes_the_chain_as_it_is() {
    let station = links(&[(100, 1), (200, 2)]);
    let merged = plan(&station, &[]);
    assert_eq!(contents(&merged), [1, 2]);
    assert_eq!(merged[1].valid_from, 200);
    assert_eq!(contents(&plan(&[], &station)), [1, 2]);
}

/// Neighbours holding one content are one version, whichever chains they came from.
#[test]
fn neighbours_with_one_content_fold() {
    let station = links(&[(100, 1), (200, 2)]);
    let terminal = links(&[(150, 2), (250, 2)]);
    assert_eq!(contents(&plan(&station, &terminal)), [1, 2]);
}

fn cfg() -> StrategiesStoreCfg {
    StrategiesStoreCfg {
        ignore_fields: vec!["Comment".into()],
    }
}

fn setup() -> (Connection, State) {
    let conn = Connection::open_in_memory().unwrap();
    let state = init(&conn, &cfg()).unwrap();
    (conn, state)
}

fn head(name: &str) -> StrategyHead {
    StrategyHead {
        name: name.into(),
        kind: "Drops".into(),
        kind_ordinal: 2,
        folder_path: String::new(),
        is_short: false,
        checked: true,
        server_ver: 1,
        server_ms: 0,
        deleted: false,
        core_name: "elsewhere".into(),
    }
}

fn version(valid_from: i64, take_profit: i64) -> StrategyVersion {
    StrategyVersion {
        valid_from,
        valid_to: None,
        change_kind: "params".into(),
        origin: Some("local".into()),
        ver_gap: false,
        server_ver: 12,
        server_ms: valid_from,
        checked_at: 1,
        raw_json: json!({"TakeProfit": take_profit, "Comment": "x"}).to_string(),
    }
}

fn incoming(versions: Vec<StrategyVersion>, from_station: bool) -> IncomingChain {
    IncomingChain {
        core_uid: CORE,
        core_name: "here".into(),
        strategy_id: STRATEGY,
        head: Some(head("Main")),
        versions,
        from_station,
    }
}

fn dump(take_profit: i64) -> StratDump {
    let mut fields = Map::new();
    fields.insert("TakeProfit".into(), Value::from(take_profit));
    fields.insert("Comment".into(), Value::from("x"));
    StratDump {
        strategy_id: STRATEGY,
        name: "Main".into(),
        kind: "Drops".into(),
        kind_ordinal: 2,
        folder_path: String::new(),
        is_short: false,
        checked: true,
        server_ver: 12,
        server_ms: 1,
        fields,
        local_edit: false,
    }
}

/// `(valid_from, valid_to, change_kind, n_changed, TakeProfit)` of the stored chain.
fn chain(conn: &Connection) -> Vec<(i64, Option<i64>, String, i64, i64)> {
    let mut statement = conn
        .prepare(
            "SELECT valid_from, valid_to, change_kind, n_changed, raw_json
             FROM strategy_versions WHERE core_uid=?1 AND strategy_id=?2 ORDER BY valid_from",
        )
        .unwrap();
    statement
        .query_map(params![CORE as i64, STRATEGY], |row| {
            let raw: String = row.get(4)?;
            let value: Value = serde_json::from_str(&raw).unwrap();
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                value["TakeProfit"].as_i64().unwrap(),
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// A new terminal: the whole chain arrives, with a head, as a chain that holds together.
#[test]
fn an_empty_store_takes_the_chain_and_its_head() {
    let (conn, mut state) = setup();
    let outcome = merge_chain(
        &conn,
        &mut state,
        incoming(
            vec![version(100, 1), version(200, 2), version(300, 3)],
            true,
        ),
    )
    .unwrap();
    assert!(outcome.changed);
    assert!(!outcome.other_lacks, "the sender has all of it");
    let stored = chain(&conn);
    assert_eq!(stored.len(), 3);
    assert_eq!(stored[0].2, "created", "the history's first row");
    assert_eq!(stored[1].1, Some(300), "closed at its successor");
    assert_eq!(stored[2].1, None, "the newest is open");
    assert_eq!(stored[2].3, 1, "its diff was rebuilt against this chain");
    let name: String = conn
        .query_row("SELECT core_name FROM strategies", [], |row| row.get(0))
        .unwrap();
    assert_eq!(name, "here", "the core's name here, not the sender's");
}

/// Regression target: not teaching the writer's head mirror about a merged head. The core's next
/// set would then read the strategy as new and open its history at `created` again.
#[test]
fn the_core_set_after_a_merge_adds_no_version() {
    let (conn, mut state) = setup();
    merge_chain(
        &conn,
        &mut state,
        incoming(vec![version(100, 1), version(200, 2)], true),
    )
    .unwrap();
    apply_full_set(&conn, &mut state, CORE, "here", true, &[dump(2)]).unwrap();
    assert_eq!(
        chain(&conn).len(),
        2,
        "the same content is the same version"
    );
    apply_full_set(&conn, &mut state, CORE, "here", false, &[dump(3)]).unwrap();
    let stored = chain(&conn);
    assert_eq!(stored.len(), 3, "a real edit still is one");
    assert_eq!(stored[2].2, "params");
}

/// A terminal whose feed got there first holds a `created` row for what the station has a history
/// of; that row is the same version as the station's newest, not a second creation.
#[test]
fn a_first_sighting_folds_into_the_history_it_belongs_to() {
    let (conn, mut state) = setup();
    apply_full_set(&conn, &mut state, CORE, "here", true, &[dump(2)]).unwrap();
    merge_chain(
        &conn,
        &mut state,
        incoming(vec![version(100, 1), version(200, 2)], true),
    )
    .unwrap();
    let stored = chain(&conn);
    assert_eq!(stored.iter().map(|r| r.4).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(stored[1].0, 200, "the station saw it first");
    assert_eq!(stored[1].2, "params");
}

/// The promise to a terminal's owner: merging adds, and what was stored stays stored.
#[test]
fn nothing_of_this_store_is_lost() {
    let (conn, mut state) = setup();
    apply_full_set(&conn, &mut state, CORE, "here", true, &[dump(1)]).unwrap();
    apply_full_set(&conn, &mut state, CORE, "here", false, &[dump(2)]).unwrap();
    let before: Vec<i64> = chain(&conn).iter().map(|row| row.4).collect();
    let outcome = merge_chain(&conn, &mut state, incoming(vec![version(100, 9)], true)).unwrap();
    assert!(
        outcome.other_lacks,
        "the sender lacks what only this store has"
    );
    let after: Vec<i64> = chain(&conn).iter().map(|row| row.4).collect();
    for take_profit in before {
        assert!(after.contains(&take_profit), "{take_profit} was lost");
    }
    assert!(after.contains(&9), "and the new one arrived");
}

/// The second run of an exchange finds nothing to do: this is what lets the summaries settle.
#[test]
fn merging_the_same_chain_again_writes_nothing() {
    let (conn, mut state) = setup();
    let versions = vec![version(100, 1), version(200, 2)];
    merge_chain(&conn, &mut state, incoming(versions.clone(), true)).unwrap();
    let outcome = merge_chain(&conn, &mut state, incoming(versions, true)).unwrap();
    assert_eq!(
        outcome,
        MergeOutcome {
            changed: false,
            other_lacks: false
        }
    );
}

/// A row seen under another observer's `local` was not edited through this one.
#[test]
fn another_observers_local_is_external_here() {
    let (conn, mut state) = setup();
    merge_chain(&conn, &mut state, incoming(vec![version(100, 1)], true)).unwrap();
    let origin: String = conn
        .query_row("SELECT origin FROM strategy_versions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(origin, "external");
}

/// Regression target: dropping the tombstone. The next pass would restore history the user was
/// told could not be restored.
#[test]
fn what_was_forgotten_here_does_not_come_back() {
    let (conn, mut state) = setup();
    apply_full_set(&conn, &mut state, CORE, "here", true, &[dump(1)]).unwrap();
    forget_core(&conn, &mut state, CORE).unwrap();
    let outcome = merge_chain(
        &conn,
        &mut state,
        incoming(vec![version(100, 1), version(200, 2)], true),
    )
    .unwrap();
    assert_eq!(outcome, MergeOutcome::default());
    assert!(chain(&conn).is_empty());
    // What was recorded after the forgetting is ordinary history, and it does arrive.
    let later = crate::util::now_unix_ms_i64() + 60_000;
    merge_chain(&conn, &mut state, incoming(vec![version(later, 5)], true)).unwrap();
    assert_eq!(chain(&conn).len(), 1);
}

/// The narrower purge of one deleted strategy leaves the same mark.
#[test]
fn a_forgotten_strategy_does_not_come_back() {
    let (conn, mut state) = setup();
    apply_full_set(&conn, &mut state, CORE, "here", true, &[dump(1)]).unwrap();
    // Gone from the core's set, which is what allows the purge at all.
    let mut other = dump(1);
    other.strategy_id = STRATEGY + 1;
    apply_full_set(&conn, &mut state, CORE, "here", false, &[other]).unwrap();
    forget(&conn, &mut state, CORE, &[STRATEGY]).unwrap();
    merge_chain(&conn, &mut state, incoming(vec![version(100, 1)], true)).unwrap();
    assert!(chain(&conn).is_empty());
}

/// Whatever cannot be a chain stays out of the store, whole.
#[test]
fn a_broken_chain_is_refused() {
    let (conn, mut state) = setup();
    let falling = incoming(vec![version(200, 1), version(100, 2)], true);
    assert!(merge_chain(&conn, &mut state, falling).is_err());
    let mut headless = incoming(vec![version(100, 1)], true);
    headless.head = None;
    assert!(merge_chain(&conn, &mut state, headless).is_err());
    assert!(chain(&conn).is_empty());
}

/// Regression target: stamping a merged history's first row `created` whenever it is not stored
/// row 0. Retention trims the oldest rows, so at the bound the first row is a later stored one, and
/// its history would be rewritten as a creation with no diff on every merge.
#[test]
fn a_row_first_after_retention_keeps_what_it_says() {
    let (_conn, state) = setup();
    let stored = |valid_from: i64, take_profit: i64| Row {
        valid_from,
        valid_to: None,
        change_kind: "params".into(),
        origin: None,
        n_changed: 3,
        ver_gap: false,
        server_ver: 12,
        server_ms: 0,
        checked_at: 0,
        raw_json: json!({"TakeProfit": take_profit}).to_string(),
        changed_json: Some("{\"kept\":true}".into()),
    };
    let local = [stored(100, 1), stored(200, 2)];
    let trimmed = [Planned {
        first: Some(1),
        second: None,
        valid_from: 200,
        content: 2,
    }];
    let rows = build_rows(&trimmed, |row| row.first, &local, |_| None, &state).unwrap();
    assert_eq!(rows[0].change_kind, "params");
    assert_eq!(rows[0].n_changed, 3);
    assert_eq!(rows[0].changed_json.as_deref(), Some("{\"kept\":true}"));
}

/// A deleted strategy's newest row, when a later one arrived, must not close before it starts: the
/// attribution of a trade reads these windows.
#[test]
fn a_deleted_strategy_never_closes_before_its_newest_row_starts() {
    let row = |valid_from: i64| Row {
        valid_from,
        valid_to: None,
        change_kind: "params".into(),
        origin: None,
        n_changed: 0,
        ver_gap: false,
        server_ver: 0,
        server_ms: 0,
        checked_at: 0,
        raw_json: "{}".into(),
        changed_json: None,
    };
    let mut rows = [row(100), row(300)];
    close_ranges(&mut rows, Some(200), Some(true));
    assert_eq!(rows[0].valid_to, Some(300));
    assert_eq!(rows[1].valid_to, Some(300), "not the older 200");
    let mut rows = [row(100), row(150)];
    close_ranges(&mut rows, Some(200), Some(true));
    assert_eq!(rows[1].valid_to, Some(200), "the moment it went stands");
    let mut rows = [row(100)];
    close_ranges(&mut rows, Some(50), Some(false));
    assert_eq!(rows[0].valid_to, None, "a live strategy's newest is open");
}

/// A stamp from far ahead would drag every later version of the strategy here into the future.
#[test]
fn a_version_stamped_far_ahead_is_refused() {
    let (conn, mut state) = setup();
    let far = crate::util::now_unix_ms_i64() + 2 * MAX_CLOCK_LEAD_MS;
    assert!(merge_chain(&conn, &mut state, incoming(vec![version(far, 1)], true)).is_err());
    assert!(chain(&conn).is_empty());
    // A wrong clock within reason is still history.
    let ahead = crate::util::now_unix_ms_i64() + 3_600_000;
    assert!(merge_chain(&conn, &mut state, incoming(vec![version(ahead, 1)], true)).is_ok());
}

/// Moving a version's `valid_from` must not leave statistics keyed by the old one.
#[test]
fn the_statistics_cache_of_a_merged_chain_goes() {
    let (conn, mut state) = setup();
    apply_full_set(&conn, &mut state, CORE, "here", true, &[dump(1)]).unwrap();
    conn.execute_batch(
        "CREATE TABLE version_stats (core_uid INTEGER, strategy_id INTEGER, valid_from INTEGER);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO version_stats VALUES (?1, ?2, 1)",
        params![CORE as i64, STRATEGY],
    )
    .unwrap();
    merge_chain(&conn, &mut state, incoming(vec![version(100, 1)], true)).unwrap();
    let left: i64 = conn
        .query_row("SELECT COUNT(*) FROM version_stats", [], |row| row.get(0))
        .unwrap();
    assert_eq!(left, 0);
}
