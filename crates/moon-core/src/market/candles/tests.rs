use super::*;
use crate::feed::Side;

fn tick(time_ms: f64, price: f32, qty: f32) -> Tick {
    Tick {
        time_ms,
        price,
        qty,
        side: Side::Buy,
    }
}

fn candle(t: f64, o: f32, h: f32, l: f32, c: f32, v: f32) -> ChartCandle {
    ChartCandle {
        t_open_ms: t,
        open: o,
        high: h,
        low: l,
        close: c,
        volume: v,
    }
}

const M: f64 = 60_000.0;
const TF5: i64 = 5 * 60_000;

#[test]
fn aggregate_basic_buckets() {
    let trades = [
        tick(0.0, 10.0, 1.0),
        tick(1.0 * M, 12.0, 2.0),
        tick(4.0 * M, 11.0, 1.0),
        tick(5.0 * M, 9.0, 3.0),
    ];
    let mut out = Vec::new();
    aggregate_trades(&trades, TF5, &mut out);
    assert_eq!(
        out,
        vec![
            candle(0.0, 10.0, 12.0, 10.0, 11.0, 4.0),
            candle(5.0 * M, 9.0, 9.0, 9.0, 9.0, 3.0),
        ]
    );
}

#[test]
fn aggregate_late_resend_updates_old_bucket() {
    let trades = [
        tick(1.0 * M, 10.0, 1.0),
        tick(6.0 * M, 20.0, 1.0),
        tick(2.0 * M, 30.0, 1.0), // Late resend into the first bucket.
    ];
    let mut out = Vec::new();
    aggregate_trades(&trades, TF5, &mut out);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].high, 30.0);
    assert_eq!(out[0].volume, 2.0);
    // The first bucket's close remains 10.0 because ordering is not reconstructed.
    assert_eq!(out[0].close, 10.0);
}

#[test]
fn resample_5m_to_15m() {
    let rows = [
        candle(0.0, 1.0, 3.0, 0.5, 2.0, 1.0),
        candle(5.0 * M, 2.0, 4.0, 1.5, 3.0, 1.0),
        candle(10.0 * M, 3.0, 5.0, 2.5, 4.0, 1.0),
        candle(15.0 * M, 4.0, 6.0, 3.5, 5.0, 1.0),
    ];
    let mut out = Vec::new();
    resample(&rows, 15 * 60_000, &mut out);
    assert_eq!(
        out,
        vec![
            candle(0.0, 1.0, 5.0, 0.5, 4.0, 3.0),
            candle(15.0 * M, 4.0, 6.0, 3.5, 5.0, 1.0),
        ]
    );
}

#[test]
fn rebuild_prefers_server_before_first_full_trade_bucket() {
    // The base has the 0- and 5-minute buckets, while trades produce local buckets at
    // 5 and 10 minutes. Base coverage at the first local timestamp moves the overlay to 10.
    let server = [
        candle(0.0, 1.0, 2.0, 0.5, 1.5, 10.0),
        candle(5.0 * M, 1.5, 3.0, 1.0, 2.0, 10.0),
    ];
    let trades = [
        tick(7.0 * M, 100.0, 1.0), // This row creates the covered 5-minute local bucket.
        tick(10.0 * M, 2.0, 1.0),
        tick(12.0 * M, 2.5, 1.0),
    ];
    let mut s = CandleSeries::default();
    s.rebuild(TF5, &server, TF5, &trades);
    let c = s.candles();
    assert_eq!(c.len(), 3);
    // The base row is retained at the covered first local timestamp.
    assert_eq!(c[1], server[1]);
    // The 10-minute bucket is local.
    assert_eq!(c[2].t_open_ms, 10.0 * M);
    assert_eq!(c[2].open, 2.0);
    assert_eq!(c[2].close, 2.5);
}

/// A thin market must not punch holes in a base that covers those buckets.
///
/// Trades are the better source for a bucket they actually cover, but only for that bucket. On a
/// coin that trades a few times an hour the overlay used to start at the FIRST trade in the window
/// and discard every base candle after it, so a full exchange history collapsed to the handful of
/// buckets that happened to contain a trade — measured live on CETUSUSDC as 15 candles across a
/// span holding 57, with 672 cached rows available and unused.
#[test]
fn rebuild_keeps_base_in_buckets_the_trades_do_not_cover() {
    let base: Vec<ChartCandle> = (0..5)
        .map(|i| {
            let t = i as f64 * TF5 as f64;
            candle(t, 10.0, 11.0, 9.0, 10.5, 1.0)
        })
        .collect();
    // Two trades in a five-bucket base: the first bucket and the last, four apart.
    let trades = [
        tick(1.0 * M, 20.0, 1.0),
        tick(4.0 * TF5 as f64 + M, 30.0, 1.0),
    ];

    let mut s = CandleSeries::default();
    s.rebuild(TF5, &base, TF5, &trades);
    let out = s.candles();

    assert_eq!(
        out.len(),
        5,
        "every base bucket must survive; trades only replace the ones they cover, got {:?}",
        out.iter()
            .map(|c| c.t_open_ms / TF5 as f64)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        out[1].close, 10.5,
        "an untraded bucket keeps its base candle"
    );
    assert_eq!(out[2].close, 10.5);
    assert_eq!(out[3].close, 10.5);
    assert_eq!(
        out[4].close, 30.0,
        "a traded bucket is still won by the trade that covers it"
    );
}

/// A live trade must take over a base bucket, not add itself on top of it.
///
/// The per-bucket merge lets the series END with a base candle — the base runs to now while a
/// thin market's last trade is older. That bucket's volume is already complete from the source,
/// so the live drain adding trade quantity to it would double count. Before the merge the series
/// always ended in a trade-derived candle and this could not happen.
#[test]
fn push_trades_takes_over_a_base_bucket_instead_of_adding_to_it() {
    let base: Vec<ChartCandle> = (0..6)
        .map(|i| candle(i as f64 * TF5 as f64, 10.0, 11.0, 9.0, 10.5, 100.0))
        .collect();
    // One trade, early. Buckets 2..5 stay base, so the series ends on a base candle.
    let trades = [tick(1.0 * TF5 as f64 + M, 20.0, 1.0)];
    let mut s = CandleSeries::default();
    s.rebuild(TF5, &base, TF5, &trades);
    assert_eq!(s.candles().len(), 6);
    assert_eq!(
        s.candles()[5].volume,
        100.0,
        "the last bucket is still the base candle"
    );

    // A live trade lands in that last, base-derived bucket.
    assert!(s.push_trades(&[tick(5.0 * TF5 as f64 + M, 42.0, 3.0)]));
    let last = s.candles()[5];
    assert_eq!(
        last.volume, 3.0,
        "the bucket is taken over by the trade, not added to the source's complete volume"
    );
    assert_eq!(last.close, 42.0);
    assert_eq!(s.candles().len(), 6, "taking over must not add a bucket");

    // A second live trade in the same bucket now accumulates normally.
    assert!(s.push_trades(&[tick(5.0 * TF5 as f64 + 2.0 * M, 44.0, 2.0)]));
    assert_eq!(s.candles()[5].volume, 5.0);
    assert_eq!(s.candles()[5].close, 44.0);
}

#[test]
fn rebuild_without_server_takes_partial_first() {
    let trades = [tick(7.0 * M, 5.0, 1.0), tick(12.0 * M, 6.0, 1.0)];
    let mut s = CandleSeries::default();
    s.rebuild(60_000, &[], TF5, &trades); // A 1-minute target cannot use a 5-minute base.
    assert_eq!(s.candles().len(), 2);
    assert_eq!(s.candles()[0].t_open_ms, 7.0 * M);
}

#[test]
fn rebuild_resamples_native_base_tf() {
    // Resample pairs from a 30-minute base into a 60-minute series.
    let base = [
        candle(0.0, 1.0, 3.0, 0.5, 2.0, 1.0),
        candle(30.0 * M, 2.0, 4.0, 1.5, 3.0, 1.0),
    ];
    let mut s = CandleSeries::default();
    s.rebuild(60 * 60_000, &base, 30 * 60_000, &[]);
    assert_eq!(s.candles(), &[candle(0.0, 1.0, 4.0, 0.5, 3.0, 2.0)]);
    // Ignore a non-divisible base: a 5-minute series cannot use a 30-minute base.
    let mut s2 = CandleSeries::default();
    s2.rebuild(TF5, &base, 30 * 60_000, &[]);
    assert!(s2.candles().is_empty());
}

#[test]
fn push_trades_updates_live_and_seals() {
    let mut s = CandleSeries::default();
    s.rebuild(TF5, &[], TF5, &[tick(1.0 * M, 10.0, 1.0)]);
    let rev = s.revision();
    assert!(s.push_trades(&[tick(2.0 * M, 12.0, 1.0)]));
    assert_eq!(s.candles().len(), 1);
    assert_eq!(s.candles()[0].high, 12.0);
    assert!(s.push_trades(&[tick(6.0 * M, 8.0, 1.0)])); // A bucket crossing opens a candle.
    assert_eq!(s.candles().len(), 2);
    assert_eq!(s.candles()[1].open, 8.0);
    assert_ne!(s.revision(), rev);
}

#[test]
fn price_range_covers_window_only() {
    let mut s = CandleSeries::default();
    s.rebuild(
        TF5,
        &[
            candle(0.0, 1.0, 100.0, 0.5, 2.0, 1.0),
            candle(5.0 * M, 2.0, 3.0, 1.5, 2.5, 1.0),
        ],
        TF5,
        &[],
    );
    // A window covering only the second bucket excludes the first bucket's extremes.
    let r = s.price_range(5.0 * M, 9.0 * M).unwrap();
    assert_eq!(r, (1.5, 3.0));
    assert_eq!(s.price_range(0.0, 20.0 * M).unwrap(), (0.5, 100.0));
}

#[test]
fn normalize_ohlc_unswaps_server_wire_order() {
    // Preserve an already valid candle.
    assert_eq!(
        normalize_ohlc(10.0, 12.0, 9.0, 11.0),
        (10.0, 12.0, 9.0, 11.0)
    );
    // With swapped (o,c,h,l) = (high, low, open, close), a real candle with
    // o=10, h=12, l=9, c=11 arrives as o=12, c=9, h=10, l=11.
    let (o, h, l, c) = normalize_ohlc(12.0, 10.0, 11.0, 9.0);
    assert_eq!((o, h, l, c), (10.0, 12.0, 9.0, 11.0));
    // A wickless bearish candle (o==h, c==l) is indistinguishable from valid data and stays intact.
    assert_eq!(normalize_ohlc(12.0, 12.0, 9.0, 9.0), (12.0, 12.0, 9.0, 9.0));
    // Preserve a flat candle.
    assert_eq!(normalize_ohlc(5.0, 5.0, 5.0, 5.0), (5.0, 5.0, 5.0, 5.0));
    // For garbage input, expand the range while preserving o/c.
    let (o, h, l, c) = normalize_ohlc(10.0, 11.0, 12.0, 9.5);
    assert_eq!((o, c), (10.0, 9.5));
    assert_eq!((h, l), (12.0, 9.5));
}

#[test]
fn orient_range_rows_by_direction() {
    // Snapshot rows contain only the range: open==high and close==low.
    let mut rows = vec![
        candle(0.0, 12.0, 12.0, 10.0, 10.0, 1.0), // Keep the first row as received (down).
        candle(5.0 * M, 14.0, 14.0, 12.0, 12.0, 1.0), // Midpoint 13 > 11: orient up.
        candle(10.0 * M, 12.0, 12.0, 9.0, 9.0, 1.0), // Midpoint 10.5 < 13: orient down.
    ];
    orient_range_rows(&mut rows);
    assert_eq!((rows[0].open, rows[0].close), (12.0, 10.0));
    assert_eq!((rows[1].open, rows[1].close), (12.0, 14.0)); // Reoriented upward.
    assert_eq!((rows[2].open, rows[2].close), (12.0, 9.0));
    // Leave a real candle with wicks unchanged.
    let honest = candle(0.0, 10.0, 12.0, 9.0, 11.0, 1.0);
    let mut rows2 = vec![honest];
    orient_range_rows(&mut rows2);
    assert_eq!(rows2[0], honest);
}

#[test]
fn deep_kind_mapping() {
    assert_eq!(deep_kind_min_for_tf(1), 1);
    assert_eq!(deep_kind_min_for_tf(5), 5);
    assert_eq!(deep_kind_min_for_tf(30), 30);
    assert_eq!(deep_kind_min_for_tf(60), 60);
    assert_eq!(deep_kind_min_for_tf(240), 240);
    assert_eq!(deep_kind_min_for_tf(1440), 1440);
}

#[test]
fn cfg_defaults_sane() {
    let cfg = CandleViewCfg::default();
    assert_eq!(cfg.tf_ms(), TF5);
    assert_eq!(cfg.mode, CANDLE_MODE_OUTLINE_IN_ZONE);
    assert!(cfg.trade_candles > 0);
    assert!(cfg.price_lines);
    assert!(cfg.show_last_price_line());
    assert!(cfg.show_mark_price_line());
    assert!(cfg.order_traces);
    assert!(cfg.moonshot_corridor);
    // Clamp an unknown or removed timeframe to 5 minutes, including legacy 15-minute configs.
    let bad = CandleViewCfg { tf_min: 15, ..cfg };
    assert_eq!(bad.tf_ms(), TF5);
    // Map legacy 30-second code 0 to 1 minute, and verify the one-day timeframe.
    assert_eq!(CandleViewCfg { tf_min: 0, ..cfg }.tf_ms(), 60_000);
    assert_eq!(
        CandleViewCfg {
            tf_min: 1440,
            ..cfg
        }
        .tf_ms(),
        86_400_000
    );
}
