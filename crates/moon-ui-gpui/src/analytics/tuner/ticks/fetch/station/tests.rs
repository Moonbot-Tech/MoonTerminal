//! What the autoload asks the station for.

use std::sync::{Arc, Mutex};

use super::{Filled, ReplayWindow, Unfillable, lacking, note_filed, prefill_rows};

/// Serializes tests that read or write the process-wide unfillable set.
static UNFILLABLE_TEST: Mutex<()> = Mutex::new(());

/// Keeping a refusal after a window edit permanently skips the newly requested margins.
#[test]
fn window_growth_invalidates_station_refusals() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let _refusals = UNFILLABLE_TEST.lock().unwrap_or_else(|p| p.into_inner());
    moon_core::market::trade_replay::set_margin_s(180);
    super::unfillable().refuse(1, -94_001, "synthetic", "XALPHA");
    moon_core::market::trade_replay::set_margin_s(600);
    let retry = super::candidate(1, -94_001);
    forget(-94_001);
    assert!(retry, "the wider window must get a fresh station attempt");
}

/// The station's hourly catch-up completes a refused trade's tape: the trade then counts in
/// `tape_trades`, never in `tail_final`, so a refusal keyed on the final-tail count alone is
/// never dropped and the row reads "not on the station" until the terminal is quit (#1070).
#[test]
fn a_catchup_filing_reopens_a_refused_row() {
    // The two station counts as two status reads report them: the finalised tails unchanged,
    // one more trade with complete tape. The other inputs of the generation stay as they are.
    // A store of its own, not the process-wide one: the counts are a global other tests write.
    let generation = |tape_trades: u64| ((0, tape_trades), 60, (600_000, 0), 0);
    let mut refused = Unfillable::default();
    refused.refresh(generation(10));
    refused.refuse(1, 42, "EX-XX", "XALPHA");
    refused.refresh(generation(10));
    assert!(
        refused.uids.contains_key(&(1, 42)),
        "an unchanged station keeps its refusal"
    );
    let revision = refused.revision;
    refused.refresh(generation(11));
    assert!(
        !refused.uids.contains_key(&(1, 42)),
        "the station filed this trade's tape, yet the row is still refused"
    );
    assert_ne!(
        refused.revision, revision,
        "the lift is what wakes an open table"
    );
}

/// A row answered short while its tail could still arrive is asked about once, not on every
/// pass: asked every 30 s it brought a few more seconds of tape each time, and each filing
/// reloaded the open table and stopped the user's search (the regression of the first cut of
/// #1070). It waits out the tail, stays a candidate throughout — never "not on the station" —
/// and is asked about again once the tail is final.
#[test]
fn a_row_waiting_for_its_tail_is_not_asked_again_before_it_settles() {
    let mut rows = Unfillable::default();
    rows.deferred.insert((1, 42), 5_000);
    assert!(rows.waiting_tail(1, 42, 4_999));
    assert!(
        !rows.uids.contains_key(&(1, 42)),
        "waiting is not a refusal"
    );
    assert!(!rows.waiting_tail(1, 42, 5_000), "the tail is final: ask");
    assert!(rows.deferred.is_empty(), "the wait is over for good");
    assert!(
        !rows.waiting_tail(2, 42, 0),
        "another core's row never waited"
    );
    // A status read drops the refusals, not the waits: the tail is final when it is final,
    // whatever the station counts, and a dropped wait is a row asked on every pass.
    rows.refresh(((0, 0), 60, (600_000, 0), 1));
    rows.deferred.insert((1, 43), 9_000);
    rows.refuse(1, 44, "synthetic", "XALPHA");
    rows.refresh(((0, 1), 60, (600_000, 0), 1));
    assert!(rows.uids.is_empty(), "the refusal is lifted");
    assert!(rows.waiting_tail(1, 43, 0), "the wait stands");
    // An axis move shifts the row's close by its core's offset, a changed window or floor moves
    // the tail: each makes the stored moment another row's, so the wait goes.
    for moved in [
        ((0, 1), 60, (600_000, 0), 2),
        ((0, 1), 60, (900_000, 0), 2),
        ((0, 1), 120, (900_000, 0), 2),
    ] {
        rows.deferred.insert((1, 43), 9_000);
        rows.refresh(moved);
        assert!(!rows.waiting_tail(1, 43, 0), "{moved:?}");
    }
    // A row that left the sample takes its wait with it.
    rows.deferred.insert((1, 43), 9_000);
    rows.retain(
        &std::collections::HashSet::new(),
        &std::collections::HashSet::new(),
    );
    assert!(!rows.waiting_tail(1, 43, 0));
}

/// The answer that defers a row: a station asked seconds after the close answers short, and
/// the row is neither refused nor left to be asked about on the next pass.
#[test]
fn a_short_answer_right_after_the_close_defers_the_row() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let _guard = UNFILLABLE_TEST.lock().unwrap_or_else(|p| p.into_inner());
    let mut queued = queued_unservable(-94_073);
    let uid = queued.deal.report_uid;
    forget(uid);
    let now = moon_core::util::time::now_unix_ms_i64();
    queued.window.close_ms = now;
    let generation = super::unfillable().generation;
    super::commit_station_answer(
        &[queued],
        Filled {
            markets: 1,
            ..Default::default()
        },
        false,
        generation,
    );
    let mut rows = super::unfillable();
    let refused = rows.uids.contains_key(&(1, uid));
    let waiting = rows.waiting_tail(1, uid, now);
    rows.deferred.remove(&(1, uid));
    drop(rows);
    super::RETRY_AT.store(i64::MAX, std::sync::atomic::Ordering::Release);
    assert!(!refused, "asked too early is not a refusal");
    assert!(
        waiting,
        "and is not asked about again until the tail settles"
    );
}

/// An idle table hears nothing from the station, so the tick wakes it — once — when a row left
/// unrefused reaches the moment its tail is final, and when the refusal memory moves. A wake
/// armed for later stays armed, and an earlier one armed meanwhile is not lost.
#[test]
fn the_tick_wakes_an_idle_table_once_per_cause() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let _refusals = UNFILLABLE_TEST.lock().unwrap_or_else(|p| p.into_inner());
    use std::sync::atomic::Ordering;
    // Settle the state other tests left: one call takes whatever moved before this test.
    super::RETRY_AT.store(i64::MAX, Ordering::Release);
    super::wake_due(0);
    assert!(!super::wake_due(0), "nothing armed, nothing moved");
    super::RETRY_AT.fetch_min(5_000, Ordering::AcqRel);
    assert!(!super::wake_due(4_999), "the tail is not final yet");
    assert_eq!(
        super::RETRY_AT.load(Ordering::Acquire),
        5_000,
        "still armed"
    );
    assert!(super::wake_due(5_000));
    assert!(!super::wake_due(5_001), "one wake for one settled tail");
    // A refusal joins and leaves: each change of the memory is one wake.
    super::unfillable().refuse(1, -94_072, "synthetic", "XALPHA");
    assert!(super::wake_due(0));
    assert!(!super::wake_due(0));
    forget(-94_072);
    assert!(super::wake_due(0));
}

/// A row asked about before the recorder treats its post-close tail as final is still being
/// written, not missing: it must not be remembered as refused. The deadline is the station's
/// own — the close, the longer of the margin and the tail, and its two-minute give-up.
#[test]
fn a_row_is_not_refused_while_its_tail_can_still_arrive() {
    let window = ReplayWindow {
        close_ms: 2_000_000,
        margin_ms: 600_000,
        ..queued_unservable(-94_071).window
    };
    // Ten minutes of margin outlast the one-minute floor; then 120 s.
    let settled = 2_000_000 + 600_000 + 120_000;
    assert!(super::tail_may_follow(&window, 60, settled - 1));
    assert!(!super::tail_may_follow(&window, 60, settled));
    // A floor longer than the margin moves the deadline with it.
    let short_margin = ReplayWindow {
        margin_ms: 30_000,
        ..window
    };
    let settled = 2_000_000 + 900_000 + 120_000;
    assert!(super::tail_may_follow(&short_margin, 900, settled - 1));
    assert!(!super::tail_may_follow(&short_margin, 900, settled));
}

/// Reusing a report uid on another core must not suppress that core's station request.
#[test]
fn station_refusals_are_scoped_to_their_core() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let _guard = UNFILLABLE_TEST.lock().unwrap_or_else(|p| p.into_inner());
    let first = queued_unservable(-94_004);
    let uid = first.deal.report_uid;
    forget(uid);
    let second_address = Arc::new(super::super::super::state::RowAddress {
        core_uid: 2,
        ..(*first.address).clone()
    });
    let close_ms = first.window.close_ms;
    let generation = super::unfillable().generation;
    super::commit_station_answer(
        &[first],
        Filled {
            markets: 1,
            ..Default::default()
        },
        false,
        generation,
    );
    let retry = super::super::super::tail::fillable_at(
        &second_address,
        uid,
        close_ms,
        30_000,
        close_ms + 3_600_000,
    );
    forget(uid);
    assert!(retry, "core 2 has not received a refusal for its own row");
}

/// The request must use the captured row margin, not a newer process-wide setting.
#[test]
fn station_requests_keep_the_rows_captured_tail_and_exact_front_gap() {
    let _window = crate::test_support::SharedWindowGuard::take();
    moon_core::market::trade_replay::set_margin_s(600);
    let row = trade("XALPHA", 1_000_000, 1_010_000);
    let wants = lacking(
        &[row],
        |row| row.clone(),
        |_, _, _, _| Some(vec![(990_000, 1_070_000)]),
    );
    assert_eq!(wants.len(), 1);
    assert_eq!(wants[0].spans, vec![(970_000, 989_999)]);
}

/// A short-tail exception cannot hide a missing pre-entry margin.
#[test]
fn allowing_short_tails_never_drops_a_missing_front_margin() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let row = trade("XALPHA", 1_000_000, 1_010_000);
    let (rows, held) = super::drop_held_at(
        60,
        vec![row],
        |row| row.clone(),
        |_, _, _, _| Some(vec![(990_000, 1_010_000)]),
        |_, _| true,
    );
    assert_eq!(held, 0);
    assert_eq!(rows.len(), 1);
}

/// Model pads alone cannot classify a 180-second pack as a held 600-second window.
#[test]
fn widened_margin_is_not_held_even_when_model_pads_are_complete() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let mut row = trade("XALPHA", 1_000_000, 1_010_000);
    row.2.margin_ms = 600_000;
    row.2.from_ms = 400_000;
    row.2.to_ms = 1_610_000;
    for allow_short in [false, true] {
        let (rows, held) = super::drop_held_at(
            60,
            vec![row.clone()],
            |row| row.clone(),
            |_, _, _, _| Some(vec![(820_000, 1_190_000)]),
            |_, _| allow_short,
        );
        assert_eq!(held, 0);
        assert_eq!(rows.len(), 1);
    }
}

/// Long positions request both endpoint margins while skipping their idle middle.
#[test]
fn long_positions_request_both_ends_and_short_close_tails() {
    let _window = crate::test_support::SharedWindowGuard::take();
    moon_core::market::trade_replay::set_margin_s(600);
    let mut row = trade("XALPHA", 1_000_000, 4_600_000);
    row.2.margin_ms = 600_000;
    row.2.long_position_ms = 300_000;
    row.2.from_ms = 400_000;
    row.2.to_ms = 5_200_000;
    let wants = lacking(
        &[row],
        |row| row.clone(),
        |_, _, _, _| Some(vec![(820_000, 1_180_000), (4_420_000, 4_780_000)]),
    );
    assert_eq!(
        wants[0].spans,
        vec![
            (400_000, 819_999),
            (1_180_001, 1_600_000),
            (4_000_000, 4_419_999),
            (4_780_001, 5_200_000)
        ]
    );
}

/// A ceiling-evicted fill must become final for this generation rather than re-fetch forever.
#[test]
fn a_fill_evicted_by_the_injected_cap_is_not_requested_again() {
    use moon_core::feed::types::{Side, Tick};
    use moon_core::market::trade_replay::trade_cache::TradeCache;
    let _window = crate::test_support::SharedWindowGuard::take();
    let _refusals = UNFILLABLE_TEST.lock().unwrap_or_else(|p| p.into_inner());
    let cache = TradeCache::open_with_ceiling(":memory:".into(), || Some(0)).unwrap();
    let uid = -94_002;
    forget(uid);
    let place = |row: &super::super::job::QueuedRow| {
        (
            row.address.exchange_key.clone(),
            row.address.market.clone(),
            row.window,
        )
    };
    let held =
        |exchange: &str, market: &str, from, to| cache.held_spans(exchange, market, from, to);
    let (rows, _, _, filled) = prefill_rows(
        vec![queued_unservable(uid)],
        place,
        held,
        |_, _| false,
        |rows| {
            let wants = lacking(rows, place, held);
            assert_eq!(wants.len(), 1);
            for want in wants {
                for (from, to) in want.spans {
                    cache.insert_from_station(
                        &want.exchange,
                        &want.market,
                        from,
                        to,
                        vec![Tick {
                            time_ms: from as f64,
                            price: 1.0,
                            qty: 1.0,
                            side: Side::Buy,
                        }],
                    );
                }
            }
            assert!(cache.sync(std::time::Duration::from_secs(5)));
            Filled {
                markets: 1,
                pieces: 1,
                ..Default::default()
            }
        },
    );
    assert_eq!(
        rows.len(),
        1,
        "zero ceiling evicts the freshly inserted print"
    );
    assert!(
        held("synthetic", "XALPHAUSDT", 0, 1_000_000)
            .unwrap()
            .is_empty()
    );
    let now_generation = super::unfillable().generation;
    super::commit_station_answer(&rows, filled, false, now_generation);
    assert!(!super::candidate(1, uid));
    let next_requests = rows
        .iter()
        .filter(|row| super::candidate(1, row.deal.report_uid))
        .count();
    forget(uid);
    assert_eq!(next_requests, 0);
}

/// A broken multi-market pull must not turn unvisited rows into permanent station misses.
#[test]
fn partial_pull_failure_does_not_latch_unanswered_rows() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let _refusals = UNFILLABLE_TEST.lock().unwrap_or_else(|p| p.into_inner());
    let uid = -94_003;
    forget(uid);
    let now_generation = super::unfillable().generation;
    super::commit_station_answer(
        &[queued_unservable(uid)],
        Filled {
            markets: 2,
            pieces: 1,
            failed: true,
            ..Default::default()
        },
        false,
        now_generation,
    );
    let retry = super::candidate(1, uid);
    forget(uid);
    assert!(retry, "transport/persistence failures must stay retryable");
}

/// Drop a fixture refusal while preserving the store's snapshot invalidation invariant.
fn forget(uid: i64) {
    let mut rows = super::unfillable();
    if rows.uids.remove(&(1, uid)).is_some() {
        rows.revision += 1;
    }
}

/// Repainting unchanged rows must share membership instead of allocating another set.
#[test]
fn refusal_snapshots_share_an_arc_until_membership_changes() {
    let mut rows = Unfillable::default();
    let (_, empty) = rows.snapshot();
    assert!(Arc::ptr_eq(&empty, &rows.snapshot().1));
    rows.refuse(1, 91, "synthetic", "XALPHA");
    let (revision, refused) = rows.snapshot();
    assert!(!Arc::ptr_eq(&empty, &refused));
    assert!(refused.contains(&(1, 91)));
    assert!(!empty.contains(&(1, 91)));
    rows.refuse(1, 91, "synthetic", "XALPHA");
    rows.filed("synthetic", "XBETA");
    let (same_revision, same) = rows.snapshot();
    assert_eq!(same_revision, revision);
    assert!(Arc::ptr_eq(&refused, &same));
    rows.filed("synthetic", "XALPHA");
    let (_, cleared) = rows.snapshot();
    assert!(!Arc::ptr_eq(&refused, &cleared));
    assert!(cleared.is_empty());
    assert!(
        refused.contains(&(1, 91)),
        "an in-progress pass keeps its captured membership"
    );
}

/// A reader recounts only the markets whose refusal membership changed after its revision —
/// a refusal, a filing that lifts one, and a generation change clearing every refused market.
#[test]
fn refusals_since_names_only_the_markets_that_changed() {
    let mut rows = Unfillable::default();
    let start = rows.since(0).0;
    rows.refuse(1, 91, "EX-XX", "XALPHAUSDT");
    let (after_alpha, changed) = rows.since(start);
    assert_eq!(
        changed,
        [("EX-XX".to_string(), "XALPHAUSDT".to_string())].into()
    );
    rows.refuse(1, 91, "EX-XX", "XALPHAUSDT");
    rows.filed("EX-XX", "XBETAUSDT");
    assert!(rows.since(after_alpha).1.is_empty(), "no membership change");
    rows.refuse(1, 92, "EX-XX", "XBETAUSDT");
    let (after_beta, changed) = rows.since(after_alpha);
    assert_eq!(
        changed,
        [("EX-XX".to_string(), "XBETAUSDT".to_string())].into()
    );
    rows.filed("EX-XX", "XALPHAUSDT");
    assert_eq!(
        rows.since(after_beta).1,
        [("EX-XX".to_string(), "XALPHAUSDT".to_string())].into()
    );
    let before_refresh = rows.since(0).0;
    rows.refresh(((1, 0), 0, (0, 0), 0));
    assert_eq!(
        rows.since(before_refresh).1,
        [("EX-XX".to_string(), "XBETAUSDT".to_string())].into(),
        "a wholesale clear names the markets it cleared"
    );
}

/// A refusal must refresh chips and counts, and unrelated filings must keep the tail admitted.
#[test]
fn refusal_refreshes_cached_classification_until_its_market_is_filed() {
    let _window = crate::test_support::SharedWindowGuard::take();
    use super::super::super::state::{DealRow, TapeStatus, TicksData, TicksState};
    use super::super::super::{rows, tail, why::DealChip};
    use moon_core::db::tuner::ticks::Verdict;
    use moon_core::db::tuner::ticks::verify::{EntryFinding, ExitFinding, RuleFlags, StopFacts};
    let _guard = UNFILLABLE_TEST.lock().unwrap_or_else(|p| p.into_inner());
    let queued = queued_unservable(-91_510);
    let uid = queued.deal.report_uid;
    let now = queued.window.close_ms + 3_600_000;
    let row = DealRow {
        deal: queued.deal.clone(),
        tape: TapeStatus::Covered,
        verdict: Some(Verdict {
            entry: Some(true),
            entry_dev_pct: None,
            exit: Some(true),
            exit_dev_pct: None,
            fill: None,
            exit_kind: None,
            line_points: None,
            entry_finding: EntryFinding::Hit,
            exit_finding: ExitFinding::Hit,
            fill_clock_ms: None,
            rules: RuleFlags::default(),
            stop: StopFacts::default(),
        }),
        address: Some(queued.address.clone()),
        ticks: Some(super::super::super::tape::PackedTape::pack(Vec::new())),
        entry_line: None,
        held: Some((30_000, 30_000)),
    };
    let mut state = TicksState::default();
    state.only_fit = true;
    state.data.apply(Ok(TicksData {
        rows: vec![row].into(),
        ..Default::default()
    }));
    let before = rows::counts_at(&mut state, 60, now);
    assert_eq!(before.get(DealChip::ShortTail), 1);
    assert_eq!(rows::population_at(&state, 60, now).fit, 0);
    assert!(rows::order_for(&mut state, 60, now).is_empty());
    let now_generation = super::unfillable().generation;
    super::commit_station_answer(
        &[queued],
        Filled {
            markets: 1,
            ..Default::default()
        },
        false,
        now_generation,
    );
    let after = rows::counts_at(&mut state, 60, now);
    assert_eq!(after.get(DealChip::AcceptedShortTail), 1);
    assert_eq!(rows::population_at(&state, 60, now).fit, 1);
    assert_eq!(rows::order_for(&mut state, 60, now), &[0]);
    let admission = tail::Admission::at(60, now);
    assert!(state.data.data().unwrap().rows[0].fit_with(&admission));
    super::unfillable().filed("synthetic", "XBETAUSDT");
    assert_eq!(
        rows::counts_at(&mut state, 60, now).get(DealChip::AcceptedShortTail),
        1
    );
    assert!(
        !state.data.data().unwrap().rows[0].station_candidate_with(&tail::Admission::at(60, now))
    );
    super::unfillable().filed("synthetic", "XALPHAUSDT");
    assert_eq!(
        rows::counts_at(&mut state, 60, now).get(DealChip::ShortTail),
        1
    );
    assert_eq!(rows::population_at(&state, 60, now).fit, 0);
    assert!(rows::order_for(&mut state, 60, now).is_empty());
    assert!(!state.data.data().unwrap().rows[0].fit_with(&tail::Admission::at(60, now)));
    assert!(!super::unfillable().uids.contains_key(&(1, uid)));

    // A clamped request must agree at its boundary in disk skipping and sample admission.
    let cap_ms = moon_core::db::tuner::ticks::tail::floor_ms(u32::MAX);
    for (trail, expected) in [(cap_ms, true), (cap_ms - 1, false)] {
        let row = &mut state.data.data_mut().unwrap().rows[0];
        row.held = Some((30_000, trail));
        let admission = tail::Admission::at(u32::MAX, now);
        assert_eq!(row.fit_with(&admission), expected);
        let window = trade("XALPHAUSDT", row.deal.buy_ms, row.deal.close_ms).2;
        let (_, held) = super::drop_held_at(
            u32::MAX,
            vec![row.clone()],
            |_| ("synthetic".into(), "XALPHAUSDT".into(), window),
            |_, _, _, _| Some(vec![(window.from_ms, window.close_ms + trail)]),
            |row, _| row.short_tail_with(&admission),
        );
        assert_eq!(held == 1, expected);
    }
}

/// A latched final row stops waiting even if its timestamps still lie inside core retention.
#[test]
fn station_final_tail_no_longer_waits_for_a_source() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let _guard = UNFILLABLE_TEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let row = queued_unservable(-91_401);
    let now = row.window.close_ms + 3_600_000;
    forget(row.deal.report_uid);
    assert!(super::super::super::tail::fillable_at(
        &row.address,
        row.deal.report_uid,
        row.window.close_ms,
        30_000,
        now
    ));
    super::unfillable().refuse(1, row.deal.report_uid, "synthetic", "XALPHAUSDT");
    let fillable = super::super::super::tail::fillable_at(
        &row.address,
        row.deal.report_uid,
        row.window.close_ms,
        30_000,
        now,
    );
    forget(row.deal.report_uid);
    assert!(!fillable);
}

/// A disk-held row must not invalidate the view when the station filed no new pieces.
#[test]
fn already_held_rows_do_not_bump_the_station_filing_generation() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let before = super::filed_generation();
    let (_, held, _, filled) = prefill_rows(
        vec![trade("HELD", 100_000, 110_000)],
        |row| row.clone(),
        |_, _, _, _| Some(vec![(0, 1_000_000)]),
        |_, _| false,
        |_| panic!("held row must not reach the station"),
    );
    assert_eq!(held, 1);
    note_filed(filled);
    assert_eq!(super::filed_generation(), before);
}

/// Unrelated filings preserve accepted tails; relevant filings and policy changes retry them.
#[test]
fn station_misses_survive_unrelated_filings() {
    let mut refused = Unfillable::default();
    refused.refresh(((7, 0), 60, (180_000, 300_000), 0));
    refused.refuse(1, 91, "synthetic", "XALPHA");
    let revision = refused.revision;
    refused.filed("synthetic", "XBETA");
    refused.filed("other", "XALPHA");
    assert!(refused.uids.contains_key(&(1, 91)));
    assert_eq!(refused.revision, revision);
    refused.filed("synthetic", "XALPHA");
    assert!(refused.uids.is_empty());
    assert_ne!(refused.revision, revision);
    for generation in [
        ((8, 0), 60, (180_000, 300_000), 0),
        ((8, 0), 120, (180_000, 300_000), 0),
        ((8, 0), 120, (600_000, 300_000), 0),
        ((8, 0), 120, (600_000, 1_800_000), 0),
        ((8, 0), 120, (600_000, 1_800_000), 1),
        // The station filed tape for one more trade: its complete-tape count alone moved.
        ((8, 1), 120, (600_000, 1_800_000), 1),
    ] {
        refused.refuse(1, 91, "synthetic", "XALPHA");
        let revision = refused.revision;
        refused.refresh(generation);
        assert!(refused.uids.is_empty());
        assert_ne!(refused.revision, revision);
    }
}

/// Publishing a smaller sample must forget its departed rows and their unreferenced market history.
#[test]
fn refusals_leave_with_their_rows() {
    let mut refused = Unfillable::default();
    refused.refuse(1, 91, "synthetic", "XALPHA");
    refused.refuse(1, 92, "synthetic", "XALPHA");
    refused.refuse(2, 91, "synthetic", "XBETA");
    let live = std::collections::HashSet::from([(1, 92)]);
    let markets =
        std::collections::HashSet::from([("synthetic".to_string(), "XALPHA".to_string())]);

    refused.retain(&live, &markets);

    assert_eq!(refused.uids.len(), 1);
    assert!(refused.uids.contains_key(&(1, 92)));
    assert!(!refused.uids.contains_key(&(2, 91)));
    assert!(
        refused
            .changed
            .contains_key(&("synthetic".to_string(), "XALPHA".to_string()))
    );
    assert!(
        !refused
            .changed
            .contains_key(&("synthetic".to_string(), "XBETA".to_string()))
    );
}

/// Measure a synthetic day of 100,000 refusals shrinking to the 2,000 rows still published.
#[test]
#[ignore = "manual refusal retention scale measurement"]
fn bench_refusal_retention_day() {
    let mut samples = Vec::new();
    for _ in 0..5 {
        let mut refused = Unfillable::default();
        let mut live = std::collections::HashSet::new();
        let mut markets = std::collections::HashSet::new();
        for core in 1..=10_u64 {
            for uid in 1..=10_000_i64 {
                let market = format!("XALPHA{}", uid % 1_000);
                refused.refuse(core, uid, "synthetic", &market);
                markets.insert(("synthetic".to_string(), market));
                if uid <= 200 {
                    live.insert((core, uid));
                }
            }
        }
        let before = refused.uids.len();
        let start = std::time::Instant::now();
        refused.retain(&live, &markets);
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!((before, refused.uids.len()), (100_000, 2_000));
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "[bench] station refusals retained=100000 -> 2000 prune median={:.6} ms",
        samples[2]
    );
}

/// A refused station-only row drops out of the footer and the next automatic prefill.
#[test]
fn an_unservable_row_is_not_offered_or_counted_again() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let _guard = UNFILLABLE_TEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    use super::super::super::state::{DealRow, RowAddress, TapeStatus};
    use moon_core::db::tuner::ticks::{Deal, Deltas};
    use moon_core::market::trade_replay::TickStatus;
    use moon_core::venue::{Brand, MarketKind, Venue};
    use std::sync::Arc;
    let row = DealRow {
        deal: Deal {
            report_uid: -91_001,
            core_uid: 1,
            core_name: String::new(),
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
            deltas: Deltas::default(),
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
            leverage: None,
            stop_anchor: None,
            delta_track: None,
            quotes: None,
            bars: None,
            own_entry: None,
            buy_set_ms: None,
            corridor: None,
            entry_placed: None,
            gap: None,
        },
        tape: TapeStatus::Refused(TickStatus::NoRoute),
        verdict: None,
        address: Some(Arc::new(RowAddress {
            core_uid: 1,
            venue: Some(Venue {
                brand: Brand::Bybit,
                kind: MarketKind::Spot,
            }),
            exchange_key: "synthetic".into(),
            market: "XALPHAUSDT".into(),
            btc_market: None,
        })),
        ticks: None,
        entry_line: None,
        held: None,
    };
    let now = 500_000_000;
    assert!(super::super::station_load_candidate(
        &row,
        &super::super::super::tail::Admission::at(60, now)
    ));
    super::unfillable().refuse(1, row.deal.report_uid, "synthetic", "XALPHAUSDT");
    assert!(!super::super::station_load_candidate(
        &row,
        &super::super::super::tail::Admission::at(60, now)
    ));
    let address = row.address.as_ref().unwrap().clone();
    let window = moon_core::db::tuner::ticks::model_window(&row.deal, 30_000, 60_000).unwrap();
    let source =
        moon_core::market::MarketDataSource::new(moon_core::market::MarketStore::shared(0.0));
    source.set_provider_map(&[(1, 1)].into());
    source.set_provider_exchanges(&[(1, moon_core::feed::ExchangeId::new(0))].into());
    let queued = super::super::job::QueuedRow::new(
        row.deal.clone(),
        address,
        source.replay_address(1).unwrap(),
        moon_core::market::trade_replay::venue_caps::TickValue::Base,
        window,
    );
    let (remaining, _, _) = super::prefill(vec![queued], None, || false);
    assert!(
        remaining.is_empty(),
        "the next autoload must not offer the row again"
    );
    forget(row.deal.report_uid);
}

/// An empty station answer for a row no later source can fetch must be remembered.
///
/// Deleting the refusal extend in `commit_station_answer` leaves the uid out, so the next
/// autoload asks the station for that row again.
#[test]
fn empty_station_answer_is_remembered_when_nothing_else_can_fetch() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let _guard = UNFILLABLE_TEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let queued = queued_unservable(-91_302);
    let uid = queued.deal.report_uid;
    forget(uid);
    let (rows, _, _, filled) = prefill_rows(
        vec![queued],
        |row| {
            (
                row.address.exchange_key.clone(),
                row.address.market.clone(),
                row.window,
            )
        },
        |_, _, _, _| None,
        |_, _| false,
        |_| Filled {
            markets: 1,
            ..Default::default()
        },
    );
    assert_eq!(
        rows.len(),
        1,
        "an empty answer must leave the row for refusal memory"
    );
    let now_generation = super::unfillable().generation;
    super::commit_station_answer(&rows, filled, false, now_generation);
    assert!(
        super::unfillable().uids.contains_key(&(1, uid)),
        "an empty station answer must remember a row nothing else can fetch"
    );
    forget(uid);
}

/// A Bybit row from 1970: no trade route, and the missing tape is outside the core horizon.
fn queued_unservable(report_uid: i64) -> super::super::job::QueuedRow {
    use super::super::super::state::{DealRow, RowAddress, TapeStatus};
    use moon_core::db::tuner::ticks::{Deal, Deltas};
    use moon_core::market::trade_replay::TickStatus;
    use moon_core::venue::{Brand, MarketKind, Venue};
    use std::sync::Arc;
    let row = DealRow {
        deal: Deal {
            report_uid,
            core_uid: 1,
            core_name: String::new(),
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
            deltas: Deltas::default(),
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
            leverage: None,
            stop_anchor: None,
            delta_track: None,
            quotes: None,
            bars: None,
            own_entry: None,
            buy_set_ms: None,
            corridor: None,
            entry_placed: None,
            gap: None,
        },
        tape: TapeStatus::Refused(TickStatus::NoRoute),
        verdict: None,
        address: Some(Arc::new(RowAddress {
            core_uid: 1,
            venue: Some(Venue {
                brand: Brand::Bybit,
                kind: MarketKind::Spot,
            }),
            exchange_key: "synthetic".into(),
            market: "XALPHAUSDT".into(),
            btc_market: None,
        })),
        ticks: None,
        entry_line: None,
        held: None,
    };
    let address = row.address.as_ref().unwrap().clone();
    let window = moon_core::db::tuner::ticks::model_window(&row.deal, 30_000, 60_000).unwrap();
    let source =
        moon_core::market::MarketDataSource::new(moon_core::market::MarketStore::shared(0.0));
    source.set_provider_map(&[(1, 1)].into());
    source.set_provider_exchanges(&[(1, moon_core::feed::ExchangeId::new(0))].into());
    super::super::job::QueuedRow::new(
        row.deal,
        address,
        source.replay_address(1).unwrap(),
        moon_core::market::trade_replay::venue_caps::TickValue::Base,
        window,
    )
}

/// Dropping by the replay pad would skip the station and leave thirty-second rows short forever.
#[test]
fn station_fills_short_tail_before_the_remaining_rows_are_queued() {
    let _window = crate::test_support::SharedWindowGuard::take();
    use std::cell::Cell;
    let end = Cell::new(140_000);
    let offered = Cell::new(0);
    let (rest, held, from_station, _) = prefill_rows(
        vec![
            trade("FILLED", 100_000, 110_000),
            trade("VENUE", 100_000, 110_000),
        ],
        |row| row.clone(),
        |_, market, _, _| {
            Some(vec![(
                70_000,
                if market == "FILLED" {
                    end.get()
                } else {
                    140_000
                },
            )])
        },
        |_, _| false,
        |rows| {
            offered.set(rows.len());
            let wants = lacking(
                rows,
                |row| row.clone(),
                |_, _, _, _| Some(vec![(70_000, 140_000)]),
            );
            assert!(
                wants
                    .iter()
                    .all(|want| want.spans == vec![(140_001, 170_000)])
            );
            end.set(170_000);
            Filled {
                pieces: 1,
                ..Default::default()
            }
        },
    );
    assert_eq!(offered.get(), 2);
    assert_eq!(held, 1);
    assert_eq!(from_station, 1);
    assert_eq!(
        rest.iter().map(|row| row.1.as_str()).collect::<Vec<_>>(),
        vec!["VENUE"]
    );
}

/// A trade `(exchange, market, open_ms, close_ms)` and its window, 30 s either side.
fn trade(market: &str, open_ms: i64, close_ms: i64) -> (String, String, ReplayWindow) {
    let window = moon_core::market::trade_replay::replay_window_ms(open_ms, close_ms, 30_000)
        .expect("a window");
    ("x".to_string(), market.to_string(), window)
}

/// The station is asked for each market once, for the windows of its rows merged, minus what the
/// file already holds; a market the file holds whole is not asked, and a market whose read did not
/// happen is asked whole.
#[test]
fn the_station_is_asked_only_for_what_the_file_lacks() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let rows = vec![
        trade("A", 100_000, 110_000),
        trade("A", 105_000, 150_000),
        trade("B", 100_000, 110_000),
        trade("C", 100_000, 110_000),
    ];
    let wants = lacking(
        &rows,
        |row| row.clone(),
        |_, market, from, to| match market {
            // A: the first 20 s held.
            "A" => Some(vec![(from, from + 19_999)]),
            // B: all of it held.
            "B" => Some(vec![(from, to)]),
            // C: the read did not happen.
            _ => None,
        },
    );
    assert_eq!(
        wants
            .iter()
            .map(|w| (w.market.as_str(), w.spans.clone()))
            .collect::<Vec<_>>(),
        vec![
            ("A", vec![(90_000, 210_000)]),
            ("C", vec![(70_000, 170_000)]),
        ]
    );
}
