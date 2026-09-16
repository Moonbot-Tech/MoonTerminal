use super::*;

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
