//! The window-delta caption uses the same read clock as volume and arbitrage.

use super::*;

/// Breakage: `retained_caption_due` is true on every call, or false for a market that just changed.
/// Consequence: a WindowDelta caption either walks retained history on every feed revision, or
/// keeps the previous coin's percentages after a switch.
#[test]
fn retained_caption_due_matches_the_arb_clock() {
    assert!(retained_caption_due(0, 1_000, "", "COIN_A"));
    assert!(!retained_caption_due(1_000, 1_249, "COIN_A", "COIN_A"));
    assert!(retained_caption_due(1_000, 1_250, "COIN_A", "COIN_A"));
    assert!(retained_caption_due(1_200, 1_300, "COIN_A", "COIN_B"));
    let market = include_str!("../../market.rs");
    let clock = include_str!("../window.rs");
    assert!(
        market.contains("retained_caption_due("),
        "the chart sync must throttle market_windows with this clock"
    );
    assert!(
        market.contains("apply_window_pull("),
        "the chart sync must store the pull through the shared assignment"
    );
    assert!(
        clock.contains("WindowPull::Hold => {}"),
        "a skipped read must keep the previous window-delta figures"
    );
}

/// Breakage: a same-market read that returns no snapshot keeps the previous percentages.
/// Consequence: a dropped provider leaves WindowDelta figures on the chart indefinitely.
#[test]
fn a_missing_snapshot_clears_the_same_markets_figures() {
    let mut figures = Some(9u32);
    let mut read_ms = 1_000i64;
    let mut market = "COIN_A".to_string();
    apply_window_pull(
        &mut figures,
        &mut read_ms,
        &mut market,
        WindowPull::Read {
            market: "COIN_A".to_string(),
            value: None,
        },
        2_000,
    );
    assert_eq!(figures, None);
    assert_eq!(read_ms, 2_000);
    assert_eq!(market, "COIN_A");

    figures = Some(4);
    apply_window_pull(
        &mut figures,
        &mut read_ms,
        &mut market,
        WindowPull::Hold,
        2_100,
    );
    assert_eq!(figures, Some(4));
    assert_eq!(read_ms, 2_000);

    apply_window_pull(
        &mut figures,
        &mut read_ms,
        &mut market,
        WindowPull::Read {
            market: "COIN_A".to_string(),
            value: Some(8),
        },
        2_500,
    );
    assert_eq!(figures, Some(8));

    apply_window_pull(
        &mut figures,
        &mut read_ms,
        &mut market,
        WindowPull::Off,
        2_600,
    );
    assert_eq!(figures, None);
    assert_eq!(read_ms, 0);
    assert!(market.is_empty());
}

/// Scale probe: how many window-delta reads a revision storm still makes.
/// Ignored: it only counts the clock, and prints the before/after read counts.
/// The derived-snapshot walk itself is outside this crate.
#[test]
#[ignore = "scale probe: window-delta reads across a 1000-revision storm"]
fn window_delta_scale_probe() {
    const REVS: i64 = 1_000;
    const STEP_MS: i64 = 10;
    let mut reads = 0i64;
    let mut last_ms = 0i64;
    let mut last_market = "";
    for step in 0..REVS {
        let now = step * STEP_MS;
        if retained_caption_due(last_ms, now, last_market, "COIN_A") {
            reads += 1;
            last_ms = now;
            last_market = "COIN_A";
        }
    }
    println!(
        "window_delta_scale revisions {REVS} step_ms {STEP_MS} old_reads {REVS} new_reads {reads}"
    );
    assert!(reads > 1);
    assert!(reads < REVS / 2);
}
