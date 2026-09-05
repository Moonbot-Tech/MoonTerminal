use std::net::{IpAddr, Ipv4Addr};

use super::{BalanceState, ConnStatus, CoreData};
use crate::feed::{
    ApiKeyExpiry, ConnFault, ConnFaultKind, CoreConfig, CoreConfigArea, CoreConfigEditEvent,
    CoreConfigEditPhase, CoreConfigEditResult, CoreConfigEditRow, CoreConfigRejection,
    CoreEndpoint, CoreIdentityFacts, CoreStartupStatus, CoreSysStatus, FeedMsg, FieldMask,
    OrderRow, OrderTrace, OrderTracePoint,
};

/// A core with the given freshness inputs; everything else stays at its default.
fn core(assets_rev: u64, rate_known: bool, stale: bool, status: ConnStatus) -> CoreData {
    let mut cd = CoreData::new();
    cd.assets_rev = assets_rev;
    cd.assets.global.usd_rate_known = rate_known;
    cd.assets_stale = stale;
    cd.status = status;
    cd
}

/// Build a completed failure whose retention is visible while a retry is connecting.
fn connection_fault() -> ConnFault {
    ConnFault {
        kind: ConnFaultKind::ConnectTimedOut { timeout_ms: 10_000 },
        identity: CoreIdentityFacts::default(),
        startup: CoreStartupStatus::default(),
    }
}

/// `store.rs:CoreData::begin_connection_attempt` must not add `self.fault = None`; clearing the
/// last failure during retry would return users to an unexplained Connection 0/1 state each backoff
/// cycle.
#[test]
fn a_replacement_attempt_retains_the_last_fault_to_explain_the_retry() {
    let fault = connection_fault();
    let mut core = CoreData::new();
    core.apply(FeedMsg::ConnFault(fault.clone()));

    core.begin_connection_attempt();

    assert_eq!(core.fault, Some(fault));
}

/// `store.rs:CoreData::apply` must keep its `ConnStatus::Ready` fault clear; removing that clear
/// would leave a red connection verdict on a healthy core and keep Settings repainting behind it.
#[test]
fn a_ready_status_erases_the_failure_that_preceded_it() {
    let mut core = CoreData::new();
    core.apply(FeedMsg::ConnFault(connection_fault()));

    core.apply(FeedMsg::Status(ConnStatus::Ready));

    assert_eq!(core.fault, None);
}

/// `store.rs:CoreData::apply` must clear a reported build on every non-Ready status; removing
/// that clear makes a reconnect to another MoonBot retain the previous host's build in Core Status.
#[test]
fn core_status_version_belongs_only_to_the_ready_connection_that_reported_it() {
    let mut core = CoreData::new();

    core.apply(FeedMsg::CoreVersion { version: 734 });
    assert_eq!(core.server_version, Some(734));

    core.apply(FeedMsg::Status(ConnStatus::Ready));
    assert_eq!(
        core.server_version,
        Some(734),
        "Ready keeps the reported build"
    );

    for status in [
        ConnStatus::Stage("reconnecting".to_string()),
        ConnStatus::Connecting,
        ConnStatus::Disconnected,
        ConnStatus::Failed("replacement failed".to_string()),
    ] {
        core.apply(FeedMsg::CoreVersion { version: 735 });
        core.apply(FeedMsg::Status(status));
        assert_eq!(
            core.server_version, None,
            "a non-Ready state must not speak for the previous connection"
        );
    }
}

/// No snapshot yet is UNKNOWN, never zero — the distinction the Assets panel exists to make.
#[test]
fn without_a_snapshot_the_balance_is_awaiting() {
    let cd = core(0, true, false, ConnStatus::Ready);
    assert_eq!(cd.balance_state(), BalanceState::Awaiting);
    assert!(!cd.balance_state().has_value());
}

/// An unvaluable snapshot outranks staleness: there is no number, so its age is moot.
#[test]
fn unpriced_outranks_stale() {
    let cd = core(7, false, true, ConnStatus::Disconnected);
    assert_eq!(cd.balance_state(), BalanceState::Unpriced);
    assert!(!cd.balance_state().has_value());
}

/// A live connection is not enough on its own. `assets_rev` and the snapshot both survive a
/// reconnect, so a core back at `Ready` still carries the retained figure until a new snapshot
/// clears the marker — without this the pre-outage balance would be re-promoted to Live.
#[test]
fn a_reconnected_core_stays_stale_until_the_marker_clears() {
    let reconnected = core(7, true, true, ConnStatus::Ready);
    assert_eq!(reconnected.balance_state(), BalanceState::Stale);
    // A retained figure is still a figure: it is shown, but only with its stale marker.
    assert!(reconnected.balance_state().has_value());
    assert!(!reconnected.balance_state().is_current());
}

/// The other half of staleness: a snapshot that arrived before the link ever reached `Ready`.
#[test]
fn a_snapshot_from_a_not_ready_link_is_stale() {
    let cd = core(7, true, false, ConnStatus::Connecting);
    assert_eq!(cd.balance_state(), BalanceState::Stale);
}

/// Ready, priced, and with no stale marker — the only combination that renders at full
/// strength.
#[test]
fn ready_priced_and_unmarked_is_live() {
    let cd = core(7, true, false, ConnStatus::Ready);
    assert_eq!(cd.balance_state(), BalanceState::Live);
    assert!(cd.balance_state().has_value());
    assert!(cd.balance_state().is_current());
}

/// `code()` must separate every variant: it is hashed into a render signature, so a collision
/// would let one trust state be cached as another.
#[test]
fn every_state_hashes_distinctly() {
    let all = [
        BalanceState::Live,
        BalanceState::Stale,
        BalanceState::Awaiting,
        BalanceState::Unpriced,
    ];
    let mut codes: Vec<u64> = all.iter().map(|s| s.code()).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), all.len());
    // Only Live is current, and only Live/Stale carry a number.
    assert_eq!(all.iter().filter(|s| s.is_current()).count(), 1);
    assert_eq!(all.iter().filter(|s| s.has_value()).count(), 2);
}

/// `store.rs:CoreData::apply` must compare endpoint updates before bumping `sys_rev`; removing the
/// comparison churns Core Status on duplicate messages, while ignoring a changed endpoint leaves
/// the process displayed under the wrong server.
#[test]
fn an_endpoint_change_invalidates_core_status_once() {
    let first = CoreEndpoint {
        address: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
        port: 3000,
    };
    let second = CoreEndpoint {
        address: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 11)),
        port: 3000,
    };
    let mut core = CoreData::new();

    core.apply(FeedMsg::Endpoint(first));
    assert_eq!(core.endpoint, Some(first));
    assert_eq!(core.sys_rev, 1);

    core.apply(FeedMsg::Endpoint(first));
    assert_eq!(core.sys_rev, 1);

    core.apply(FeedMsg::Endpoint(second));
    assert_eq!(core.endpoint, Some(second));
    assert_eq!(core.sys_rev, 2);
}

/// `store.rs:CoreData::begin_connection_attempt` must clear both endpoint and telemetry; retaining
/// either moves the previous machine's CPU/RAM under a replacement key before fresh health arrives.
#[test]
fn a_replacement_feed_clears_endpoint_scoped_health() {
    let endpoint = CoreEndpoint {
        address: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 8)),
        port: 3000,
    };
    let health = CoreSysStatus {
        process_cpu_percent: Some(21),
        system_cpu_percent: Some(44),
        used_memory_mb: Some(512),
        free_physical_memory_mb: Some(4096),
        logical_cpu_count: Some(16),
        round_trip_ms: Some(180),
        order_api_latency_ms: Some(60),
        updated_ms: 123,
    };
    let mut core = CoreData::new();
    core.apply(FeedMsg::Endpoint(endpoint));
    core.apply(FeedMsg::SysStatus(health));
    let previous_rev = core.sys_rev;

    core.begin_connection_attempt();

    assert_eq!(core.status, ConnStatus::Connecting);
    assert_eq!(core.endpoint, None);
    assert_eq!(core.sys, CoreSysStatus::default());
    assert_eq!(core.sys_rev, previous_rev + 1);
}

/// The API-key poll re-reports the same answer every few hours, and MoonProto rebuilds the absolute
/// date from the CURRENT clock each time, so a byte-equal answer never arrives. The revision must
/// track what the answer SAYS — anything watching it would otherwise see a change every six hours
/// on a key that did not move.
#[test]
fn an_unchanged_key_answer_does_not_bump_the_revision() {
    let mut core = CoreData::new();
    let first = ApiKeyExpiry {
        unlimited: false,
        known: true,
        days_left: Some(30),
        at_unix: Some(1_800_000_000),
        checked_ms: 1_000,
    };
    core.apply(FeedMsg::ApiExpiry(first));
    let after_first = core.api_expiry_rev;

    core.apply(FeedMsg::ApiExpiry(ApiKeyExpiry {
        checked_ms: 1_000 + 6 * 60 * 60 * 1_000,
        ..first
    }));

    assert_eq!(core.api_expiry_rev, after_first, "same answer, later check");
    assert_eq!(
        core.api_expiry.map(|e| e.checked_ms),
        Some(1_000 + 6 * 60 * 60 * 1_000),
        "the newer receipt time is still retained"
    );
}

/// A key that lost a day is a real change and must reach the panel.
#[test]
fn a_changed_day_count_bumps_the_revision() {
    let mut core = CoreData::new();
    let first = ApiKeyExpiry {
        unlimited: false,
        known: true,
        days_left: Some(8),
        at_unix: Some(1_800_000_000),
        checked_ms: 1_000,
    };
    core.apply(FeedMsg::ApiExpiry(first));
    let after_first = core.api_expiry_rev;

    core.apply(FeedMsg::ApiExpiry(ApiKeyExpiry {
        days_left: Some(7),
        ..first
    }));

    assert_eq!(core.api_expiry_rev, after_first + 1);
}

/// Feeds `msgs` through the real `ServerLog` path so both counters advance as they do in production.
fn feed_log(cd: &mut CoreData, msgs: &[&str]) {
    cd.apply(FeedMsg::ServerLog(
        msgs.iter()
            .map(|msg| crate::feed::CoreLogLine {
                time_ms: 0,
                recv_ms: 0,
                msg: (*msg).to_string(),
            })
            .collect(),
    ));
}

/// `log_seq` counts LINES, unlike `log_rev`, which counts batches.
///
/// The Log panel subtracts cursors from it to learn how many lines it missed; advancing it per
/// batch would report one missed line for a batch of two hundred.
#[test]
fn log_seq_counts_lines_not_batches() {
    let mut cd = CoreData::new();
    feed_log(&mut cd, &["a", "b", "c"]);
    feed_log(&mut cd, &["d"]);

    assert_eq!(cd.log_seq, 4);
    assert_eq!(cd.log_rev, 2, "the batch counter must keep its own meaning");
}

/// A cursor reads each line exactly once, which is what makes appending safe.
#[test]
fn log_since_hands_over_each_line_once() {
    let mut cd = CoreData::new();
    feed_log(&mut cd, &["a", "b"]);

    let (lines, cursor) = cd.log_since(0);
    assert_eq!(
        lines.map(|l| l.msg.clone()).collect::<Vec<_>>(),
        ["a", "b"],
        "a zero cursor reads the whole ring"
    );

    let (lines, cursor) = cd.log_since(cursor);
    assert_eq!(lines.count(), 0, "nothing new means nothing returned");

    feed_log(&mut cd, &["c"]);
    let (lines, _) = cd.log_since(cursor);
    assert_eq!(lines.map(|l| l.msg.clone()).collect::<Vec<_>>(), ["c"]);
}

/// A minimal open order at the given wire creation time, otherwise an unused but plausible default.
fn order_row(uid: u64, create_time_ms: f64) -> OrderRow {
    OrderRow {
        market: "BTCUSDT".into(),
        market_display: "BTCUSDT".into(),
        coin: "BTC".into(),
        quote: "USDT".into(),
        is_short: false,
        size: 0.01,
        remaining_size: 0.01,
        sl_on: false,
        ts_on: false,
        vstop_on: false,
        sl_fixed: false,
        ts_fixed: false,
        vstop_fixed: false,
        vstop_level: 0.0,
        vstop_vol: 0.0,
        buy_price: 60_000.0,
        sell_price: 0.0,
        create_time_ms,
        sell_create_time_ms: 0.0,
        entry_fill_time_ms: 0.0,
        price: 60_000.0,
        fill_pct: 0.0,
        strat: "test".into(),
        strat_name: String::new(),
        strat_id: 0,
        status: String::new(),
        uid,
        emulator: false,
        job_is_done: false,
        pending: false,
        filled: false,
        stop_loss: None,
        trailing: None,
        take_profit: None,
        vstop: None,
        pending_cond: None,
        liq: None,
        panic_sell: false,
        is_moon_shot: false,
        corridor_price_down: 0.0,
        corridor_price_up: 0.0,
        buy_trace: None,
        sell_trace: None,
    }
}

/// An order row carrying a LOOSE trace sample: `create_time_ms - anchor_ms` is the skew these
/// tests want the estimator to adopt. Since HIGH 4, `create_time_ms - now_ms` alone can no longer
/// feed the estimator (see `session::clock_skew`), so every store-level test that needs an actual
/// adoption must supply trace data instead — the estimator has no `WARMUP_MS` gate on that class.
fn traced_order_row(uid: u64, create_time_ms: f64, anchor_ms: f64) -> OrderRow {
    let mut r = order_row(uid, create_time_ms);
    r.buy_trace = Some(OrderTrace {
        points: vec![
            OrderTracePoint {
                time_ms: create_time_ms,
                price: 60_000.0,
            },
            OrderTracePoint {
                time_ms: anchor_ms,
                price: 60_000.0,
            },
        ],
        tmp_point: None,
        stop_price: None,
        stop_time_ms: None,
    });
    r
}

/// A raw skewed batch, once corrected, must land the retained AND table times within a second of
/// true UTC — not just move them by some plausible-looking amount.
#[test]
fn a_skewed_batch_lands_within_a_second_of_true_utc() {
    let now = crate::util::now_unix_ms();
    let mut cd = CoreData::new();
    let raw = now + 7_200_000.0;

    cd.apply(FeedMsg::Orders(vec![
        traced_order_row(1, raw, now),
        traced_order_row(2, raw, now),
    ]));

    assert!((cd.orders[0].create_time_ms - now).abs() < 1_000.0);
    let retained = cd
        .order_lines
        .iter_market("BTCUSDT")
        .find(|o| o.uid == 1)
        .expect("order must be retained")
        .create_ms;
    assert!((retained - now).abs() < 1_000.0);
}

/// An honest UTC core must never adopt a correction, so its batch passes through untouched and
/// `order_lines_rev` advances exactly as it did before this estimator existed.
#[test]
fn an_honest_utc_core_passes_through_byte_identical() {
    let now = crate::util::now_unix_ms();
    let mut cd = CoreData::new();

    cd.apply(FeedMsg::Orders(vec![order_row(1, now), order_row(2, now)]));

    assert_eq!(cd.orders[0].create_time_ms, now);
    assert_eq!(cd.orders[1].create_time_ms, now);
    assert_eq!(cd.clock_skew.skew_ms(), 0.0);
    assert_eq!(cd.order_lines_rev, 1);
}

/// `ingest_order_rows` must observe the RAW batch once and correct it once — calling `correct`
/// twice, or observing an already-corrected batch, would subtract the skew a second time.
#[test]
fn the_same_raw_batch_applied_twice_does_not_shift_twice() {
    let now = crate::util::now_unix_ms();
    let mut cd = CoreData::new();
    let raw = now + 7_200_000.0;
    let batch = || vec![traced_order_row(1, raw, now), traced_order_row(2, raw, now)];

    cd.apply(FeedMsg::Orders(batch()));
    let first = cd.orders[0].create_time_ms;

    cd.apply(FeedMsg::Orders(batch()));
    let second = cd.orders[0].create_time_ms;

    assert_eq!(first, second);
}

/// A replacement feed may be a different MoonBot on a different clock: the previous connection's
/// estimate must not survive it.
#[test]
fn begin_connection_attempt_clears_the_clock_skew_estimate() {
    let now = crate::util::now_unix_ms();
    let mut cd = CoreData::new();
    let raw = now + 7_200_000.0;
    cd.apply(FeedMsg::Orders(vec![
        traced_order_row(1, raw, now),
        traced_order_row(2, raw, now),
    ]));
    assert_eq!(cd.clock_skew.skew_ms(), 7_200_000.0);

    cd.begin_connection_attempt();

    assert_eq!(cd.clock_skew.skew_ms(), 0.0);
}

/// An estimate that only becomes adoptable on a LATER batch must repair the lines a prior batch
/// already retained with raw wire time, not just correct batches from that point forward.
///
/// Uses a NEGATIVE skew (`raw` in the PAST) rather than a positive one: a positive skew's raw
/// future-dated time would be folded to "now" by `order_lines.rs:wire_line_start`'s own pre-
/// existing guard before this estimator ever gets a second vote, masking exactly the pre-adoption
/// state this test needs to observe.
#[test]
fn a_later_adoption_repairs_lines_retained_by_an_earlier_batch() {
    let now = crate::util::now_unix_ms();
    let mut cd = CoreData::new();
    let raw = now - 7_200_000.0;

    // Batch 1: a single new order — one vote, not enough to adopt yet, so it is retained raw.
    cd.apply(FeedMsg::Orders(vec![traced_order_row(1, raw, now)]));
    let rev_after_batch1 = cd.order_lines_rev;
    let retained_before = cd
        .order_lines
        .iter_market("BTCUSDT")
        .find(|o| o.uid == 1)
        .expect("order must be retained")
        .create_ms;
    assert!(
        (retained_before - raw).abs() < 1.0,
        "batch 1 alone must not adopt"
    );

    // Batch 2: two more new orders agreeing on the same bucket — now enough votes to adopt.
    cd.apply(FeedMsg::Orders(vec![
        traced_order_row(1, raw, now),
        traced_order_row(2, raw, now),
        traced_order_row(3, raw, now),
    ]));

    let retained_after = cd
        .order_lines
        .iter_market("BTCUSDT")
        .find(|o| o.uid == 1)
        .expect("order must still be retained")
        .create_ms;
    assert!(
        (retained_after - now).abs() < 1_000.0,
        "batch 1's line must be repaired once the estimate is adopted, got {retained_after} against {now}"
    );
    assert!(cd.order_lines_rev > rev_after_batch1);
}

/// HIGH 3's pin: a reconnect must not leave retained lines double-shifted once a NEW estimate is
/// adopted on the replacement connection. `begin_connection_attempt` resets the skew AND bumps the
/// correction generation, so a retained order that reappears on the new connection RE-DERIVES its
/// `create_ms` from the row under the new estimate, rather than compounding the previous
/// connection's shift with the new one's delta.
#[test]
fn a_reconnect_then_a_new_adoption_leaves_retained_create_ms_correct() {
    let now1 = crate::util::now_unix_ms();
    let mut cd = CoreData::new();
    let raw1 = now1 + 7_200_000.0;

    // First connection adopts a +2h skew and retains uid 1 corrected under it.
    cd.apply(FeedMsg::Orders(vec![
        traced_order_row(1, raw1, now1),
        traced_order_row(2, raw1, now1),
    ]));
    assert_eq!(cd.clock_skew.skew_ms(), 7_200_000.0);
    let retained_first = cd
        .order_lines
        .iter_market("BTCUSDT")
        .find(|o| o.uid == 1)
        .expect("order must be retained")
        .create_ms;
    assert!((retained_first - now1).abs() < 1_000.0);

    // A replacement feed resets the estimate; the retained order survives the reconnect untouched
    // until the next batch.
    cd.begin_connection_attempt();
    assert_eq!(cd.clock_skew.skew_ms(), 0.0);

    // The new connection is a DIFFERENT clock: a -1.5h skew this time. uid 1 reappears alongside a
    // fresh uid agreeing on the same bucket.
    let now2 = crate::util::now_unix_ms();
    let raw2 = now2 - 5_400_000.0;
    cd.apply(FeedMsg::Orders(vec![
        traced_order_row(1, raw2, now2),
        traced_order_row(10, raw2, now2),
    ]));
    assert_eq!(cd.clock_skew.skew_ms(), -5_400_000.0);

    let retained_second = cd
        .order_lines
        .iter_market("BTCUSDT")
        .find(|o| o.uid == 1)
        .expect("order must still be retained")
        .create_ms;
    assert!(
        (retained_second - now2).abs() < 1_000.0,
        "must re-derive under the NEW connection's skew, not compound it with the old one's shift, \
         got {retained_second} against {now2}"
    );
}

/// An ordinary reconnect on an UNCHANGED core must re-adopt the same offset without moving a
/// retained start. The deleted `shift_wire_times` path double-shifted here: after `reset`,
/// `observe` returned `0 - skew` and that delta was applied to already-corrected retained times.
///
/// `store.rs:ingest_order_rows` applying that observe-delta to uids the store already knew, after
/// `update` has re-derived them, would land the entry two hours LEFT of the candles.
#[test]
fn a_reconnect_on_the_same_core_does_not_move_a_retained_start() {
    let now = crate::util::now_unix_ms();
    let mut cd = CoreData::new();
    let raw = now + 7_200_000.0;

    cd.apply(FeedMsg::Orders(vec![
        traced_order_row(1, raw, now),
        traced_order_row(2, raw, now),
    ]));
    assert_eq!(cd.clock_skew.skew_ms(), 7_200_000.0);
    let retained_before = cd
        .order_lines
        .iter_market("BTCUSDT")
        .find(|o| o.uid == 1)
        .expect("order must be retained");
    let create_before = retained_before.create_ms;
    let step0_before = retained_before.lines[0].steps[0].0;
    assert!((create_before - now).abs() < 1_000.0);

    cd.begin_connection_attempt();
    assert_eq!(cd.clock_skew.skew_ms(), 0.0);

    cd.apply(FeedMsg::Orders(vec![
        traced_order_row(1, raw, now),
        traced_order_row(2, raw, now),
    ]));
    assert_eq!(cd.clock_skew.skew_ms(), 7_200_000.0);

    let retained_after = cd
        .order_lines
        .iter_market("BTCUSDT")
        .find(|o| o.uid == 1)
        .expect("order must still be retained");
    assert!(
        (retained_after.create_ms - create_before).abs() < 1.0,
        "reconnect + same offset must not move create_ms, got {} against {}",
        retained_after.create_ms,
        create_before
    );
    assert!(
        (retained_after.lines[0].steps[0].0 - step0_before).abs() < 1.0,
        "reconnect + same offset must not move steps[0], got {} against {}",
        retained_after.lines[0].steps[0].0,
        step0_before
    );
}

/// A cursor ahead of the counter means the store restarted under the reader: read the ring again.
///
/// Saturating the subtraction to zero instead would leave that core's rows frozen forever, because
/// every later comparison stays below the stale cursor.
#[test]
fn log_since_recovers_from_a_restarted_counter() {
    let mut cd = CoreData::new();
    feed_log(&mut cd, &["fresh"]);

    let (lines, _) = cd.log_since(9_000);

    assert_eq!(lines.map(|l| l.msg.clone()).collect::<Vec<_>>(), ["fresh"]);
}

/// A repeated run-state report must not churn its revision, and losing `Ready` must move it.
///
/// Breakage: bumping on every repeat wakes every cached surface gated on the token whenever a core
/// re-states what it already said; not bumping when the confirmation drops leaves a control
/// claiming a state the current connection never reported.
#[test]
fn a_repeated_run_state_report_is_silent_but_a_lost_connection_is_not() {
    let mut core = CoreData::new();
    core.apply(FeedMsg::Status(ConnStatus::Ready));
    core.apply(FeedMsg::RuntimeState(crate::feed::RuntimeState {
        is_started: true,
        auto_detect_active: true,
    }));
    core.apply(FeedMsg::StrategiesRunning(true));
    let (runtime_rev, trading_rev) = (core.runtime_state_rev, core.strategies_running_rev);

    core.apply(FeedMsg::StrategiesRunning(true));
    assert_eq!(
        core.strategies_running_rev, trading_rev,
        "same value, same connection"
    );
    assert_eq!(core.runtime_state_rev, runtime_rev);

    core.apply(FeedMsg::Status(ConnStatus::Disconnected));
    assert_ne!(core.strategies_running_rev, trading_rev);
    assert_ne!(core.runtime_state_rev, runtime_rev);
    let trading_rev = core.strategies_running_rev;

    // The same value again, but now it re-confirms a connection that had reported nothing.
    core.apply(FeedMsg::Status(ConnStatus::Ready));
    core.apply(FeedMsg::StrategiesRunning(true));
    assert_ne!(
        core.strategies_running_rev, trading_rev,
        "re-confirmation is a change a control renders"
    );
}

/// The strategy engine and the market runtime are reported over different commands, so one must
/// never move the other's revision.
///
/// Breakage: folding them into one counter makes a pending "start trading" intent look answered by
/// an unrelated auto-detect flip, and the button hands itself back showing the pre-command state.
#[test]
fn each_run_state_half_moves_only_its_own_revision() {
    let mut core = CoreData::new();
    core.apply(FeedMsg::Status(ConnStatus::Ready));
    let (runtime_rev, trading_rev) = (core.runtime_state_rev, core.strategies_running_rev);

    core.apply(FeedMsg::StrategiesRunning(false));
    assert_ne!(core.strategies_running_rev, trading_rev);
    assert_eq!(core.runtime_state_rev, runtime_rev);

    let trading_rev = core.strategies_running_rev;
    core.apply(FeedMsg::RuntimeState(crate::feed::RuntimeState {
        is_started: true,
        auto_detect_active: false,
    }));
    assert_ne!(core.runtime_state_rev, runtime_rev);
    assert_eq!(core.strategies_running_rev, trading_rev);
}

/// A reconnect must not LOSE the core's run state — MoonProto repeats neither init nor its
/// post-init resync on `Connected { fresh: false }`, and the protocol has no request for either
/// half, so a value dropped here is a value nobody will ever send again.
///
/// Breakage (the one this test was written for): clearing the fields on a non-`Ready` status left
/// every core that survived one link blip with an "unknown" run state for the rest of the session,
/// while the core itself was `Ready` and trading.
#[test]
fn a_reconnect_keeps_the_run_state_but_marks_it_unconfirmed() {
    let mut core = CoreData::new();
    core.apply(FeedMsg::Status(ConnStatus::Ready));
    core.apply(FeedMsg::RuntimeState(crate::feed::RuntimeState {
        is_started: true,
        auto_detect_active: true,
    }));
    core.apply(FeedMsg::StrategiesRunning(true));
    assert!(core.runtime_state_confirmed && core.strategies_running_confirmed);
    let (runtime_rev, trading_rev) = (core.runtime_state_rev, core.strategies_running_rev);

    // The link blips: Reconnecting, then Ready again with no fresh push behind it.
    core.apply(FeedMsg::Status(ConnStatus::Stage("reconnecting".into())));
    core.apply(FeedMsg::Status(ConnStatus::Ready));

    assert_eq!(
        core.runtime_state.map(|state| state.is_started),
        Some(true),
        "the last known runtime state must survive the reconnect"
    );
    assert_eq!(
        core.strategies_running,
        Some(true),
        "the last known trading state must survive the reconnect"
    );
    assert!(
        !core.runtime_state_confirmed && !core.strategies_running_confirmed,
        "but neither half may still claim to be confirmed by THIS connection"
    );
    assert_ne!(
        (core.runtime_state_rev, core.strategies_running_rev),
        (runtime_rev, trading_rev),
        "losing confirmation changes what a control draws, so both revisions must move"
    );

    // A fresh push re-confirms its own half only.
    core.apply(FeedMsg::StrategiesRunning(true));
    assert!(core.strategies_running_confirmed);
    assert!(!core.runtime_state_confirmed);
}

/// A DIFFERENT MoonBot answering on the same connection must not inherit the previous one's run
/// state — unlike a reconnect, this drops the values themselves.
///
/// Breakage: MoonProto keeps its retained settings and strategy state across a server restart (it
/// clears only news and session profits), so a terminal that merely un-confirms would keep showing
/// — and let a button act on — the state of a process that no longer exists.
#[test]
fn a_server_restart_forgets_the_previous_instances_run_state() {
    let mut core = CoreData::new();
    core.apply(FeedMsg::Status(ConnStatus::Ready));
    core.apply(FeedMsg::RuntimeState(crate::feed::RuntimeState {
        is_started: true,
        auto_detect_active: true,
    }));
    core.apply(FeedMsg::StrategiesRunning(true));
    let (runtime_rev, trading_rev) = (core.runtime_state_rev, core.strategies_running_rev);

    core.apply(FeedMsg::RunStateForgotten);

    assert_eq!(core.runtime_state, None);
    assert_eq!(core.strategies_running, None);
    assert!(!core.runtime_state_confirmed && !core.strategies_running_confirmed);
    assert_ne!(core.runtime_state_rev, runtime_rev);
    assert_ne!(core.strategies_running_rev, trading_rev);

    // Idempotent: a second notice about the same restart is not a change.
    let (runtime_rev, trading_rev) = (core.runtime_state_rev, core.strategies_running_rev);
    core.apply(FeedMsg::RunStateForgotten);
    assert_eq!(core.runtime_state_rev, runtime_rev);
    assert_eq!(core.strategies_running_rev, trading_rev);
}

/// The core republishes its API request quota every few minutes, usually with the same number. The
/// revision must track the VALUE, or every reader watching it would rebuild on an unchanged quota
/// several times an hour.
#[test]
fn an_unchanged_quota_does_not_bump_the_revision() {
    let mut core = CoreData::new();
    core.apply(FeedMsg::ApiQuota(Some(1_065_447)));
    let after_first = core.api_quota_rev;

    core.apply(FeedMsg::ApiQuota(Some(1_065_447)));
    assert_eq!(core.api_quota_rev, after_first, "same quota, later push");

    core.apply(FeedMsg::ApiQuota(Some(1_065_400)));
    assert_eq!(
        core.api_quota_rev,
        after_first + 1,
        "a new number is a change"
    );
    assert_eq!(core.api_quota, Some(1_065_400));
}

/// The projection one attempt of an edit carried, as the sequence builds it: the core's own
/// snapshot with the edited area laid over it.
///
/// `drift` moves a field NO mask here names, which is what a second attempt picks up when a trader
/// changes something in Moonbot's own dialogs between two sends of the same edit.
fn attempt(drift: bool) -> CoreConfig {
    let mut cfg = moonproto::shared_config::SharedConfig::default();
    if drift {
        // Asserted, not assumed: were this ever the wire default, the drift would be no drift and
        // the regression tests below would pass against any implementation.
        assert_ne!(
            cfg.trading.multi_orders.buy_move_click, 7,
            "the drifted value must differ from the wire default"
        );
        cfg.trading.multi_orders.buy_move_click = 7;
    }
    let mut projected = crate::feed::live::core_config_from_proto(&cfg);
    // The edit itself, identical on every attempt.
    projected.general.take_profit_pct = 7.5;
    projected
}

/// One `Submitted` for that attempt, under the mask the SEND named — the union of everything
/// queued, which is not always the mask of one user edit.
fn submitted_with(config: CoreConfig, touched: FieldMask) -> FeedMsg {
    FeedMsg::CoreConfigEdit(CoreConfigEditEvent::Submitted(Box::new(
        CoreConfigEditRow {
            phase: CoreConfigEditPhase::Pending,
            submitted_at_ms: 0,
            config,
            touched,
            mismatches: None,
        },
    )))
}

/// The common case: one edit of the General page.
fn submitted(config: CoreConfig) -> FeedMsg {
    submitted_with(config, FieldMask::EMPTY.with_general())
}

/// The core's verdict on the areas it refused.
fn rejected() -> FeedMsg {
    FeedMsg::CoreConfigEdit(CoreConfigEditEvent::Resolved(
        CoreConfigEditResult::NotApplied(CoreConfigRejection::Areas(vec![CoreConfigArea::General])),
    ))
}

/// The rejection currently on the retained row, if any.
fn retained_rejection(cd: &CoreData) -> Option<&CoreConfigRejection> {
    cd.core_config_edit
        .as_ref()
        .expect("a submitted edit is retained")
        .mismatches
        .as_ref()
}

/// Regression target: a RETRY of the same edit must keep the rejection it already received.
///
/// The row's `config` is the store's only way to ask "same edit or a new one", and the sequence
/// fills it with the projection of the SENT PACKET — the core's snapshot with the edited area laid
/// over it. So a field the write never named, moved on the core between two attempts, made the two
/// projections differ, the retry read as a fresh edit, and the "the core refused these areas"
/// notice the trader had been shown was dropped.
#[test]
fn a_retry_keeps_its_rejection_when_an_untouched_area_drifted() {
    let mut cd = CoreData::new();

    // Both preconditions, asserted rather than assumed: the drift must survive INTO the
    // projection (a field dropped from `CoreConfig` would make it no drift at all), and the
    // rejection must actually reach the row.
    assert_ne!(
        attempt(false),
        attempt(true),
        "the drift must be visible in the projection this predicate compares"
    );
    cd.apply(submitted(attempt(false)));
    cd.apply(rejected());
    assert!(
        retained_rejection(&cd).is_some(),
        "the rejection must reach the row first, or this test proves nothing"
    );

    // The same edit, sent again on a snapshot that drifted outside its mask.
    cd.apply(submitted(attempt(true)));

    assert!(
        retained_rejection(&cd).is_some(),
        "a retry of the SAME edit must keep the rejection it already received"
    );
}

/// A send's mask is the UNION of everything queued, so it NARROWS when a coalesced batch's head is
/// confirmed and the rest goes out again. That is the same work still in flight and must keep its
/// verdict — mask EQUALITY would call it a different edit, and this is the commonest retry there is.
///
/// Unlike its neighbours this guards the new predicate's SHAPE rather than the shipped bug: the old
/// whole-projection comparison passed it by accident, since the two submissions were identical.
#[test]
fn a_batch_that_narrowed_after_a_partial_apply_keeps_its_rejection() {
    let mut cd = CoreData::new();

    cd.apply(submitted_with(
        attempt(false),
        FieldMask::EMPTY.with_general().with_special(),
    ));
    cd.apply(rejected());

    // The `special` half was confirmed and left the queue; the rest re-sends under what remains.
    cd.apply(submitted_with(
        attempt(false),
        FieldMask::EMPTY.with_general(),
    ));

    assert!(
        retained_rejection(&cd).is_some(),
        "a narrowed batch is still the same work and keeps its verdict"
    );
}

/// Scope GROWING is a new user edit joining the batch, and that starts fresh: the old verdict
/// describes a packet which did not carry the new area.
#[test]
fn a_batch_that_grew_by_coalescing_starts_fresh() {
    let mut cd = CoreData::new();

    cd.apply(submitted(attempt(false)));
    cd.apply(rejected());

    cd.apply(submitted_with(
        attempt(false),
        FieldMask::EMPTY.with_general().with_special(),
    ));

    assert!(
        retained_rejection(&cd).is_none(),
        "a batch that gained an area is not a retry of the old one"
    );
}

/// A row that already gave up describes work which LEFT the queue. A later submission repeating the
/// same values is a new edit, and inheriting the dead one's verdict would show a refusal for a
/// write nobody has answered yet.
#[test]
fn a_submission_after_a_give_up_does_not_inherit_its_verdict() {
    let mut cd = CoreData::new();

    cd.apply(submitted(attempt(false)));
    cd.apply(rejected());
    cd.apply(FeedMsg::CoreConfigEdit(CoreConfigEditEvent::Resolved(
        CoreConfigEditResult::GaveUp,
    )));

    cd.apply(submitted(attempt(false)));

    assert!(
        retained_rejection(&cd).is_none(),
        "the dead edit's verdict must not attach to a new one"
    );
}

/// The other half of the rule: a genuinely different edit must NOT inherit the previous one's
/// rejection, or the notice would name areas the new write never asked about.
///
/// It held before the mask was carried too — whole-projection equality separated these two as
/// well — but it is NOT redundant: it is the only test that fails if the within-mask comparison is
/// dropped, because it is the only one where two submissions differ inside the mask and nowhere
/// else.
#[test]
fn a_different_edit_does_not_inherit_the_previous_rejection() {
    let mut cd = CoreData::new();

    cd.apply(submitted(attempt(false)));
    cd.apply(rejected());

    let mut other = attempt(false);
    other.general.take_profit_pct = 9.25;
    cd.apply(submitted(other));

    assert!(
        retained_rejection(&cd).is_none(),
        "a fresh edit starts without a verdict"
    );
}
