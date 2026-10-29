use super::*;

/// Frozen pre-incremental aggregate fold, independent of the production contribution visitor.
fn frozen_mini(bins: &mut HashMap<i64, (f64, f64)>, width: f64, row: MiniCandle) {
    let buy = f64::from(row.buy_vol);
    let sell = f64::from(row.sell_vol);
    let buy = if buy.is_finite() { buy.max(0.0) } else { 0.0 };
    let sell = if sell.is_finite() { sell.max(0.0) } else { 0.0 };
    if buy <= 0.0 && sell <= 0.0 {
        return;
    }
    if !(row.min_price.is_finite() && row.max_price.is_finite()) {
        return;
    }
    let lo = f64::from(row.min_price.min(row.max_price));
    let hi = f64::from(row.min_price.max(row.max_price));
    if hi <= 0.0 {
        return;
    }
    let row_of = |price: f64| {
        (price / width)
            .floor()
            .clamp(i64::MIN as f64 / 4.0, i64::MAX as f64 / 4.0) as i64
    };
    let first = row_of(lo.max(0.0));
    let last = row_of(hi);
    let span = hi - lo;
    if first == last || span <= 0.0 || last - first > 4_096 {
        let slot = bins.entry(row_of((lo + hi) * 0.5)).or_insert((0.0, 0.0));
        slot.0 += buy;
        slot.1 += sell;
        return;
    }
    for id in first..=last {
        let row_lo = id as f64 * width;
        let row_hi = row_lo + width;
        let overlap = (hi.min(row_hi) - lo.max(row_lo)).max(0.0);
        if overlap <= 0.0 {
            continue;
        }
        let share = overlap / span;
        let slot = bins.entry(id).or_insert((0.0, 0.0));
        slot.0 += buy * share;
        slot.1 += sell * share;
    }
}

/// Frozen pre-incremental raw fold: pins both the price bin and the side sign interpretation.
fn frozen_trade(bins: &mut HashMap<i64, (f64, f64)>, width: f64, row: TradeHistoryRow) {
    let price = f64::from(row.price);
    let value = price * f64::from(row.quantity());
    if !(price.is_finite() && price > 0.0 && value.is_finite() && value > 0.0) {
        return;
    }
    let id = (price / width)
        .floor()
        .clamp(i64::MIN as f64 / 4.0, i64::MAX as f64 / 4.0) as i64;
    let slot = bins.entry(id).or_insert((0.0, 0.0));
    if row.is_buy() {
        slot.0 += value;
    } else {
        slot.1 += value;
    }
}

/// Adapt the production visitor to the original hand-calculated bin assertions below.
fn fold_trade(bins: &mut HashMap<i64, (f64, f64)>, width: f64, row: TradeHistoryRow) {
    visit_trade(width, row, |id, buy, sell| {
        let slot = bins.entry(id).or_insert((0.0, 0.0));
        slot.0 += buy;
        slot.1 += sell;
    });
}

/// Frozen old full-profile selection, independent of the incremental tape and bin membership.
fn old_profile(
    minis: &[MiniCandle],
    trades: &[TradeHistoryRow],
    from: i64,
    width: f64,
) -> Vec<PriceProfileRow> {
    let mut bins = HashMap::new();
    let selected = minis.iter().skip_while(|r| r.time.unix_millis() < from);
    let mut newest = None;
    for row in selected {
        newest = Some(newest.map_or(row.time.unix_millis(), |at: i64| {
            at.max(row.time.unix_millis())
        }));
        frozen_mini(&mut bins, width, *row);
    }
    let tail = newest.map_or(from, |at| at + 1);
    for row in trades.iter().skip_while(|r| r.time.unix_millis() < tail) {
        frozen_trade(&mut bins, width, *row);
    }
    let mut rows: Vec<_> = bins
        .into_iter()
        .filter(|(_, (buy, sell))| *buy > 0.0 || *sell > 0.0)
        .map(|(id, (buy, sell))| PriceProfileRow {
            price_lo: (id as f64 * width) as f32,
            price_hi: ((id + 1) as f64 * width) as f32,
            buy_quote: buy as f32,
            sell_quote: sell as f32,
        })
        .collect();
    rows.sort_by(|a, b| a.price_lo.total_cmp(&b.price_lo));
    rows
}

/// Appends, retention, time expiry, late resends and source compaction must keep exact old rows.
#[test]
fn chart_market_incremental_profile_matches_frozen_full_fold() {
    let mut profile = PriceProfile::new();
    let mut minis = Vec::new();
    let mut trades = Vec::new();
    let width = 0.3;
    for step in 0..100 {
        let from = (step - 12).max(0) * 1000;
        profile.minis.clear();
        profile.trades.clear();
        if step % 5 == 0 {
            let mut row = mini(9.1, 11.7, 77.3, 33.9);
            row.time = MoonTime::from_unix_millis((step - 4).max(0) * 1000);
            minis.push(row);
            profile.minis.push(row);
        }
        for offset in [0, 100, -100] {
            let row = trade(
                step * 1000 + offset,
                10.1 + (step % 7) as f32 * 0.13,
                if offset < 0 { -1.7 } else { 2.3 },
            );
            trades.push(row);
            profile.trades.push(row);
        }
        let tail = profile.fold_minis(from, width, 8);
        profile.fold_trades(tail, width, 30);
        profile.publish(width);
        let retained_minis = &minis[minis.len().saturating_sub(8)..];
        let retained_trades = &trades[trades.len().saturating_sub(30)..];
        assert_eq!(
            profile.rows(),
            old_profile(retained_minis, retained_trades, from, width),
            "step {step}"
        );
        let revision = profile.revision();
        profile.publish(width);
        assert_eq!(
            profile.revision(),
            revision,
            "unchanged bins must not re-upload"
        );
    }
}

/// Time the retained fold at 100000 raw prints plus 20000 five-second aggregates.
#[test]
#[ignore = "scale probe: prints timings"]
fn chart_market_scale_profile() {
    let minis: Vec<_> = (0..20_000)
        .map(|i| mini(100.0 + (i % 20) as f32, 103.0 + (i % 20) as f32, 77.0, 33.0))
        .collect();
    let trades: Vec<_> = (0..100_000)
        .map(|i| trade(i, 100.0 + (i % 20) as f32 * 0.1, 1.5))
        .collect();
    let mut samples = Vec::new();
    for _ in 0..11 {
        let start = std::time::Instant::now();
        let mut bins = HashMap::new();
        for row in &minis {
            frozen_mini(&mut bins, 0.25, *row);
        }
        for row in &trades {
            frozen_trade(&mut bins, 0.25, *row);
        }
        std::hint::black_box(&bins);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "chart_market profile fold 20000 minis + 100000 trades: {:.3} ms",
        samples[5]
    );
    let mut profile = PriceProfile::new();
    profile.minis = minis;
    profile.trades = trades;
    let tail = profile.fold_minis(0, 0.25, 20_000);
    profile.fold_trades(tail, 0.25, 100_000);
    profile.publish(0.25);
    assert_eq!(
        profile.rows(),
        old_profile(&profile.minis, &profile.trades, 0, 0.25)
    );
    profile.minis.clear();
    profile.trades.clear();
    let mut samples = Vec::new();
    for index in 0..11 {
        profile.trades.clear();
        profile.trades.push(trade(100_001 + index, 101.0, 1.5));
        let start = std::time::Instant::now();
        let tail = profile.fold_minis(0, 0.25, 20_000);
        profile.fold_trades(tail, 0.25, 100_000);
        profile.publish(0.25);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "chart_market incremental profile append + retention expiry: {:.3} ms",
        samples[5]
    );
}

/// Reconnect and provider replacement invalidate retained cursors even without an archive merge.
#[test]
fn chart_market_profile_rebuilds_on_retained_source_replacement() {
    let mut profile = PriceProfile::new();
    let mut key = HistoryKey::default();
    for next in [
        HistoryKey {
            client_epoch: 1,
            ..key
        },
        HistoryKey {
            generation: 1,
            client_epoch: 1,
            ..key
        },
        HistoryKey {
            generation: 1,
            client_epoch: 1,
            catalog_rev: 1,
            ..key
        },
    ] {
        profile.refresh(None, None, ProfileWindow::All, 0.5, key, 1000);
        profile.trades.push(trade(1000, 10.0, 2.0));
        profile.fold_trades(0, 0.5, 10);
        profile.publish(0.5);
        assert_eq!(profile.rows(), old_profile(&[], &profile.trades, 0, 0.5));
        let revision = profile.revision();
        profile.refresh(None, None, ProfileWindow::All, 0.5, key, 1001);
        assert_eq!(
            profile.revision(),
            revision,
            "same-source TTL reuses the profile"
        );
        profile.refresh(None, None, ProfileWindow::All, 0.5, next, 1001);
        assert!(
            profile.rows().is_empty(),
            "the replacement has no retained rows"
        );
        assert_eq!(profile.revision(), revision + 1);
        key = next;
    }
}

fn trade(ms: i64, price: f32, qty: f32) -> TradeHistoryRow {
    TradeHistoryRow {
        time: MoonTime::from_unix_millis(ms),
        price,
        qty,
    }
}

fn mini(lo: f32, hi: f32, buy: f32, sell: f32) -> MiniCandle {
    MiniCandle {
        time: MoonTime::ZERO,
        cnt: 1,
        min_price: lo,
        max_price: hi,
        buy_vol: buy,
        sell_vol: sell,
    }
}

/// Rows as `(id, buy, sell)`, the amounts rounded to a cent: the ring stores `f32` prices and
/// quantities, so a product printed exactly would compare a decimal literal against its binary
/// neighbour.
fn rows_of(bins: &HashMap<i64, (f64, f64)>) -> Vec<(i64, f64, f64)> {
    let cents = |v: f64| (v * 100.0).round() / 100.0;
    let mut v: Vec<_> = bins
        .iter()
        .map(|(k, (b, s))| (*k, cents(*b), cents(*s)))
        .collect();
    v.sort_by_key(|r| r.0);
    v
}

/// A sale lands on the selling side of the row of its own price, as a positive amount.
///
/// Breakage: the ring spells a sale by the quantity's sign bit; folding the raw quantity in would
/// subtract from the row, and a sell-heavy band would draw shorter than an empty one.
#[test]
fn a_trade_lands_in_the_row_of_its_price_on_its_own_side() {
    let mut bins = HashMap::new();
    fold_trade(&mut bins, 0.5, trade(0, 10.2, 2.0));
    fold_trade(&mut bins, 0.5, trade(1, 10.4, -3.0));
    fold_trade(&mut bins, 0.5, trade(2, 10.6, 1.0));

    assert_eq!(
        rows_of(&bins),
        vec![(20, 20.4, 31.2), (21, 10.6, 0.0)],
        "10.2 and 10.4 share row 20 (10.0..10.5); 10.6 is row 21"
    );
}

/// An aggregate's two sides are spread over the rows its range crosses, by overlap.
///
/// Breakage: lumping a five-second candle that walked three rows into one of them draws a spike
/// where the market traded evenly, and the `PriceFrame` slider stops meaning anything below the
/// candle's own range.
#[test]
fn an_aggregate_is_spread_over_its_range_by_overlap() {
    let mut bins = HashMap::new();
    // 10.0..11.0 at a row width of 0.5: rows 20 and 21, half each.
    fold_mini(&mut bins, 0.5, mini(10.0, 11.0, 100.0, 40.0));
    let rows = rows_of(&bins);
    assert_eq!(rows.len(), 2);
    assert!((rows[0].1 - 50.0).abs() < 1e-9 && (rows[0].2 - 20.0).abs() < 1e-9);
    assert!((rows[1].1 - 50.0).abs() < 1e-9 && (rows[1].2 - 20.0).abs() < 1e-9);

    // 10.4..10.6 at 0.5: row 20 gets 0.1 of 0.2, row 21 the other half.
    let mut bins = HashMap::new();
    fold_mini(&mut bins, 0.5, mini(10.4, 10.6, 10.0, 0.0));
    let rows = rows_of(&bins);
    assert_eq!(rows.len(), 2);
    assert!((rows[0].1 - 5.0).abs() < 1e-6);
    assert!((rows[1].1 - 5.0).abs() < 1e-6);
}

/// The whole of an aggregate is kept: the shares over its rows sum to what it carried.
///
/// Breakage: a proportional spread that loses the row at either edge (an off-by-one on `last`)
/// silently drops turnover, and the profile under-reports exactly the busiest candles.
#[test]
fn spreading_keeps_the_aggregate_whole() {
    let mut bins = HashMap::new();
    fold_mini(&mut bins, 0.3, mini(10.07, 11.93, 77.0, 33.0));
    let (buy, sell) = bins
        .values()
        .fold((0.0, 0.0), |acc, (b, s)| (acc.0 + b, acc.1 + s));
    assert!((buy - 77.0).abs() < 1e-6, "buy {buy}");
    assert!((sell - 33.0).abs() < 1e-6, "sell {sell}");
}

/// An aggregate inside one row, or one whose range is unusable, lands whole in one row.
#[test]
fn a_narrow_or_degenerate_aggregate_lands_in_one_row() {
    let mut bins = HashMap::new();
    fold_mini(&mut bins, 0.5, mini(10.1, 10.2, 5.0, 5.0));
    assert_eq!(rows_of(&bins), vec![(20, 5.0, 5.0)]);

    let mut bins = HashMap::new();
    // Wider than the spread cap: the midpoint's row takes it all rather than a 20k-row walk.
    fold_mini(&mut bins, 0.001, mini(10.0, 30.0, 5.0, 0.0));
    assert_eq!(bins.len(), 1);
    assert_eq!(rows_of(&bins)[0].0, row_of(20.0, 0.001));

    let mut bins = HashMap::new();
    fold_mini(&mut bins, 0.5, mini(f32::NAN, 10.0, 5.0, 0.0));
    fold_mini(&mut bins, 0.5, mini(10.0, 10.0, 0.0, 0.0));
    assert!(bins.is_empty(), "no price or no volume: nothing to place");
}

/// The bench's stand-in reports rows sorted by price with the candle's direction split.
///
/// Breakage: rows out of order would upload a profile whose bars do not line up with the price
/// axis, and the bench would show a picture no live core can produce.
#[test]
fn the_synthetic_profile_is_sorted_and_split_by_direction() {
    let candle = |open: f32, close: f32, low: f32, high: f32| crate::market::ChartCandle {
        t_open_ms: 0.0,
        open,
        high,
        low,
        close,
        volume: 1.0,
        quote_volume: 100.0,
    };
    let rows = synthetic_profile(
        &[candle(10.0, 11.0, 10.0, 11.0), candle(9.0, 8.5, 8.5, 9.0)],
        0.5,
    );
    assert!(
        rows.windows(2).all(|w| w[0].price_lo < w[1].price_lo),
        "sorted by price"
    );
    let rising: f32 = rows
        .iter()
        .filter(|r| r.price_lo >= 10.0)
        .map(|r| r.buy_quote)
        .sum();
    assert!(
        (rising - 62.0).abs() < 1e-3,
        "a rising candle leans to buying: {rising}"
    );
    assert!(synthetic_profile(&[candle(1.0, 1.0, 1.0, 1.0)], 0.0).is_empty());
}
