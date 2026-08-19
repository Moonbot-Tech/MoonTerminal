use super::*;
use crate::feed::{OrderTrace, OrderTracePoint};

/// A minimal order row with the given uid and (possibly absent, `0.0`) wire creation time.
/// Everything else is a plausible but otherwise unused default.
fn row(uid: u64, create_time_ms: f64) -> OrderRow {
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

/// A buy-trace row whose first point is still the raw create-time ANCHOR. LOOSE sampling now
/// requires `points[0].time_ms == create_time_ms`; a test that used `create_time_ms = 0` and an
/// unanchored pair would silently stop voting and pass for the wrong reason.
fn loose_row(uid: u64, p0: f64, p1: f64) -> OrderRow {
    let mut r = row(uid, p0);
    r.buy_trace = Some(trace(&[p0, p1]));
    r
}

/// A trace with the given point times, `points[0] - points[1] == sample` when it has two points.
fn trace(point_times: &[f64]) -> OrderTrace {
    OrderTrace {
        points: point_times
            .iter()
            .map(|&t| OrderTracePoint {
                time_ms: t,
                price: 60_000.0,
            })
            .collect(),
        tmp_point: None,
        stop_price: None,
        stop_time_ms: None,
    }
}

/// `clock_skew.rs:best_bucket` requiring only one vote (a `MIN_VOTES` regression) would adopt one
/// of these; each stale order lands in its own bucket and none may adopt alone.
#[test]
fn old_orders_alone_never_adopt_on_one_vote() {
    let mut skew = CoreClockSkew::default();
    let now = 100_000_000.0;
    let rows = [
        row(1, now - 1_000_000.0),
        row(2, now - 5_000_000.0),
        row(3, now - 9_000_000.0),
    ];

    let delta = skew.observe(&rows, |_| true, now);

    assert_eq!(delta, None);
    assert_eq!(skew.skew_ms(), 0.0);
}

/// The oracle is the bucket's own literal, `8 * BUCKET_MS`, not the mean of the two samples
/// (`7_135_000`) — `best_bucket` mutated into an average would fail this exact-value check.
#[test]
fn two_trace_samples_near_two_hours_adopt_exactly_the_bucket() {
    let mut skew = CoreClockSkew::default();
    let r1 = loose_row(1, 8_000_000.0, 840_000.0); // sample = skew - 40s
    let r2 = loose_row(2, 9_000_000.0, 1_890_000.0); // sample = skew - 90s

    let delta = skew.observe(&[r1, r2], |_| true, 0.0);

    assert_eq!(delta, Some(-7_200_000.0));
    assert_eq!(skew.skew_ms(), 7_200_000.0);
}

/// A lone vote, even a large and plausible one, can be a single stale order rather than a real
/// skew — `MIN_VOTES` exists precisely to refuse it.
#[test]
fn a_single_future_order_adopts_nothing() {
    let mut skew = CoreClockSkew::default();
    let now = 1_000_000.0;
    let rows = [row(1, now + 9.0 * 3_600_000.0)];

    let delta = skew.observe(&rows, |_| true, now);

    assert_eq!(delta, None);
    assert_eq!(skew.skew_ms(), 0.0);
}

/// Three scattered stale orders (one vote each, three different buckets) must not out-vote or
/// dilute the two-sample cluster; the adopted value is the cluster's own bucket, not a mean across
/// all five samples.
#[test]
fn a_clustered_pair_adopts_over_scattered_singles() {
    let mut skew = CoreClockSkew::default();
    let now = 100_000_000.0;
    let mut rows = vec![
        row(1, now - 2_000_000.0),
        row(2, now - 3_500_000.0),
        row(3, now - 6_000_000.0),
    ];
    rows.push(loose_row(4, 10_800_000.0, 30_000.0)); // sample = 3h - 30s
    rows.push(loose_row(5, 10_770_000.0, 30_000.0)); // sample = 3h - 60s

    let delta = skew.observe(&rows, |_| true, now);

    assert_eq!(delta, Some(-10_800_000.0));
    assert_eq!(skew.skew_ms(), 10_800_000.0);
}

/// `MIN_ABS_SKEW_MS` sits between credible NTP drift and the smallest real timezone offset; a
/// dropping-the-deadband mutation would adopt this 30-minute cluster instead of leaving an honest
/// core alone.
#[test]
fn a_thirty_minute_cluster_is_refused_by_the_deadband() {
    let mut skew = CoreClockSkew::default();
    let r1 = loose_row(1, 2_000_000.0, 200_000.0); // sample = 1_800_000
    let r2 = loose_row(2, 3_000_000.0, 1_200_000.0); // sample = 1_800_000

    let delta = skew.observe(&[r1, r2], |_| true, 0.0);

    assert_eq!(delta, None);
    assert_eq!(skew.skew_ms(), 0.0);
}

/// A skew this large cannot be real — `MAX_SKEW_MS` caps plausibility at 14h — so the sample must
/// never reach the vote count regardless of how many rows agree on it.
#[test]
fn a_forty_hour_sample_is_refused_by_the_plausibility_window() {
    let mut skew = CoreClockSkew::default();
    let sample = 40.0 * 3_600_000.0;
    let r1 = loose_row(1, sample, 0.0);
    let r2 = loose_row(2, sample, 0.0);

    let delta = skew.observe(&[r1, r2], |_| true, 0.0);

    assert_eq!(delta, None);
    assert_eq!(skew.skew_ms(), 0.0);
}

/// The estimator is sign-agnostic: a core lagging true UTC must adopt through the exact same
/// bucket-and-vote path as one leading it.
#[test]
fn a_negative_skew_adopts_through_the_same_path() {
    let mut skew = CoreClockSkew::default();
    // p0 must be > 1.0 so the new anchor rule still admits the sample.
    let r1 = loose_row(1, 1_000.0, 18_001_000.0); // sample = -5h
    let r2 = loose_row(2, 100_000.0, 18_100_000.0); // sample = -5h

    let delta = skew.observe(&[r1, r2], |_| true, 0.0);

    assert_eq!(delta, Some(18_000_000.0));
    assert_eq!(skew.skew_ms(), -18_000_000.0);
}

/// Un-learning: two TIGHT samples clustered at bucket zero must revoke a stale non-zero estimate in
/// either direction, and `observe`'s return value is the exact amount every already-retained wire
/// time now needs shifting by.
#[test]
fn two_tight_samples_at_zero_revoke_an_adopted_skew() {
    let mut skew = CoreClockSkew::default();
    let t0 = 1_000_000.0;
    let r1 = loose_row(1, 7_200_000.0, 0.0);
    let r2 = loose_row(2, 7_300_000.0, 100_000.0);
    assert_eq!(skew.observe(&[r1, r2], |_| true, t0), Some(-7_200_000.0));
    assert_eq!(skew.skew_ms(), 7_200_000.0);

    // Past WARMUP_MS, so a never-seen uid counts as TIGHT, not LOOSE.
    let t1 = t0 + 61_000.0;
    let rows = [row(3, t1), row(4, t1)];

    let delta = skew.observe(&rows, |_| false, t1);

    assert_eq!(delta, Some(7_200_000.0));
    assert_eq!(skew.skew_ms(), 0.0);
}

/// LOOSE evidence is a lower bound, never a reason to shrink the correction already in force — only
/// a TIGHT sample may do that (proven by the previous test).
#[test]
fn loose_samples_never_lower_an_adopted_skew() {
    let mut skew = CoreClockSkew::default();
    let t0 = 1_000_000.0;
    let r1 = loose_row(1, 7_200_000.0, 0.0);
    let r2 = loose_row(2, 7_300_000.0, 100_000.0);
    assert_eq!(skew.observe(&[r1, r2], |_| true, t0), Some(-7_200_000.0));
    assert_eq!(skew.skew_ms(), 7_200_000.0);

    // A plausible, above-deadband, but SMALLER-magnitude cluster (1h vs the adopted 2h).
    let r3 = loose_row(3, 3_600_000.0, 0.0);
    let r4 = loose_row(4, 3_700_000.0, 100_000.0);

    let delta = skew.observe(&[r3, r4], |_| true, t0 + 5_000.0);

    assert_eq!(delta, None);
    assert_eq!(skew.skew_ms(), 7_200_000.0);
}

/// Before `WARMUP_MS` has elapsed, a never-seen uid's sample must fall back to LOOSE — which cannot
/// revoke an existing estimate — rather than being read as a precise TIGHT point estimate.
#[test]
fn an_unseen_uid_before_warmup_is_treated_as_loose() {
    let mut skew = CoreClockSkew::default();
    let t0 = 1_000_000.0;
    let r1 = loose_row(1, 7_200_000.0, 0.0);
    let r2 = loose_row(2, 7_300_000.0, 100_000.0);
    assert_eq!(skew.observe(&[r1, r2], |_| true, t0), Some(-7_200_000.0));
    assert_eq!(skew.skew_ms(), 7_200_000.0);

    // Only 30s later — inside WARMUP_MS — with two brand-new uids sitting exactly at bucket zero.
    // Read as TIGHT this would revoke the estimate to 0.0, as in `two_tight_samples_at_zero...`.
    let t1 = t0 + 30_000.0;
    let rows = [row(3, t1), row(4, t1)];

    let delta = skew.observe(&rows, |_| false, t1);

    assert_eq!(
        delta, None,
        "a loose 0-bucket sample must not revoke the adopted skew"
    );
    assert_eq!(skew.skew_ms(), 7_200_000.0);
}

/// `correct` must touch only the leg-time scalars and each trace's OWN first point — the rest of
/// the trace, `tmp_point`, and `stop_time_ms` are already corrected by the core itself, and shifting
/// them again would double-correct the values that need no help.
#[test]
fn correct_shifts_only_the_scalars_and_first_trace_points() {
    let skew = CoreClockSkew {
        skew_ms: 7_200_000.0,
        first_seen_ms: None,
        generation: 0,
    };
    let mut r = row(1, 10_000_000.0);
    r.sell_create_time_ms = 9_000_000.0;
    r.entry_fill_time_ms = 8_000_000.0;
    r.buy_trace = Some(OrderTrace {
        points: vec![
            OrderTracePoint {
                time_ms: 10_000_000.0,
                price: 60_000.0,
            },
            OrderTracePoint {
                time_ms: 9_500_000.0,
                price: 60_100.0,
            },
        ],
        tmp_point: Some(OrderTracePoint {
            time_ms: 9_800_000.0,
            price: 60_050.0,
        }),
        stop_price: Some(59_000.0),
        stop_time_ms: Some(9_600_000.0),
    });
    let mut rows = [r];

    skew.correct(&mut rows);

    let r = &rows[0];
    assert_eq!(r.create_time_ms, 10_000_000.0 - 7_200_000.0);
    assert_eq!(r.sell_create_time_ms, 9_000_000.0 - 7_200_000.0);
    assert_eq!(r.entry_fill_time_ms, 8_000_000.0 - 7_200_000.0);
    let trace = r.buy_trace.as_ref().expect("buy trace must remain");
    assert_eq!(trace.points[0].time_ms, 10_000_000.0 - 7_200_000.0);
    assert_eq!(
        trace.points[1].time_ms, 9_500_000.0,
        "points[1..] is already corrected"
    );
    assert_eq!(
        trace.tmp_point.map(|p| p.time_ms),
        Some(9_800_000.0),
        "tmp_point is already corrected"
    );
    assert_eq!(
        trace.stop_time_ms,
        Some(9_600_000.0),
        "stop_time_ms is already corrected"
    );
}

/// `wire_line_start` and the rest of the pipeline treat `0.0` as "absent", not as the epoch —
/// `correct` must leave it there rather than shifting it into a bogus negative time.
#[test]
fn correct_leaves_absent_fields_at_zero() {
    let skew = CoreClockSkew {
        skew_ms: 7_200_000.0,
        first_seen_ms: None,
        generation: 0,
    };
    let mut rows = [row(1, 10_000_000.0)];
    assert_eq!(rows[0].sell_create_time_ms, 0.0);
    assert_eq!(rows[0].entry_fill_time_ms, 0.0);

    skew.correct(&mut rows);

    assert_eq!(rows[0].sell_create_time_ms, 0.0);
    assert_eq!(rows[0].entry_fill_time_ms, 0.0);
}

/// HIGH 4's pin, and the most important test in this file: `create_time_ms - now_ms` feeding LOOSE
/// unconditionally (the pre-amendment bug) would let a startup cohort of similarly-aged old orders
/// cluster into one bucket and cross `MIN_VOTES` with a value that is the cohort's AGE, not a real
/// skew, silently moving every line on an honest UTC core. All three uids are new AND this is the
/// very first `observe` call (unwarmed), which is exactly the dangerous case the module documents.
///
/// Magnitudes sit ABOVE `MIN_ABS_SKEW_MS` (45 min). A 15-minute cohort would be deadbanded to 0
/// even under the regression, so this would pass for the wrong reason.
#[test]
fn a_same_age_cohort_of_old_orders_on_a_utc_core_adopts_nothing() {
    let mut skew = CoreClockSkew::default();
    let now = 100_000_000.0;
    let rows = [
        row(1, now - 7_200_000.0),
        row(2, now - 7_250_000.0),
        row(3, now - 7_150_000.0),
    ];

    let delta = skew.observe(&rows, |_| false, now);

    assert_eq!(delta, None);
    assert_eq!(skew.skew_ms(), 0.0);
}

/// Distinct-uid voting: one order's buy and sell traces both landing in the same bucket must still
/// count as a single vote, so a lone order can never cross `MIN_VOTES` by itself no matter how many
/// trace samples it contributes.
#[test]
fn one_orders_buy_and_sell_traces_alone_cannot_adopt() {
    let mut skew = CoreClockSkew::default();
    let mut r = row(1, 7_200_000.0);
    r.sell_create_time_ms = 7_200_000.0;
    r.buy_trace = Some(trace(&[7_200_000.0, 0.0]));
    r.sell_trace = Some(trace(&[7_200_000.0, 0.0]));

    let delta = skew.observe(&[r], |_| true, 0.0);

    assert_eq!(delta, None);
    assert_eq!(skew.skew_ms(), 0.0);
}

/// moonproto shrinks a long trace from the FRONT once it outgrows its cap. After that,
/// `points[0]` is an ordinary already-corrected tick, not the raw create-time anchor.
///
/// `clock_skew.rs:CoreClockSkew::correct` dropping the `p0.time_ms == raw_create` guard would
/// push that tick one whole skew left of the candles.
#[test]
fn correct_leaves_a_shrunk_trace_first_point_alone() {
    let skew = CoreClockSkew {
        skew_ms: 7_200_000.0,
        first_seen_ms: None,
        generation: 0,
    };
    let create_raw = 10_000_000.0;
    let shrunk_p0 = 9_000_000.0;
    let mut r = row(1, create_raw);
    r.buy_trace = Some(OrderTrace {
        points: vec![
            OrderTracePoint {
                time_ms: shrunk_p0,
                price: 60_000.0,
            },
            OrderTracePoint {
                time_ms: 8_500_000.0,
                price: 60_100.0,
            },
        ],
        tmp_point: None,
        stop_price: None,
        stop_time_ms: None,
    });
    let mut rows = [r];

    skew.correct(&mut rows);

    assert_eq!(rows[0].create_time_ms, create_raw - 7_200_000.0);
    let trace = rows[0].buy_trace.as_ref().expect("buy trace must remain");
    assert_eq!(
        trace.points[0].time_ms, shrunk_p0,
        "a shrunk-from-the-front first point is already corrected and must not move"
    );
    assert_eq!(trace.points[1].time_ms, 8_500_000.0);
}

/// The same shrink: `points[0] - points[1]` is the gap between two already-corrected ticks, not a
/// skew sample. `clock_skew.rs:CoreClockSkew::observe` dropping `points[0].time_ms == raw_create`
/// would let a pair of shrunk traces adopt that gap as if it were a real offset.
#[test]
fn a_shrunk_trace_does_not_produce_a_loose_sample() {
    let mut skew = CoreClockSkew::default();
    let create = 20_000_000.0;
    let p0 = 7_200_000.0;
    let p1 = 0.0;
    let mut r1 = row(1, create);
    r1.buy_trace = Some(trace(&[p0, p1]));
    let mut r2 = row(2, create);
    r2.buy_trace = Some(trace(&[p0, p1]));

    let delta = skew.observe(&[r1, r2], |_| true, 0.0);

    assert_eq!(delta, None);
    assert_eq!(skew.skew_ms(), 0.0);
}
