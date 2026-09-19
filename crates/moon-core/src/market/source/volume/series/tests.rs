use super::*;
use crate::market::trade_replay::venue_caps::TickValue;

/// A trade as the retained ring holds one: a SELL is spelled by the quantity's sign bit.
fn trade(ms: i64, price: f32, qty: f32) -> TradeHistoryRow {
    TradeHistoryRow {
        time: MoonTime::from_unix_millis(ms),
        price,
        qty,
    }
}

fn mini(ms: i64, buy: f32, sell: f32) -> MiniCandle {
    MiniCandle {
        time: MoonTime::from_unix_millis(ms),
        cnt: 1,
        min_price: 1.0,
        max_price: 2.0,
        buy_vol: buy,
        sell_vol: sell,
    }
}

fn series() -> SideSeries {
    let mut s = SideSeries::new(0);
    s.seeded = true;
    s
}

/// A sale lands on the selling side as a positive amount; the sign bit is the side, not a value.
#[test]
fn a_sale_is_a_positive_amount_on_the_selling_side() {
    let mut s = series();
    s.add_trade(trade(1_000, 100.0, 2.0));
    s.add_trade(trade(1_500, 100.0, -3.0));
    assert_eq!(s.slots.len(), 1);
    assert_eq!(s.slots[0].buy, 200.0);
    assert_eq!(s.slots[0].sell, 300.0);
}

/// Rows in time order extend the vector; an out-of-order row still lands in its own slot, in
/// order, rather than after the newest.
#[test]
fn out_of_order_rows_keep_the_slots_sorted() {
    let mut s = series();
    s.add_trade(trade(0, 1.0, 1.0));
    s.add_trade(trade(20_000, 1.0, 1.0));
    s.add_trade(trade(10_000, 1.0, 1.0));
    s.add_trade(trade(10_500, 1.0, 1.0));
    let ids: Vec<i64> = s.slots.iter().map(|x| x.id).collect();
    assert_eq!(ids, vec![0, 10, 20]);
    assert_eq!(s.slots[1].buy, 2.0);
}

/// A mini-candle with a NaN side contributes nothing on that side, not a NaN that poisons the
/// bucket it lands in.
#[test]
fn a_nan_aggregate_side_is_folded_as_zero() {
    let mut s = series();
    s.add_mini(mini(0, f32::NAN, 5.0));
    assert_eq!(s.slots[0].buy, 0.0);
    assert_eq!(s.slots[0].sell, 5.0);
}

/// Each sample carries the sums over the window that ENDS where the sample ends, so a print stays
/// in the picture for a whole window after it happened and leaves exactly one window later.
#[test]
fn rolling_keeps_a_print_for_one_window_after_it() {
    let slots = vec![Slot {
        id: 10, // 10 s
        buy: 4.0,
        sell: 1.0,
    }];
    let mut out = Vec::new();
    rolling_slots(&slots, 15_000, 5_000, 0, 60_000, &mut out);
    let opens: Vec<i64> = out.iter().map(|b| b.t_open_ms).collect();
    // The sample [10 s, 15 s) is the first whose window (0 s, 15 s] holds the print; the sample
    // [20 s, 25 s) has window (10 s, 25 s], still holding it; [25 s, 30 s) has (15 s, 30 s], not.
    assert_eq!(opens, vec![10_000, 15_000, 20_000]);
    assert!(
        out.iter()
            .all(|b| b.buy_quote == 4.0 && b.sell_quote == 1.0)
    );
    assert!(out.iter().all(|b| b.tf_ms == 5_000));
}

/// Inside the window the sums accumulate; a coarser step samples the same rolling sums less often
/// and aligns its samples to the step.
#[test]
fn rolling_sums_accumulate_and_a_coarse_step_samples_less_often() {
    let slots: Vec<Slot> = (0..12)
        .map(|i| Slot {
            id: i * 5, // 0 s .. 55 s, one per five seconds
            buy: 1.0,
            sell: 0.0,
        })
        .collect();
    let mut out = Vec::new();
    rolling_slots(&slots, 60_000, 5_000, 0, 55_000, &mut out);
    let sums: Vec<f32> = out.iter().map(|b| b.buy_quote).collect();
    assert_eq!(sums, (1..=12).map(|n| n as f32).collect::<Vec<_>>());

    rolling_slots(&slots, 60_000, 20_000, 0, 55_000, &mut out);
    let opens: Vec<i64> = out.iter().map(|b| b.t_open_ms).collect();
    assert_eq!(opens, vec![0, 20_000, 40_000]);
    let sums: Vec<f32> = out.iter().map(|b| b.buy_quote).collect();
    assert_eq!(sums, vec![4.0, 8.0, 12.0]);
    assert!(out.iter().all(|b| b.tf_ms == 20_000));
}

/// Slots older than the first window never enter it, and a print inside the window before the
/// range starts is still counted by the range's first samples.
#[test]
fn rolling_counts_prints_just_before_the_range() {
    let slots = vec![
        Slot {
            id: 0, // 0 s: older than any window of the range
            buy: 100.0,
            sell: 0.0,
        },
        Slot {
            id: 55, // 55 s: inside the first sample's window (60 s + 5 s - 15 s = 50 s ..)
            buy: 2.0,
            sell: 0.0,
        },
    ];
    let mut out = Vec::new();
    rolling_slots(&slots, 15_000, 5_000, 60_000, 70_000, &mut out);
    let opens: Vec<i64> = out.iter().map(|b| b.t_open_ms).collect();
    assert_eq!(opens, vec![60_000, 65_000]);
    assert!(out.iter().all(|b| b.buy_quote == 2.0));
}

/// A step wider than the interval widens the interval to the step: every print lands in some
/// sample, none falls between two windows.
#[test]
fn rolling_never_leaves_a_gap_between_windows() {
    // Prints at 12 s and 17 s; interval 5 s, step 20 s. With the interval as asked the sample
    // opening at 0 s would cover (15 s, 20 s] only and the print at 12 s would be nowhere.
    let slots = vec![
        Slot {
            id: 12,
            buy: 1.0,
            sell: 0.0,
        },
        Slot {
            id: 17,
            buy: 2.0,
            sell: 0.0,
        },
    ];
    let mut out = Vec::new();
    rolling_slots(&slots, 5_000, 20_000, 0, 20_000, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].t_open_ms, 0);
    assert_eq!(out[0].buy_quote, 3.0);
}

/// Empty stretches emit nothing: a run quiet for longer than the window is absent, not zeros.
#[test]
fn rolling_emits_only_samples_with_something_in_them() {
    let slots = vec![
        Slot {
            id: 0,
            buy: 1.0,
            sell: 0.0,
        },
        Slot {
            id: 500,
            buy: 1.0,
            sell: 0.0,
        },
    ];
    let mut out = Vec::new();
    rolling_slots(&slots, 5_000, 5_000, 0, 1_000_000, &mut out);
    assert_eq!(out.len(), 2);
    assert_eq!(out[1].t_open_ms, 500_000);
}

/// A width or step below the native one cannot be served finer than the data: both read as one
/// slot, and the rest of the arithmetic still holds.
#[test]
fn rolling_never_goes_below_the_native_width() {
    let slots = vec![Slot {
        id: 15,
        buy: 1.0,
        sell: 1.0,
    }];
    let mut out = Vec::new();
    rolling_slots(&slots, 300, 300, 0, 100_000, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].tf_ms, SIDE_BUCKET_MS);
    assert_eq!(out[0].t_open_ms, 15_000);
}

/// An inverted window is empty rather than a panic or a walk of the whole series.
#[test]
fn rolling_of_an_inverted_window_is_empty() {
    let slots = vec![Slot {
        id: 15,
        buy: 1.0,
        sell: 1.0,
    }];
    let mut out = vec![SideVolumeBucket::default()];
    rolling_slots(&slots, 5_000, 5_000, 100_000, 0, &mut out);
    assert!(out.is_empty());
}

/// The core's archive prepends rows older than the cursor, so a changed archive revision must
/// rebuild the series; the same revision must not. Both rings absent here: the rebuild is judged
/// by what happens to the slots, not by what a ring would refill.
#[test]
fn a_changed_archive_revision_rebuilds_the_series() {
    let mut s = SideSeries::new(0);
    s.advance(None, None, 1, 0);
    assert!(s.seeded);
    s.add(10, 1.0, 0.0);
    // Same revision: the slots survive.
    s.advance(None, None, 1, 5_000);
    assert_eq!(s.slots.len(), 1);
    // A merged archive: the series starts over (and, with rings, would reseed from them).
    s.advance(None, None, 2, 10_000);
    assert!(s.slots.is_empty());
    assert!(
        s.seeded,
        "re-seeded under the new revision, even from nothing"
    );
    assert_eq!(s.archive_rev, 2);
}

/// A series never outgrows its cap: the oldest slots go, the newest stay, and the order holds.
#[test]
fn trim_drops_the_oldest_slots_past_the_cap() {
    let mut s = series();
    for i in 0..(MAX_SLOTS as i64 + 10) {
        s.add(i, 1.0, 0.0);
    }
    s.trim();
    assert_eq!(s.slots.len(), MAX_SLOTS);
    assert_eq!(s.slots[0].id, 10);
    assert_eq!(s.slots.last().map(|x| x.id), Some(MAX_SLOTS as i64 + 9));
}

/// The bench split keeps the candle's whole turnover: buy plus sell is the figure it started from,
/// sampled once per minute over a one-minute window.
#[test]
fn synthetic_sides_preserve_the_candle_turnover() {
    let candle = crate::market::ChartCandle {
        t_open_ms: 60_000.0,
        open: 1.0,
        high: 2.0,
        low: 0.5,
        close: 1.5,
        volume: 10.0,
        quote_volume: 100.0,
    };
    // A one-minute interval sampled once a minute: the sample opening at the candle's end holds
    // the whole candle; the one at its open holds nothing of it yet.
    let out = synthetic_sides(&[candle], 60_000, 60_000, 0, 180_000);
    let whole = out
        .iter()
        .find(|b| b.t_open_ms == 60_000)
        .expect("the sample over the candle");
    assert!((whole.buy_quote + whole.sell_quote - 100.0).abs() < 1e-3);
    let out = [*whole];
    assert!(
        out[0].buy_quote > out[0].sell_quote,
        "a rising bar leans to buying"
    );
}

/// A replay's split is the ticks' own sides where the ticks are, and the leaned candle turnover
/// only where they are not.
///
/// Breakage: a candle inside the covered span would add the same prints a second time; a tick's
/// side or value going into the wrong column would draw the band mirrored.
#[test]
fn replay_sides_take_ticks_where_covered_and_candles_elsewhere() {
    use crate::feed::{Side, Tick};
    let tick = |time_ms: f64, price: f32, qty: f32, side: Side| Tick {
        time_ms,
        price,
        qty,
        side,
    };
    // Two prints in the covered minute: 2 × 1.5 bought, 1 × 1.5 sold.
    let ticks = [
        tick(60_500.0, 1.5, 2.0, Side::Buy),
        tick(61_500.0, 1.5, 1.0, Side::Sell),
        // Rejected outright: a print with no value.
        tick(62_500.0, 1.5, 0.0, Side::Buy),
    ];
    let candle = |t_open_ms: f64, quote: f32| crate::market::ChartCandle {
        t_open_ms,
        open: 1.0,
        high: 2.0,
        low: 0.5,
        close: 1.5,
        volume: 10.0,
        quote_volume: quote,
    };
    // The covered minute's bar must be ignored; the next minute's bar stands in for its prints.
    let candles = [candle(60_000.0, 1000.0), candle(120_000.0, 100.0)];
    let slots = super::side_slots_of_ticks(&ticks, TickValue::Base);
    let out = super::replay_sides(
        &slots,
        Some((60_000, 119_999)),
        &candles,
        60_000,
        60_000,
        0,
        240_000,
    );
    let at = |t: i64| out.iter().find(|b| b.t_open_ms == t).expect("sample");
    // The sample ending at 120 000 covers the covered minute: ticks only.
    let covered = at(60_000);
    assert!((covered.buy_quote - 3.0).abs() < 1e-3, "2 × 1.5 bought");
    assert!((covered.sell_quote - 1.5).abs() < 1e-3, "1 × 1.5 sold");
    // The next sample covers the uncovered minute: the candle's turnover, leaned to buying.
    let bars = at(120_000);
    assert!((bars.buy_quote + bars.sell_quote - 100.0).abs() < 1e-2);
    assert!(
        bars.buy_quote > bars.sell_quote,
        "a rising bar leans to buying"
    );
    // No ticks at all: every bar counts.
    let out = super::replay_sides(&[], None, &candles, 60_000, 60_000, 0, 240_000);
    let first = out.iter().find(|b| b.t_open_ms == 60_000).expect("sample");
    assert!((first.buy_quote + first.sell_quote - 1000.0).abs() < 1e-1);
}

/// The seam between ticks and bars is per second: a bar straddling the covered span's end gives
/// only its uncovered seconds, so a covered second is never counted twice — the one second the
/// end falls inside is the only overlap, and it is a sixtieth of a bar, not a minute.
///
/// Breakage: `covered` ends on the fetch window's millisecond stamps, never on a minute, so a
/// per-bar rule re-added every boundary minute's turnover on top of its real prints — exactly
/// around the entry and the exit.
#[test]
fn replay_sides_take_only_the_uncovered_seconds_of_a_straddling_bar() {
    let candle = crate::market::ChartCandle {
        t_open_ms: 60_000.0,
        open: 1.0,
        high: 2.0,
        low: 0.5,
        close: 1.5,
        volume: 10.0,
        quote_volume: 600.0,
    };
    // Ticks cover the minute's first 20.5 seconds; the bar may stand in only for the other 39.
    let covered = Some((60_000, 80_499));
    let out = super::replay_sides(&[], covered, &[candle], 60_000, 60_000, 0, 180_000);
    let sample = out
        .iter()
        .find(|b| b.t_open_ms == 60_000)
        .expect("the sample over the bar");
    // 600 over 60 seconds = 10 per second; seconds 60..=79 are covered (the 80th straddles the
    // end and is not wholly inside), so 40 seconds stand in.
    assert!(
        (sample.buy_quote + sample.sell_quote - 400.0).abs() < 1e-2,
        "only the uncovered seconds: {}",
        sample.buy_quote + sample.sell_quote
    );
}

/// The replay's split is taken from the RAW prints, so thinning them for drawing changes nothing.
///
/// Breakage: summing the thinned run — a few representative ticks per bucket — collapsed the
/// band exactly where the ticks were, as the developer saw on a live window (19.09.2026).
#[test]
fn side_slots_come_from_the_raw_prints_not_the_thinned_ones() {
    use crate::feed::{Side, Tick};
    // Sixty prints inside one second, alternating sides, 1 quote each.
    let raw: Vec<Tick> = (0..60)
        .map(|i| Tick {
            time_ms: 60_000.0 + f64::from(i) * 10.0,
            price: 1.0,
            qty: 1.0,
            side: if i % 2 == 0 { Side::Buy } else { Side::Sell },
        })
        .collect();
    let slots = super::side_slots_of_ticks(&raw, TickValue::Base);
    assert_eq!(slots.len(), 1);
    assert!((slots[0].buy - 30.0).abs() < 1e-9);
    assert!((slots[0].sell - 30.0).abs() < 1e-9);
    // Thinned to four representatives, the run would have kept a fifteenth of the turnover.
    let (thinned, _) = crate::market::trade_replay::fit_ticks(raw.clone(), 4);
    let from_thinned = super::side_slots_of_ticks(&thinned, TickValue::Base);
    assert!(from_thinned[0].buy + from_thinned[0].sell < 60.0);
    // The band, drawn from the raw slots, reads the whole second.
    let out = super::replay_sides(
        &slots,
        Some((60_000, 60_999)),
        &[],
        1_000,
        1_000,
        60_000,
        61_000,
    );
    let sample = out.iter().find(|b| b.t_open_ms == 60_000).expect("sample");
    assert!((sample.buy_quote + sample.sell_quote - 60.0).abs() < 1e-3);
}

/// A contract count becomes a quote value through the contract's size — by the fixed dollar
/// value on an inverse contract, by the coin size and the price on a linear one — and stays out
/// of the band altogether when the size is unknown.
#[test]
fn side_slots_value_contracts_by_their_size_or_not_at_all() {
    use crate::feed::{Side, Tick};
    let print = Tick {
        time_ms: 1_000.0,
        price: 50_000.0,
        qty: 3.0,
        side: Side::Buy,
    };
    let inverse = super::side_slots_of_ticks(
        &[print],
        TickValue::InverseContracts {
            usd_per_contract: 100.0,
        },
    );
    assert!((inverse[0].buy - 300.0).abs() < 1e-9, "3 contracts × $100");
    let linear = super::side_slots_of_ticks(
        &[print],
        TickValue::LinearContracts {
            coins_per_contract: 0.01,
        },
    );
    assert!(
        (linear[0].buy - 1_500.0).abs() < 1e-6,
        "3 × 0.01 coins × 50 000"
    );
    assert!(super::side_slots_of_ticks(&[print], TickValue::Unknown).is_empty());
}
