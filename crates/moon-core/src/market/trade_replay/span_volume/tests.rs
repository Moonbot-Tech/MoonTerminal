//! A replay's traded value over a period: prints inside the coverage, bars outside, no
//! millisecond counted twice.

use super::*;
use crate::market::candles::ChartCandle;
use crate::market::source::SideSlot;
use crate::market::trade_replay::{
    Coverage, ReplayWindow, TickStatus, TickThinning, TradeReplaySource,
};

const MINUTE_MS: i64 = 60_000;

/// A one-minute bar turning over 6000 quote, 100 a second.
fn bar(open_ms: i64) -> ChartCandle {
    ChartCandle {
        t_open_ms: open_ms as f64,
        open: 1.0,
        high: 1.1,
        low: 0.9,
        close: 1.05,
        volume: 6_000.0,
        quote_volume: 6_000.0,
    }
}

fn slot(id: i64, buy: f64, sell: f64) -> SideSlot {
    SideSlot { id, buy, sell }
}

/// Three minutes of bars, prints walked over `[60.5 s, 120 s)`: the walk's start falls inside
/// second 60, whose slot therefore holds only that second's last half.
fn series(side_slots: Vec<SideSlot>) -> TradeReplaySeries {
    TradeReplaySeries {
        source: TradeReplaySource::CoreTicks,
        venue: None,
        window: ReplayWindow {
            from_ms: 0,
            to_ms: 3 * MINUTE_MS,
            open_ms: 60_500,
            close_ms: 119_999,
            margin_ms: 0,
            long_position_ms: 5 * MINUTE_MS,
            over_budget: false,
        },
        tf_ms: MINUTE_MS,
        candles: vec![bar(0), bar(MINUTE_MS), bar(2 * MINUTE_MS)],
        ticks: Vec::new(),
        identity: 1358,
        tick_status: TickStatus::Served,
        thinning: TickThinning::Raw,
        partial: true,
        side_slots,
        covered: Coverage::one((60_500, 119_999)),
    }
}

fn prints() -> Vec<SideSlot> {
    vec![slot(60, 10.0, 0.0), slot(70, 5.0, 3.0), slot(100, 0.0, 7.0)]
}

/// On whole seconds inside the coverage the sides are the slots' own sums — measured, no bar
/// share added, and a quiet covered stretch is a real zero rather than no answer.
#[test]
fn whole_seconds_on_the_prints_are_measured() {
    let s = series(prints());
    let v = s.volume_between(61_000, 101_000).unwrap();
    assert_eq!((v.buy_quote, v.sell_quote, v.bars_quote), (5.0, 10.0, 0.0));
    assert_eq!(v.fit, ReplayVolumeFit::Exact);

    let quiet = s.volume_between(80_000, 90_000).unwrap();
    assert_eq!(quiet.total_quote(), 0.0);
    assert_eq!(quiet.fit, ReplayVolumeFit::Exact);
}

/// A cut through a traded second takes its time share and says so; a cut through a second that
/// did not trade changes nothing and stays exact.
#[test]
fn a_cut_traded_second_is_an_estimate() {
    let s = series(prints());
    let v = s.volume_between(70_250, 80_000).unwrap();
    assert_eq!((v.buy_quote, v.sell_quote), (3.75, 2.25));
    assert_eq!(v.fit, ReplayVolumeFit::CutSecond);

    let quiet_cut = s.volume_between(80_250, 90_500).unwrap();
    assert_eq!(quiet_cut.fit, ReplayVolumeFit::Exact);
}

/// The coverage's first second holds prints for its covered half only: taking that half whole is
/// exact, taking half of it is a half share.
#[test]
fn the_edge_second_is_shared_against_its_covered_part() {
    let s = series(prints());
    let whole = s.volume_between(60_500, 61_000).unwrap();
    assert_eq!((whole.buy_quote, whole.fit), (10.0, ReplayVolumeFit::Exact));
    let half = s.volume_between(60_750, 61_000).unwrap();
    assert_eq!(
        (half.buy_quote, half.fit),
        (5.0, ReplayVolumeFit::CutSecond)
    );
}

/// Reaching past the coverage adds the bars' share over the uncovered milliseconds only. The
/// boundary second's covered half is the prints', its other half the bar's — the band counts that
/// second from both, this must not.
#[test]
fn bars_answer_only_the_uncovered_milliseconds() {
    let s = series(prints());
    let v = s.volume_between(50_000, 61_000).unwrap();
    assert_eq!(v.buy_quote, 10.0);
    // Ten seconds of the first bar (1000) plus the uncovered half second of the second (50).
    assert!((v.bars_quote - 1_050.0).abs() < 1e-9, "{}", v.bars_quote);
    assert_eq!(v.fit, ReplayVolumeFit::Bars);
}

/// No slots — no prints, or a contract size this build cannot value — hands the covered stretch
/// back to the bars, the way the volume band falls back.
#[test]
fn without_slots_the_bars_answer_everywhere() {
    let s = series(Vec::new());
    let v = s.volume_between(61_000, 62_000).unwrap();
    assert!((v.bars_quote - 100.0).abs() < 1e-9, "{}", v.bars_quote);
    assert_eq!((v.buy_quote, v.sell_quote), (0.0, 0.0));
    assert_eq!(v.fit, ReplayVolumeFit::Bars);
}

/// A period no bar and no print reaches has no answer, and neither has an empty one.
#[test]
fn a_period_nothing_reaches_has_no_answer() {
    let s = series(prints());
    assert_eq!(s.volume_between(500_000, 510_000), None);
    assert_eq!(s.volume_between(70_000, 70_000), None);
}
