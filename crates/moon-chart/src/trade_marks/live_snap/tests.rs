//! Regressions for price-preserving live display-time estimates.

use super::LiveTradeSnap;
use crate::trade_marks::{TapePrint, TradeMark};

/// A short with report prices that do not have exact binary representations.
fn short() -> TradeMark {
    TradeMark {
        buy_ms: 31_000,
        close_ms: 32_000,
        buy_price: 0.14275000000000002,
        sell_price: 0.14142000000000005,
        qty: 10.0,
        is_short: true,
    }
}

/// Build a feed-precision public print, as the live renderer receives it.
fn tick(t_ms: i64, price: f64) -> TapePrint {
    TapePrint {
        t_ms,
        price: f64::from(price as f32),
    }
}

/// Losing subsecond placement or replacing execution prices with f32 tape prices breaks this.
#[test]
fn matching_ticks_move_only_time_and_late_batches_improve_it() {
    let original = short();
    let mut snap = LiveTradeSnap::default();
    snap.set_marks(&[original]);
    assert_eq!(snap.marks(), vec![original]);
    assert!(snap.observe([tick(31_766, 0.14275)]));
    assert!(snap.observe([tick(32_650, 0.14142)]));
    let expected = TradeMark {
        buy_ms: 31_766,
        close_ms: 32_650,
        ..original
    };
    assert_eq!(snap.marks(), vec![expected]);
    assert!(!snap.observe([tick(33_010, 0.14275), tick(31_900, 0.14275)]));
    assert!(!snap.set_marks(&[original]));
    assert_eq!(snap.marks(), vec![expected]);
    assert!(snap.observe([tick(31_700, 0.14275)]));
    assert_eq!(snap.marks()[0].buy_ms, 31_700);
}

/// A previous/next second, adjacent price level or invalid print must not invent a match.
#[test]
fn uncertain_prices_and_neighbouring_seconds_keep_report_coordinates() {
    let mut snap = LiveTradeSnap::default();
    snap.set_marks(&[short()]);
    assert!(!snap.observe([
        tick(30_999, 0.14275),
        tick(32_000, 0.14275),
        tick(31_800, 0.14276),
        tick(32_100, 0.14143),
        tick(31_100, f64::NAN),
        tick(31_200, f64::INFINITY),
    ]));
    assert_eq!(snap.marks(), vec![short()]);
}

/// Independently matching the two ends must never draw an exit before the entry.
#[test]
fn same_second_trade_rejects_reversed_or_incomplete_estimates() {
    let original = TradeMark {
        close_ms: 31_000,
        ..short()
    };
    let mut snap = LiveTradeSnap::default();
    snap.set_marks(&[original]);
    snap.observe([tick(31_800, 0.14275)]);
    assert_eq!(snap.marks(), vec![original]);
    snap.observe([tick(31_100, 0.14142)]);
    assert_eq!(snap.marks(), vec![original]);
    snap.observe([tick(31_050, 0.14275)]);
    assert_eq!(
        snap.marks(),
        vec![TradeMark {
            buy_ms: 31_050,
            close_ms: 31_100,
            ..original
        }]
    );
}

/// New report rows retain older estimates after tape eviction; revised prices retire their match.
#[test]
fn report_updates_preserve_only_unchanged_keys_and_storage_stays_bounded() {
    let mut snap = LiveTradeSnap::default();
    snap.set_marks(&[short()]);
    snap.observe([tick(31_766, 0.14275)]);
    let newer = TradeMark {
        buy_ms: 40_000,
        close_ms: 41_000,
        ..short()
    };
    snap.set_marks(&[short(), newer]);
    assert_eq!(snap.marks()[0].buy_ms, 31_766);
    snap.set_marks(&[TradeMark {
        buy_price: 0.15,
        ..short()
    }]);
    assert_eq!(snap.marks()[0].buy_ms, 31_000);
    assert_eq!(snap.matches.len(), 2);
    snap.set_marks(&[]);
    assert!(snap.matches.is_empty());
    assert!(!snap.observe([tick(31_700, 0.15)]));
}

/// An already precise timestamp must not be overwritten by another record's matching key.
#[test]
fn precise_stamps_and_fresh_panes_do_not_inherit_estimates() {
    let precise = TradeMark {
        buy_ms: 31_500,
        ..short()
    };
    let mut snap = LiveTradeSnap::default();
    snap.set_marks(&[short(), precise]);
    snap.observe([tick(31_766, 0.14275)]);
    assert_eq!(snap.marks()[1].buy_ms, 31_500);
    let mut other_pane = LiveTradeSnap::default();
    other_pane.set_marks(&[short()]);
    assert_eq!(other_pane.marks(), vec![short()]);
}

/// Provider replacement retires old estimates but accepts the first replacement batch immediately.
#[test]
fn replacement_source_accepts_first_batch_without_republishing_report() {
    let mut snap = LiveTradeSnap::default();
    snap.set_marks(&[short()]);
    snap.observe([tick(31_100, 0.14275)]);
    snap.reset_matches();
    assert_eq!(snap.marks(), vec![short()]);
    assert!(snap.observe([tick(31_800, 0.14275)]));
    assert_eq!(snap.marks()[0].buy_ms, 31_800);
}

/// Exact source timestamps must survive large epoch offsets that f32 GPU instances cannot retain.
#[test]
fn large_epoch_offsets_do_not_move_a_print_across_second_boundary() {
    let original = TradeMark {
        buy_ms: 1_700_086_401_000,
        close_ms: 1_700_086_402_000,
        ..short()
    };
    let mut snap = LiveTradeSnap::default();
    snap.set_marks(&[original]);
    assert!(!snap.observe([tick(1_700_086_400_999, 0.14275)]));
    snap.observe([tick(1_700_086_401_001, 0.14275)]);
    assert_eq!(snap.marks()[0].buy_ms, 1_700_086_401_001);
}

/// A successful but empty seed must be retryable when an archive arrives with crosses hidden.
#[test]
fn archive_requests_seed_without_discarding_previously_resolved_ends() {
    let mut snap = LiveTradeSnap::default();
    snap.set_marks(&[short()]);
    snap.seed_complete();
    assert!(!snap.needs_seed());
    snap.observe([tick(31_766, 0.14275)]);
    snap.request_seed();
    assert!(snap.needs_seed());
    assert_eq!(snap.marks()[0].buy_ms, 31_766);
    snap.observe([tick(32_650, 0.14142)]);
    snap.seed_complete();
    assert!(!snap.needs_seed());
    snap.reset_matches();
    assert!(snap.needs_seed());
}
