//! Settled misses carried across reloads, and stage C's byte-bounded replay.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::super::{
    Fetched, HeldTape, Misses, TapeBudget, miss_holds, missing_still_holds, replay_chunked,
    replay_fetched, short_allowed, unservable_status,
};
use crate::analytics::tuner::ticks::signature::SamplePolicy;
use crate::analytics::tuner::ticks::state::{DealRow, RowAddress, SettledMiss, TapeStatus};
use crate::analytics::tuner::ticks::tail::Admission;
use moon_core::db::tuner::ticks::{Deal, model_window};
use moon_core::feed::types::{Side, Tick};
use moon_core::market::trade_replay::Coverage;

fn synthetic_deal(report_uid: i64) -> Deal {
    Deal {
        report_uid,
        core_uid: 1,
        core_name: "CORE_X".into(),
        strategy_id: 1,
        kind: "Spread".into(),
        coin: "XALPHA".into(),
        buy_ms: 100_000,
        close_ms: 110_000,
        buy_price: 1.0,
        sell_price: 1.01,
        spent: 100.0,
        is_short: false,
        sell_reason: String::new(),
        fact_pnl: 0.0,
        sizing: None,
        pnl_pct: false,
        profit: None,
        deltas: Default::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        fact_modifier: None,
        hook_depth_pct: None,
        drops_detect: None,
        hook_stated_take_pct: None,
        strike_detect: None,
        step_lag_ms: 0.0,
        round_trip_ms: None,
        stop_anchor: None,
        delta_track: None,
        bars: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
        gap: None,
    }
}

fn unlisted_address() -> Arc<RowAddress> {
    Arc::new(RowAddress {
        core_uid: 1,
        venue: None,
        exchange_key: moon_core::fixture::unlisted::EXCHANGE_KEY.into(),
        market: "XALPHA".into(),
        btc_market: None,
    })
}

fn blank(report_uid: i64) -> DealRow {
    DealRow {
        deal: synthetic_deal(report_uid),
        tape: TapeStatus::Missing,
        verdict: None,
        address: Some(unlisted_address()),
        ticks: None,
        entry_line: None,
        held: None,
    }
}

/// A held answer: prints every second and coverage over `[from_s, to_s]`, the price stepping
/// at the close.
fn held(from_s: i64, to_s: i64) -> HeldTape {
    let window = model_window(&synthetic_deal(1), 30_000, i64::MAX).expect("window");
    let ticks = (from_s..=to_s)
        .map(|second| Tick {
            time_ms: second as f64 * 1_000.0,
            price: if second < 110 { 1.0 } else { 1.01 },
            qty: 1.0,
            side: Side::Buy,
        })
        .collect();
    (ticks, Coverage::one((from_s * 1_000, to_s * 1_000)), window)
}

/// What a chunk of the synthetic table holds: row 1 fully held, row 2 held with a hole before
/// the buy off a settled read, row 3 the same hole off a failed read, row 4 never answered.
fn synthetic_held(chunk: &[DealRow]) -> (HashMap<i64, HeldTape>, HashSet<i64>) {
    let mut tapes = HashMap::new();
    let mut settled = HashSet::new();
    for row in chunk {
        let uid = row.deal.report_uid;
        match uid {
            1 => {
                tapes.insert(uid, held(70, 180));
                settled.insert(uid);
            }
            2 => {
                tapes.insert(uid, held(95, 180));
                settled.insert(uid);
            }
            3 => {
                tapes.insert(uid, held(95, 180));
            }
            _ => {}
        }
    }
    (tapes, settled)
}

fn chunked(budget: TapeBudget) -> (Vec<DealRow>, Misses) {
    replay_chunked(
        (1..=4).map(blank).collect(),
        Default::default(),
        budget,
        synthetic_held,
        &Default::default(),
        Default::default(),
        None,
        3_710_000,
        None,
    )
}

fn sample() -> SamplePolicy {
    SamplePolicy {
        min_tail_s: 60,
        switches: (false, true),
        margin_ms: 30_000,
        long_position_ms: i64::MAX,
    }
}

fn admission(now_ms: i64) -> Admission {
    let mut admission = Admission::at(60, now_ms);
    admission.switches = (false, true);
    admission
}

/// A settled miss is noted and, unchanged, carried; a failed read is not noted, so the reload
/// asks again; a row never answered is neither.
#[test]
fn settled_miss_is_carried_failed_read_is_not() {
    let (rows, misses) = chunked(TapeBudget {
        step: 256,
        max_bytes: usize::MAX,
    });
    assert_eq!(rows[0].tape, TapeStatus::Covered);
    assert_eq!(rows[1].tape, TapeStatus::Missing);
    assert_eq!(rows[2].tape, TapeStatus::Missing);
    assert_eq!(rows[3].tape, TapeStatus::Missing);
    assert_eq!(
        misses.keys().copied().collect::<HashSet<_>>(),
        HashSet::from([2]),
        "only the settled read's miss is noted"
    );

    let (held, short_allowed, market) = misses[&2].clone();
    let miss = SettledMiss {
        held,
        short_allowed,
        model: Default::default(),
        sample: sample(),
        refusal_revision: 0,
        market,
    };
    let address = unlisted_address();
    let deal = synthetic_deal(2);
    let admission = admission(3_710_000);
    assert!(
        miss_holds(&rows[1], &miss, &deal, &address, &admission),
        "an unchanged settled miss is carried"
    );
    let mut corrected = deal.clone();
    corrected.sell_price = 1.02;
    assert!(!miss_holds(
        &rows[1], &miss, &corrected, &address, &admission
    ));
    let mut moved = (*address).clone();
    moved.market = "XBETA".into();
    assert!(!miss_holds(&rows[1], &miss, &deal, &moved, &admission));
}

/// A short tail the core archive can still fill waits as missing; once its window passes, the
/// carried miss no longer holds, and the replay admits the row as a short tail.
#[test]
fn fillable_miss_is_replayed_once_its_window_passes() {
    let deal = synthetic_deal(1);
    let address = unlisted_address();
    let window = model_window(&deal, 30_000, i64::MAX).expect("window");
    let trail = Some((30_000, 0));
    let recent = deal.close_ms + 3_600_000;
    let gone = deal.close_ms + moon_core::market::CORE_ARCHIVE_RETENTION_MS + 3_600_000;
    let short_then = short_allowed(
        &address,
        deal.report_uid,
        window.close_ms,
        trail,
        60,
        recent,
        (false, true),
    );
    assert!(!short_then, "the core archive can still fill the tail");
    let miss = SettledMiss {
        held: trail,
        short_allowed: short_then,
        model: Default::default(),
        sample: sample(),
        refusal_revision: 0,
        market: (address.exchange_key.clone(), address.market.clone(), None),
    };
    let mut row = blank(1);
    row.held = trail;
    assert!(miss_holds(&row, &miss, &deal, &address, &admission(recent)));
    assert!(
        !miss_holds(&row, &miss, &deal, &address, &admission(gone)),
        "the clock made the tail unfillable"
    );

    // Replayed now — long past the archive — the same held tape is a short tail.
    let fetched = Fetched {
        rows: vec![row],
        tapes: [(1, held(70, 110))].into(),
        traces: Default::default(),
        settled: [1].into(),
    };
    let rows = replay_fetched(
        fetched,
        &Default::default(),
        Default::default(),
        None,
        gone,
        None,
        None,
    );
    assert_eq!(rows[0].tape, TapeStatus::Covered);
    assert_eq!(rows[0].held, Some((30_000, 0)));
}

/// The chunked replay gives the rows and misses an unbounded one gives, chunk by chunk.
#[test]
fn tiny_budget_replays_like_unbounded() {
    type Summary = (
        Vec<(i64, TapeStatus, Option<(i64, i64)>, String, bool)>,
        Vec<(
            i64,
            (
                Option<(i64, i64)>,
                bool,
                (String, String, Option<moon_core::venue::Venue>),
            ),
        )>,
    );
    let summary = |(rows, misses): (Vec<DealRow>, Misses)| -> Summary {
        let rows = rows
            .iter()
            .map(|r| {
                (
                    r.deal.report_uid,
                    r.tape,
                    r.held,
                    format!("{:?}", r.verdict),
                    r.ticks.is_some(),
                )
            })
            .collect();
        let mut misses: Vec<_> = misses.into_iter().collect();
        misses.sort_by_key(|m| m.0);
        (rows, misses)
    };
    let unbounded = summary(chunked(TapeBudget {
        step: usize::MAX,
        max_bytes: usize::MAX,
    }));
    let tiny = summary(chunked(TapeBudget {
        step: 1,
        max_bytes: 1,
    }));
    assert_eq!(unbounded, tiny);
    assert_eq!(unbounded.0.len(), 4);
}

/// The fast path asks every settled miss again at the reload's clock: one whose short tail the
/// clock made unfillable, or whose window fell out of retention, no longer holds — and the
/// full reload then refuses the latter.
#[test]
fn settled_miss_stops_vouching_once_the_clock_moves_it() {
    let deal = synthetic_deal(1);
    let address = unlisted_address();
    let trail = Some((30_000, 0));
    let recent = deal.close_ms + 3_600_000;
    let gone = deal.close_ms + moon_core::market::CORE_ARCHIVE_RETENTION_MS + 3_600_000;
    let mut row = blank(1);
    row.held = trail;
    let settled = HashMap::from([(
        1,
        SettledMiss {
            held: trail,
            short_allowed: false,
            model: Default::default(),
            sample: sample(),
            refusal_revision: 0,
            market: (address.exchange_key.clone(), address.market.clone(), None),
        },
    )]);
    assert!(missing_still_holds(&row, &settled, &admission(recent)));
    assert!(
        !missing_still_holds(&row, &settled, &admission(gone)),
        "the clock moved the row"
    );
    assert!(
        !missing_still_holds(&row, &HashMap::new(), &admission(recent)),
        "a miss nobody settled is not vouched for"
    );
    assert_eq!(unservable_status(&address, &deal, recent), None);
    assert!(matches!(
        unservable_status(&address, &deal, gone),
        Some(TapeStatus::Refused(_))
    ));
    let mut unaddressed = row.clone();
    unaddressed.address = None;
    assert!(missing_still_holds(
        &unaddressed,
        &HashMap::new(),
        &admission(gone)
    ));
}
