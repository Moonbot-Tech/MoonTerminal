//! Regression tests for native market-history backfill claims.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::*;

/// Extreme fine and coarse prefetch candles cannot move the paused chart away from its visible prices.
#[test]
fn visible_fit_excludes_prefetched_extremes_and_includes_intersecting_coarse_bars() {
    let candle = |time, low, high| ChartCandle {
        t_open_ms: time,
        open: low,
        close: high,
        low,
        high,
        volume: 1.0,
        quote_volume: 1.0,
    };
    let mut cursor = ChartHistoryCursor::default();
    cursor.candle_series.rebuild(
        60_000,
        &[
            candle(0.0, 0.01, 0.9),
            candle(600_000.0, 0.142, 0.146),
            candle(660_000.0, 0.143, 0.147),
            candle(1_200_000.0, 0.02, 0.8),
        ],
        60_000,
        &[],
    );
    cursor.coarse_fill = vec![
        (candle(0.0, 0.001, 9.0), 300_000.0),
        (candle(900_000.0, 0.14, 0.149), 300_000.0),
    ];
    assert_eq!(
        visible_candle_fit(&cursor, 60_000, (600_000.0, 720_000.0), None),
        Some((0.142, 0.147))
    );
    assert_eq!(
        visible_candle_fit(&cursor, 60_000, (950_000.0, 960_000.0), None),
        Some((0.14, 0.149))
    );
    assert_eq!(
        visible_candle_fit(
            &cursor,
            60_000,
            (600_000.0, 720_000.0),
            Some((0.141, 0.148))
        ),
        Some((0.141, 0.148))
    );
    assert_eq!(
        visible_candle_fit(&cursor, 60_000, (2_000_000.0, 2_100_000.0), None),
        None
    );
}

/// `history.rs:wire_row_candle` re-inlining a base-volume estimate in either adapter, or passing
/// `false` at a call site, makes quote-denominated futures rows render as volume times price.
#[test]
fn wire_rows_preserve_quote_turnover_only_for_quote_denominated_sources() {
    let quote_wire = wire_row_candle(1_000.0, 90.0, 120.0, 80.0, 110.0, 12_000.0, true);
    let base_wire = wire_row_candle(2_000.0, 90.0, 120.0, 80.0, 110.0, 12_000.0, false);

    // OHLC4 is 100, calculated here rather than through the production helper.
    assert_eq!(quote_wire.quote_volume, 12_000.0);
    assert_eq!(quote_wire.volume, 120.0);
    assert_ne!(quote_wire.quote_volume, 12_000.0 * 100.0);
    assert_eq!(base_wire.volume, 12_000.0);
    assert_eq!(base_wire.quote_volume, 12_000.0 * 100.0);
}

/// `history.rs:native_backfill_due` must keep an elapsed retry due while a just-claimed or
/// clock-earlier retry remains blocked; changing the due comparison would either suppress missing
/// history or multiply requests against the MoonBot core.
#[test]
fn native_backfill_due_obeys_elapsed_retry_and_clock_order() {
    let now = Instant::now();
    let claimed = NativeBackfillAttempt {
        last_attempt: now,
        delay_s: HISTORY_RETRY_MIN_S,
        attempts: 1,
    };

    assert!(native_backfill_due(None, now));
    assert!(!native_backfill_due(Some(&claimed), now));
    assert!(native_backfill_due(
        Some(&claimed),
        now + Duration::from_secs(HISTORY_RETRY_MIN_S as u64),
    ));
    assert!(!native_backfill_due(
        Some(&claimed),
        now - Duration::from_secs(1),
    ));
}

/// `history.rs:history_retry_next_delay_s` must retain the documented 30, 60, 120, 240, 480, 600
/// sequence; removing its floor or cap would respectively hammer or silently starve history
/// recovery after a transient core disconnect.
#[test]
fn history_retry_delay_stays_floored_and_capped() {
    let actual = [
        None,
        Some(30),
        Some(60),
        Some(120),
        Some(240),
        Some(480),
        Some(600),
    ]
    .map(history_retry_next_delay_s);

    assert_eq!(actual, [30, 60, 120, 240, 480, 600, 600]);
}

/// `history.rs:history_retry_next_delay_s` must normalize a recorded zero into the retry band and
/// resume at the doubled 30-second floor; restarting below that schedule would distort recovery.
#[test]
fn history_retry_delay_normalizes_zero_to_the_doubled_floor() {
    let delay_s = history_retry_next_delay_s(Some(0));

    assert!(
        (HISTORY_RETRY_MIN_S..=HISTORY_RETRY_MAX_S).contains(&delay_s)
            && delay_s == HISTORY_RETRY_MIN_S * 2,
        "a recorded zero must resume the doubled floor inside the retry band"
    );
}

/// `history.rs:history_retry_next_delay_s` must cap a u32::MAX prior delay without overflowing;
/// losing that guard would crash the terminal's retry path instead of safely preserving history.
#[test]
fn history_retry_delay_handles_u32_max_without_overflow() {
    assert_eq!(history_retry_next_delay_s(Some(u32::MAX)), 600);
}

/// `history.rs:NativeBackfillGate::claim` must spend exactly five claims before refusing a key;
/// removing the budget would keep an unfillable market consuming the core's exchange-request
/// allowance indefinitely.
#[test]
fn native_backfill_gate_spends_the_five_claim_budget() {
    let gate = NativeBackfillGate::default();
    let key = (7, "BTCUSDT".to_string(), 60);
    let mut now = Instant::now();

    for _ in 0..NATIVE_BACKFILL_MAX_ATTEMPTS {
        let delay_s = gate
            .claim(key.clone(), now)
            .expect("an unspent key must grant its next claim");
        now += Duration::from_secs(delay_s as u64);
    }

    assert_eq!(gate.claim(key, now), None);
}

/// `history.rs:NativeBackfillGate::{claim,forget_provider,retain_providers,clear}` must claim before
/// queueing and restore only dropped providers' budgets; delaying the claim or skipping a reset
/// would respectively duplicate a request across panels or leave a replacement core without history.
#[test]
fn native_backfill_gate_claims_once_and_scopes_lifecycle_resets() {
    let gate = NativeBackfillGate::default();
    let provider_a_key = (7, "BTCUSDT".to_string(), 60);
    let provider_b_key = (8, "ETHUSDT".to_string(), 60);
    let now = Instant::now();

    assert_eq!(gate.claim(provider_a_key.clone(), now), Some(30));
    assert_eq!(gate.claim(provider_a_key.clone(), now), None);
    assert_eq!(gate.claim(provider_b_key.clone(), now), Some(30));

    gate.forget_provider(7);
    assert_eq!(gate.claim(provider_a_key.clone(), now), Some(30));
    assert_eq!(gate.claim(provider_b_key.clone(), now), None);

    let mut keep = HashSet::new();
    keep.insert(7);
    gate.retain_providers(&keep);
    assert_eq!(gate.claim(provider_a_key.clone(), now), None);
    assert_eq!(gate.claim(provider_b_key.clone(), now), Some(30));

    gate.clear();
    assert_eq!(gate.claim(provider_a_key, now), Some(30));
    assert_eq!(gate.claim(provider_b_key, now), Some(30));
}

/// A partial hourly candle fits its uploaded wick, without including distant prefetched candles.
#[test]
fn fixture_fit_uses_complete_uploaded_boundary_candles() {
    let candle = |time, low, high| ChartCandle {
        t_open_ms: time,
        open: low,
        close: low,
        low,
        high,
        volume: 1.0,
        quote_volume: 1.0,
    };
    let rows = [
        candle(0.0, 1.0, 1000.0),
        candle(3_600_000.0, 90.0, 150.0),
        candle(7_200_000.0, 2.0, 2000.0),
    ];
    assert_eq!(
        fixture_visible_fit(&rows, 3_600_000, (3_900_000.0, 4_200_000.0)),
        Some((90.0, 150.0))
    );
    assert_eq!(
        fixture_visible_fit(&rows, 3_600_000, (10_800_000.0, 11_000_000.0)),
        None
    );
}

/// `history.rs:visible_candle_fit` starting its coarse scan without the `coarse_fill_max_tf`
/// widening drops a coarse filler that opened before the window but still overhangs its left
/// edge, so auto-Y ignores a bar drawn on screen. Compared with a full scan of the fill.
#[test]
fn visible_fit_windowed_coarse_scan_equals_a_full_scan() {
    let candle = |time: f64, low: f32, high: f32| ChartCandle {
        t_open_ms: time,
        open: low,
        close: high,
        low,
        high,
        volume: 1.0,
        quote_volume: 1.0,
    };
    let mut state = 0xDEAD_BEEF_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    // Ascending mixed-width fill: hourly bars with 5-minute bars between them.
    let hour = 3_600_000.0_f64;
    let five = 300_000.0_f64;
    let mut fill = Vec::new();
    let mut t = 0.0;
    while fill.len() < 3_000 {
        let lo = 1.0 + (next() % 1_000) as f32;
        let hi = lo + (next() % 50) as f32;
        let tf = if next() % 3 == 0 { hour } else { five };
        fill.push((candle(t, lo, hi), tf as f32));
        t += tf;
    }
    let hourly: Vec<f64> = fill
        .iter()
        .filter(|(_, tf)| f64::from(*tf) == hour)
        .map(|(c, _)| c.t_open_ms)
        .collect();
    let mut cursor = ChartHistoryCursor::default();
    cursor.coarse_fill = fill.clone();
    cursor.coarse_fill_max_tf = Some(hour);
    let last = t;
    let mut overhang_hits = 0;
    for q in 0..3_000 {
        let (a, b) = if q % 3 == 0 {
            // Start strictly inside an hourly bar: it opens before the window and overhangs it.
            let open = hourly[(next() % hourly.len() as u64) as usize];
            let a = open + 1.0 + (next() % (hour as u64 - 1)) as f64;
            (a, a + (next() % 5) as f64 * five)
        } else {
            let a = (next() % last as u64) as f64;
            (a, a + (next() % 40) as f64 * five)
        };
        let mut want: Option<(f32, f32)> = None;
        for (c, tf) in &fill {
            let tf = f64::from(*tf);
            if tf > 60_000.0 && c.t_open_ms + tf > a && c.t_open_ms <= b {
                if c.t_open_ms < a {
                    overhang_hits += 1;
                }
                want = Some(match want {
                    Some((l, h)) => (l.min(c.low), h.max(c.high)),
                    None => (c.low, c.high),
                });
            }
        }
        take_visible_fit_visited();
        assert_eq!(
            visible_candle_fit(&cursor, 60_000, (a, b), None),
            want,
            "window [{a}, {b}]"
        );
        let visited = take_visible_fit_visited();
        let bound = (b - a) / five + hour / five + 2.0;
        assert!(visited as f64 <= bound, "visited {visited} > {bound}");
        if q % 1_000 == 0 {
            println!("[visible_fit] before={} after={visited}", fill.len());
        }
    }
    assert!(overhang_hits > 0);
}

/// Polls the prefix of a cursor that has never loaded one, against a cache whose worker never
/// answers, so every poll after the first finds the read still in flight.
fn poll_stalled(cursor: &mut ChartHistoryCursor, cache: &crate::market::kline_cache::KlineCache) {
    poll_cache_prefix(
        cursor,
        Some(cache),
        Some("binance"),
        "BTCUSDT",
        1,
        1_000_000,
        60_000,
        2_000_000,
    );
}

/// Breakage: `kline_cache.rs` `PendingPrefixRead::poll` turned from `try_recv()` into a
/// `recv_timeout(READ_TIMEOUT)` wait. Consequence: every chart frame with a read in flight blocks
/// the UI thread for up to 250 ms again instead of drawing the rows it already holds.
#[test]
fn prefix_poll_with_a_read_in_flight_never_waits_on_the_worker() {
    let (cache, ops) = crate::market::kline_cache::KlineCache::stalled_for_tests();
    let mut cursor = ChartHistoryCursor::default();
    poll_stalled(&mut cursor, &cache);
    let generation = cursor.cache_generation;

    let started = Instant::now();
    for _ in 0..4 {
        poll_stalled(&mut cursor, &cache);
    }
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_millis(200),
        "4 polls of a stalled read took {elapsed:?}; the frame path must not wait"
    );
    assert!(cursor.cache_pending.is_some(), "the read stays in flight");
    assert!(cursor.cache_rows.is_empty());
    assert_eq!(cursor.cache_generation, generation);
    assert_eq!(ops.try_iter().count(), 1, "exactly one read was queued");
}

/// Breakage: `source/mod.rs` `ChartHistoryCursor::invalidate_cache_prefix` stops dropping
/// `cache_pending`. Consequence: a read queued before a cache merge answers with the pre-merge
/// rows and marks the prefix fresh, so the merged rows are never drawn this session.
#[test]
fn invalidating_the_prefix_drops_the_read_in_flight_and_asks_again() {
    let (cache, ops) = crate::market::kline_cache::KlineCache::stalled_for_tests();
    let mut cursor = ChartHistoryCursor::default();
    poll_stalled(&mut cursor, &cache);

    cursor.invalidate_cache_prefix();
    assert!(cursor.cache_pending.is_none(), "the pre-merge read is gone");
    poll_stalled(&mut cursor, &cache);

    assert_eq!(
        ops.try_iter().count(),
        2,
        "a fresh read follows the invalidation"
    );
}

/// Breakage: `history.rs` `series_floor` loses its hysteresis and re-anchors on every call.
/// Consequence: every pan past the 20% prefetch rebuilds the whole candle series again.
/// Numbers: rebuilds per 100 in-span pans were 100 before the hysteresis, 0 with it.
#[test]
fn pans_inside_the_retained_span_never_rebuild_the_series() {
    let span = 3_600_000i64;
    let want0 = 10_000_000_000i64;
    let mut floor = series_floor(i64::MAX, want0, span);
    assert_eq!(floor, want0 - span);
    let reset_for = |old: i64, new: i64| {
        series_reset_due(&SeriesResetInputs {
            params_reset: false,
            valid: true,
            tf_changed: false,
            trades_newly_available: false,
            deep_sig_changed: false,
            exchange_key_changed: false,
            floor_moved: new != old,
            cache_arrived: false,
        })
    };

    let mut resets = 0;
    for k in 0..100i64 {
        // 50 steps right by 1/50 span, then 50 back left to 0.9 span before the start.
        let want = if k < 50 {
            want0 + k * span / 50
        } else {
            want0 + span - (k - 49) * (span * 19 / 10) / 50
        };
        assert!(want >= floor && want <= floor + 4 * span);
        let next = series_floor(floor, want, span);
        if reset_for(floor, next) {
            resets += 1;
        }
        floor = next;
    }
    assert_eq!(resets, 0, "no rebuild while the view stays in the span");

    let next = series_floor(floor, floor - 1, span);
    assert!(
        reset_for(floor, next),
        "one pan past the floor rebuilds once"
    );
    assert_eq!(next, floor - 1 - span);
}
