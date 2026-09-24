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
        quote_volume: 0.0,
    }
}

/// `market/candles.rs:estimate_quote_volume` replacing OHLC4 with close makes range-only history
/// report an extreme rather than its midpoint, so the quote-money band misstates turnover.
#[test]
fn quote_turnover_estimate_is_bounded_midpoint_based_and_orientation_invariant() {
    let volume = 7.0;
    let estimate = estimate_quote_volume(volume, 12.0, 20.0, 10.0, 18.0);
    assert!((volume * 10.0..=volume * 20.0).contains(&estimate));

    let range_only = estimate_quote_volume(volume, 20.0, 20.0, 10.0, 10.0);
    assert_eq!(range_only, volume * (20.0 + 10.0) * 0.5);
    assert_eq!(
        range_only,
        estimate_quote_volume(volume, 10.0, 20.0, 10.0, 20.0),
        "orienting a range-only row must not change its OHLC4 turnover estimate"
    );
    for invalid in [0.0, -1.0, f32::NAN] {
        assert_eq!(estimate_quote_volume(invalid, 12.0, 20.0, 10.0, 18.0), 0.0);
    }
    assert_eq!(
        estimate_quote_volume(volume, f32::NAN, 20.0, 10.0, 18.0),
        0.0
    );
}

/// `market/candles.rs:aggregate_trades` and `resample` estimating instead of summing price times
/// quantity makes a live chart disagree with the exact trades that formed a candle.
#[test]
fn tick_turnover_stays_exact_through_late_resend_and_resample() {
    let trades = [
        tick(1.0 * M, 10.0, 2.0),
        tick(6.0 * M, 20.0, 3.0),
        tick(2.0 * M, 30.0, 1.0),
    ];
    let mut buckets = Vec::new();
    aggregate_trades(&trades, TF5, &mut buckets);
    assert_eq!(
        buckets[0].quote_volume, 50.0,
        "late resend adds its own price times quantity"
    );
    assert_eq!(buckets[1].quote_volume, 60.0);

    let mut coarse = Vec::new();
    resample(&buckets, 15 * 60_000, &mut coarse);
    assert_eq!(
        coarse[0].quote_volume, 110.0,
        "a coarse bucket sums exact child turnover"
    );
}

/// `market/candles.rs:compose_with_coarse` dropping the copied turnover field makes history-tail
/// buckets render as zero quote money even though their cached data is present.
#[test]
fn coarse_composition_preserves_the_cached_quote_turnover() {
    let minute = 60_000.0;
    let mut cached = candle(5.0 * minute, 10.0, 12.0, 9.0, 11.0, 3.0);
    cached.quote_volume = 33.0;
    let mut out = Vec::new();
    compose_with_coarse(
        &[
            candle(0.0, 10.0, 11.0, 9.0, 10.5, 1.0),
            candle(10.0 * minute, 11.0, 12.0, 10.0, 11.5, 1.0),
        ],
        minute,
        &[CoarseLayer {
            rows: &[cached],
            tf_ms: 5.0 * minute,
        }],
        &mut out,
    );
    assert_eq!(
        out.iter()
            .find(|(c, _)| c.t_open_ms == cached.t_open_ms)
            .map(|(c, _)| c.quote_volume),
        Some(33.0)
    );
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
    let mut first = candle(0.0, 10.0, 12.0, 10.0, 11.0, 4.0);
    first.quote_volume = 45.0;
    let mut second = candle(5.0 * M, 9.0, 9.0, 9.0, 9.0, 3.0);
    second.quote_volume = 27.0;
    assert_eq!(out, vec![first, second]);
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

/// Reverting the fresh zone widths would give new users the former 3/0 chart.
#[test]
fn cfg_defaults_sane() {
    let cfg = CandleViewCfg::default();
    assert_eq!(cfg.tf_ms(), TF5);
    assert_eq!(cfg.mode, CANDLE_MODE_OUTLINE_IN_ZONE);
    assert_eq!((cfg.trade_candles, cfg.hide_candles), (50, 50));
    assert!(
        cfg.carried_lines.is_none(),
        "a fresh value carries nothing to migrate"
    );
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

/// A `layout.toml` or `charts.json` written before the price-line split carries only
/// `price_lines`. It is still read — into the carrier the startup pass moves to
/// `ChartGraphicsCfg` — because losing it would turn BOTH lines back on for every user who had
/// them off, which is exactly the setting they went into the popup to change.
#[test]
fn legacy_price_lines_flag_is_carried_for_both_split_lines() {
    let off: CandleViewCfg = toml::from_str("tf_min = 5\nprice_lines = false\n")
        .expect("a pre-split layout.toml must still load");
    let carried = off.carried_lines.expect("carried");
    assert_eq!(carried.last_price_line, Some(false));
    assert_eq!(carried.mark_price_line, Some(false));
    assert_eq!(
        carried.moonshot_zone, None,
        "a key the file never had is not invented"
    );

    let on: CandleViewCfg =
        serde_json::from_str(r#"{"tf_min":5,"price_lines":true}"#).expect("a pre-split spec loads");
    let carried = on.carried_lines.expect("carried");
    assert_eq!(carried.last_price_line, Some(true));
    assert_eq!(carried.mark_price_line, Some(true));
}

/// A file carrying BOTH the legacy flag and a split one keeps the split value: the legacy flag is
/// a fallback, not an override, or a stale key would outrank what the user last clicked.
#[test]
fn split_price_line_flags_outrank_the_legacy_one() {
    let cfg: CandleViewCfg =
        serde_json::from_str(r#"{"price_lines":false,"mark_price_line":true}"#)
            .expect("a mixed spec loads");
    let carried = cfg.carried_lines.expect("carried");
    assert_eq!(
        carried.last_price_line,
        Some(false),
        "the legacy flag still covers last"
    );
    assert_eq!(
        carried.mark_price_line,
        Some(true),
        "the split flag wins for mark"
    );
}

/// The carrier survives a save for as long as it is carried — under the old keys, never as a
/// key of its own — and a value carrying nothing writes none of them. That is what lets a save
/// that lands before the carry-over is committed leave the next launch's pass something to read.
#[test]
fn the_carrier_is_written_back_under_the_old_keys_until_consumed() {
    let cfg: CandleViewCfg =
        serde_json::from_str(r#"{"tf_min":5,"moonshot_zone":false}"#).expect("loads");
    assert!(cfg.carried_lines.is_some());
    let text = serde_json::to_string(&cfg).expect("serializes");
    assert!(
        text.contains(r#""moonshot_zone":false"#),
        "still carried: written back"
    );
    assert!(
        !text.contains("last_price_line"),
        "a key the file never had is not invented"
    );
    assert!(!text.contains("carried_lines"));
    let back: CandleViewCfg = serde_json::from_str(&text).expect("reloads");
    assert_eq!(back, cfg);

    let consumed = CandleViewCfg {
        carried_lines: None,
        ..cfg
    };
    let text = serde_json::to_string(&consumed).expect("serializes");
    assert!(
        !text.contains("moonshot_zone"),
        "consumed: the old key is gone"
    );
}

/// Filling absent saved widths from the fresh default would silently redraw older charts.
#[test]
fn missing_keys_fall_back_to_defaults() {
    for cfg in [
        serde_json::from_str::<CandleViewCfg>("{}").expect("an empty spec loads"),
        toml::from_str::<CandleViewCfg>("mode = 2\n").expect("an older table loads"),
    ] {
        assert_eq!(
            cfg,
            CandleViewCfg {
                trade_candles: 3,
                hide_candles: 0,
                ..CandleViewCfg::default()
            }
        );
    }
    let trade_only: CandleViewCfg = toml::from_str("trade_candles = 7\n").unwrap();
    assert_eq!((trade_only.trade_candles, trade_only.hide_candles), (7, 0));
    let hide_only: CandleViewCfg = serde_json::from_str(r#"{"hide_candles":2}"#).unwrap();
    assert_eq!((hide_only.trade_candles, hide_only.hide_candles), (3, 2));
}

/// A hand-written `nan` must not reach the config. It compares unequal to itself, so the engine's
/// "did the candle view change?" check would fire on EVERY frame, marking the view dirty and
/// rebuilding order geometry forever.
#[test]
fn a_non_finite_outline_width_falls_back_to_the_default() {
    let cfg: CandleViewCfg = toml::from_str("outline_px = nan\n").expect("loads");
    assert_eq!(cfg.outline_px, CandleViewCfg::default().outline_px);
    assert_eq!(cfg, cfg, "the loaded config must compare equal to itself");
}

/// Only what the history read consumes may buy a pane reset. A style checkbox or a leftover
/// carrier must leave the reduced value untouched, while the timeframe must move it.
#[test]
fn history_inputs_ignores_style_and_overlay_fields() {
    let base = CandleViewCfg::default();
    let styled = CandleViewCfg {
        outline_px: 3.0,
        wicks_in_zone: !base.wicks_in_zone,
        neutral_in_zone: !base.neutral_in_zone,
        hide_candles: 5,
        trades_limit: 7,
        carried_lines: Some(CarriedLines {
            last_price_line: Some(false),
            mark_price_line: None,
            moonshot_zone: None,
        }),
        ..base
    };
    assert_eq!(
        styled.history_inputs(),
        base.history_inputs(),
        "a style or overlay change must not reset the pane's history"
    );

    for changed in [
        CandleViewCfg { tf_min: 30, ..base },
        CandleViewCfg {
            mode: CANDLE_MODE_OFF,
            ..base
        },
        CandleViewCfg {
            trade_candles: base.trade_candles + 1,
            ..base
        },
    ] {
        assert_ne!(
            changed.history_inputs(),
            base.history_inputs(),
            "a read input must still reset: {changed:?}"
        );
    }
}

/// The `[candle_view]` block copied VERBATIM out of a real `layout.toml` this machine had written
/// before the split, with the price lines deliberately turned off. A synthetic fixture would only
/// prove the migration against the keys the migration itself knows about.
#[test]
fn a_real_pre_split_layout_block_still_loads() {
    let cfg: CandleViewCfg = toml::from_str(
        "tf_min = 1\n\
         mode = 0\n\
         trade_candles = 0\n\
         hide_candles = 0\n\
         trades_limit = 50000\n\
         outline_px = 1.0\n\
         wicks_in_zone = true\n\
         neutral_in_zone = false\n\
         price_lines = false\n",
    )
    .expect("a layout.toml written before the split must still load");

    assert_eq!(cfg.tf_min, 1);
    assert_eq!(cfg.mode, CANDLE_MODE_FILLED);
    assert_eq!(cfg.trade_candles, 0);
    assert!(cfg.wicks_in_zone);
    assert!(!cfg.neutral_in_zone);
    let carried = cfg
        .carried_lines
        .expect("the user had the price lines off: carried");
    assert_eq!(carried.last_price_line, Some(false));
    assert_eq!(carried.mark_price_line, Some(false));
    assert_eq!(
        carried.moonshot_zone, None,
        "a key that did not exist is not invented"
    );
}

/// Saving and reloading must be lossless, or a setting would drift every time the app writes its
/// config. The wire shim only affects reading, so this is the guard that it stays symmetric.
#[test]
fn candle_view_survives_a_save_and_reload_round_trip() {
    // EVERY field differs from the default on purpose: a field the wire shim forgets is filled
    // from the default on load, so a fixture sharing any default value would let that loss pass.
    // Spelled out in full rather than through `..default()` for the same reason — a field added
    // later fails to compile here until someone gives it a non-default value.
    let cfg = CandleViewCfg {
        tf_min: 30,
        mode: CANDLE_MODE_FILLED,
        trade_candles: 10,
        hide_candles: 2,
        trades_limit: 1_234,
        outline_px: 3.0,
        wicks_in_zone: false,
        neutral_in_zone: true,
        // `None` is what every value the popup writes holds; the carried case has a test of its
        // own above.
        carried_lines: None,
    };
    let toml_back: CandleViewCfg =
        toml::from_str(&toml::to_string(&cfg).expect("serializes")).expect("reloads");
    assert_eq!(toml_back, cfg);
    let json_back: CandleViewCfg =
        serde_json::from_str(&serde_json::to_string(&cfg).expect("serializes")).expect("reloads");
    assert_eq!(json_back, cfg);
}

/// `candles::first_full_bucket_ms` must refuse only a leading partial bucket; replacing its
/// ceil-and-boundary body with the former plain floor would overwrite a complete cached candle
/// with the tail of a restarted trade ring.
#[test]
fn first_full_bucket_refuses_a_partial_leading_bucket_without_losing_aligned_ones() {
    let tf = 5 * 60_000;

    assert_eq!(
        first_full_bucket_ms(300_001.0, tf),
        600_000,
        "a source starting inside a bucket cannot seal that bucket"
    );
    assert_eq!(
        first_full_bucket_ms(600_000.0, tf),
        600_000,
        "a source starting on a boundary already covers a whole bucket"
    );
    assert_eq!(
        first_full_bucket_ms(-600_000.0, tf),
        -600_000,
        "a negative boundary must not move toward zero"
    );
    assert_eq!(
        first_full_bucket_ms(0.0, tf),
        0,
        "the epoch boundary must remain the epoch"
    );
}

/// `candles::compose_with_coarse` must fill an interior cache hole; deleting interior-hole
/// discovery makes a fragmented chart leave its cached minutes undrawn after a restart.
#[test]
fn compose_with_coarse_fills_interior_holes_in_ascending_gpu_order() {
    let minute = 60_000.0;
    let series = [
        candle(0.0, 10.0, 11.0, 9.0, 10.5, 1.0),
        candle(10.0 * minute, 11.0, 12.0, 10.0, 11.5, 1.0),
    ];
    let five_minute_rows = [candle(5.0 * minute, 20.0, 21.0, 19.0, 20.5, 1.0)];
    let mut out = Vec::new();

    compose_with_coarse(
        &series,
        minute,
        &[CoarseLayer {
            rows: &five_minute_rows,
            tf_ms: 5.0 * minute,
        }],
        &mut out,
    );

    assert!(
        out.iter()
            .any(|(c, tf)| c.t_open_ms == 5.0 * minute && *tf == 5.0 * minute as f32),
        "the five-minute cache row must appear inside the missing one-minute stretch"
    );
    assert!(
        out.windows(2)
            .all(|pair| pair[0].0.t_open_ms <= pair[1].0.t_open_ms),
        "GPU consumers require candles in ascending opening-time order"
    );
    assert!(
        out.iter().all(|(_, tf)| *tf > 0.0),
        "each candle needs a positive drawing width"
    );
}

/// `candles::compose_with_coarse` must use only layers that fit and must pass each residual hole
/// to the next layer; changing the inclusive width check to `>` drops an exactly-fitting cache
/// bucket and leaves a visible chart gap.
#[test]
fn compose_with_coarse_uses_inclusive_width_and_only_the_remaining_holes() {
    let minute = 60_000.0;
    let series = [
        candle(0.0, 10.0, 11.0, 9.0, 10.5, 1.0),
        candle(6.0 * minute, 11.0, 12.0, 10.0, 11.5, 1.0),
    ];
    let exact_fit = [candle(0.0, 20.0, 21.0, 19.0, 20.5, 1.0)];
    let mut out = Vec::new();
    compose_with_coarse(
        &series,
        minute,
        &[CoarseLayer {
            rows: &exact_fit,
            tf_ms: 5.0 * minute,
        }],
        &mut out,
    );
    assert!(
        out.iter()
            .any(|(c, tf)| c.t_open_ms == 0.0 && *tf == 5.0 * minute as f32),
        "a hole exactly one coarse period wide accepts its aligned coarse bucket"
    );

    let wide_series = [
        candle(0.0, 10.0, 11.0, 9.0, 10.5, 1.0),
        candle(332.0 * minute, 11.0, 12.0, 10.0, 11.5, 1.0),
    ];
    let daily_rows = [candle(0.0, 30.0, 31.0, 29.0, 30.5, 1.0)];
    compose_with_coarse(
        &wide_series,
        minute,
        &[CoarseLayer {
            rows: &daily_rows,
            tf_ms: 1_440.0 * minute,
        }],
        &mut out,
    );
    assert_eq!(
        out.len(),
        wide_series.len(),
        "a daily bucket cannot represent the measured 331-minute hole"
    );

    let five_minute_rows = [candle(5.0 * minute, 20.0, 21.0, 19.0, 20.5, 1.0)];
    let twenty_minute_rows = [
        candle(0.0, 30.0, 31.0, 29.0, 30.5, 1.0),
        candle(20.0 * minute, 31.0, 32.0, 30.0, 31.5, 1.0),
        candle(40.0 * minute, 32.0, 33.0, 31.0, 32.5, 1.0),
    ];
    let series = [
        candle(0.0, 10.0, 11.0, 9.0, 10.5, 1.0),
        candle(60.0 * minute, 11.0, 12.0, 10.0, 11.5, 1.0),
    ];
    compose_with_coarse(
        &series,
        minute,
        &[
            CoarseLayer {
                rows: &five_minute_rows,
                tf_ms: 5.0 * minute,
            },
            CoarseLayer {
                rows: &twenty_minute_rows,
                tf_ms: 20.0 * minute,
            },
        ],
        &mut out,
    );
    assert_eq!(
        out.iter()
            .filter(|(c, tf)| c.t_open_ms == 5.0 * minute && *tf == 5.0 * minute as f32)
            .count(),
        1,
        "the finer row is emitted once while the coarser layer reaches its uncovered neighbours"
    );
    assert!(
        out.iter()
            .any(|(c, tf)| c.t_open_ms == 20.0 * minute && *tf == 20.0 * minute as f32),
        "the coarser layer reaches the five-minute layer's long internal residual"
    );
}

/// `candles::compose_with_coarse` must preserve its degenerate input contract; dropping those
/// branches can blank a newly opened chart or duplicate an already continuous one.
#[test]
fn compose_with_coarse_preserves_empty_single_and_contiguous_series_inputs() {
    let minute = 60_000.0;
    let rows = [
        candle(0.0, 10.0, 11.0, 9.0, 10.5, 1.0),
        candle(5.0 * minute, 11.0, 12.0, 10.0, 11.5, 1.0),
    ];
    let layer = [CoarseLayer {
        rows: &rows,
        tf_ms: 5.0 * minute,
    }];
    let mut out = Vec::new();
    compose_with_coarse(&[], minute, &layer, &mut out);
    assert_eq!(
        out.iter().map(|(c, _)| *c).collect::<Vec<_>>(),
        rows,
        "an empty fine series falls back to every available coarse row"
    );

    let single = [candle(0.0, 10.0, 11.0, 9.0, 10.5, 1.0)];
    compose_with_coarse(&single, minute, &[], &mut out);
    assert_eq!(out, vec![(single[0], minute as f32)]);

    let contiguous = [
        candle(0.0, 10.0, 11.0, 9.0, 10.5, 1.0),
        candle(minute, 11.0, 12.0, 10.0, 11.5, 1.0),
        candle(2.0 * minute, 12.0, 13.0, 11.0, 12.5, 1.0),
    ];
    compose_with_coarse(&contiguous, minute, &[], &mut out);
    assert_eq!(
        out,
        contiguous
            .iter()
            .copied()
            .map(|c| (c, minute as f32))
            .collect::<Vec<_>>(),
        "empty layers leave an already complete fine series untouched"
    );
}

/// `market/candles.rs:thin_ticks` synthesising bucket-mid points or dropping its role ordering
/// lies about when a market traded and can draw points out of chronological order.
#[test]
fn thin_ticks_keeps_only_real_ascending_points_and_copies_non_positive_buckets() {
    let input = vec![
        tick(0.0, 10.0, 1.0),
        tick(100.0, 14.0, 2.0),
        tick(200.0, 8.0, 3.0),
        tick(300.0, 12.0, 4.0),
        tick(1_000.0, 20.0, 5.0),
        tick(1_100.0, 18.0, 6.0),
        tick(1_200.0, 22.0, 7.0),
        tick(1_300.0, 19.0, 8.0),
        tick(1_400.0, 21.0, 9.0),
    ];
    let mut thinned = Vec::new();
    thin_ticks(&input, 1_000, &mut thinned);

    assert!(
        thinned.iter().all(|point| input.iter().any(|source| {
            source.time_ms == point.time_ms
                && source.price == point.price
                && source.qty == point.qty
        })),
        "every emitted point must be an exact time, price, and quantity observed in the input"
    );
    assert!(
        thinned
            .windows(2)
            .all(|pair| pair[0].time_ms <= pair[1].time_ms),
        "thinned points must stay ascending for the chart consumer"
    );
    for bucket in [0_i64, 1_i64] {
        assert!(
            thinned
                .iter()
                .filter(|point| (point.time_ms as i64).div_euclid(1_000) == bucket)
                .count()
                <= 4,
            "a bucket may keep at most first, high, low, and last real points"
        );
    }

    let mut copied = Vec::new();
    thin_ticks(&input, 0, &mut copied);
    assert_eq!(
        copied
            .iter()
            .map(|point| (point.time_ms, point.price, point.qty))
            .collect::<Vec<_>>(),
        input
            .iter()
            .map(|point| (point.time_ms, point.price, point.qty))
            .collect::<Vec<_>>(),
        "bucket_ms == 0 must preserve raw ticks unchanged"
    );
    thin_ticks(&input, -1, &mut copied);
    assert_eq!(
        copied
            .iter()
            .map(|point| (point.time_ms, point.price, point.qty))
            .collect::<Vec<_>>(),
        input
            .iter()
            .map(|point| (point.time_ms, point.price, point.qty))
            .collect::<Vec<_>>(),
        "negative bucket widths also copy input unchanged"
    );
}

/// `market/candles.rs:merge_bases` — a holey native kind is filled from the finer cached kinds
/// before the range-only snapshot gets to draw the bucket (#634: half the 5-minute candles of
/// LSKUSDT drew as wickless bodies from the snapshot while kind-1 rows for the same hours sat in
/// the cache unread). Priority is the part order: snapshot < kind 1 < kind 5 < native < deep.
#[test]
fn merge_bases_fills_native_holes_from_finer_kinds_over_the_snapshot() {
    // Range-only snapshot rows for buckets 0 and 5: open == high, close == low, no wick.
    let snap = [
        candle(0.0, 12.0, 12.0, 8.0, 8.0, 100.0),
        candle(5.0 * M, 13.0, 13.0, 9.0, 9.0, 100.0),
    ];
    // Native kind 5 covers only bucket 0.
    let native = [candle(0.0, 10.0, 11.0, 9.5, 10.5, 50.0)];
    // Kind-1 rows cover bucket 5 in full and bucket 10 for two minutes of five.
    let fine = [
        candle(5.0 * M, 10.0, 10.5, 9.9, 10.2, 1.0),
        candle(6.0 * M, 10.2, 10.8, 10.1, 10.6, 1.0),
        candle(7.0 * M, 10.6, 10.7, 10.0, 10.1, 1.0),
        candle(8.0 * M, 10.1, 10.4, 9.8, 10.3, 1.0),
        candle(9.0 * M, 10.3, 10.9, 10.2, 10.7, 1.0),
        candle(10.0 * M, 10.7, 11.2, 10.6, 11.0, 1.0),
        candle(11.0 * M, 11.0, 11.1, 10.4, 10.5, 1.0),
    ];
    let mut out = Vec::new();
    merge_bases(
        TF5,
        &[
            BasePart {
                rows: &snap,
                tf_ms: TF5,
            },
            BasePart {
                rows: &fine,
                tf_ms: 60_000,
            },
            BasePart {
                rows: &native,
                tf_ms: TF5,
            },
        ],
        &mut out,
    );
    assert_eq!(out.len(), 3, "buckets 0, 5 and 10, got {out:?}");
    assert_eq!(
        out[0], native[0],
        "the native row wins the bucket it covers"
    );
    assert_eq!(
        out[1],
        candle(5.0 * M, 10.0, 10.9, 9.8, 10.7, 5.0),
        "a native hole is filled from the finer kind, not from the snapshot"
    );
    assert_eq!(
        out[2],
        candle(10.0 * M, 10.7, 11.2, 10.4, 10.5, 2.0),
        "a partial period of finer rows still yields a bucket"
    );

    // Without the finer part the snapshot's wickless row is all bucket 5 has.
    merge_bases(
        TF5,
        &[
            BasePart {
                rows: &snap,
                tf_ms: TF5,
            },
            BasePart {
                rows: &native,
                tf_ms: TF5,
            },
        ],
        &mut out,
    );
    assert_eq!(out[1], snap[1]);
    // A part coarser than the target, or one that does not divide it, contributes nothing.
    merge_bases(
        60_000,
        &[BasePart {
            rows: &snap,
            tf_ms: TF5,
        }],
        &mut out,
    );
    assert!(out.is_empty());
}

/// `market/candles.rs:has_holes` gates the finer-kind cache reads: a dense native set costs no
/// extra read, a missing bucket anywhere in the window — prefix, gap or tail — does.
#[test]
fn has_holes_sees_the_prefix_the_gaps_and_the_tail() {
    let row = |i: f64| candle(i * TF5 as f64, 1.0, 2.0, 0.5, 1.5, 1.0);
    assert!(has_holes(&[], TF5, 0, 3 * TF5), "an empty set is one hole");
    // Dense across the window: no hole.
    let dense = [row(0.0), row(1.0), row(2.0)];
    assert!(!has_holes(&dense, TF5, 0, 3 * TF5));
    // The partial bucket at either edge is not a hole.
    assert!(!has_holes(&dense, TF5, -(TF5 - 1), 3 * TF5));
    assert!(!has_holes(&dense, TF5, 0, 4 * TF5));
    // A gap between rows, or more than one bucket missing at either edge, is.
    assert!(has_holes(&[row(0.0), row(2.0)], TF5, 0, 3 * TF5));
    assert!(has_holes(&dense, TF5, -2 * TF5, 3 * TF5));
    assert!(
        has_holes(&dense, TF5, 0, 4 * TF5 + 1),
        "a tail older than a bucket is a hole"
    );
}

/// Max is a sentinel count, not a wider number. Replacing the equality with `>=` would treat a
/// hand-written `1000` as Max and stretch that chart to the ring.
#[test]
fn candle_zone_max_is_only_the_sentinel() {
    assert!(is_candle_zone_max(CANDLE_ZONE_MAX));
    assert!(!is_candle_zone_max(0));
    assert!(!is_candle_zone_max(50));
    assert!(!is_candle_zone_max(CANDLE_ZONE_MAX - 1));
}

/// A saved Max step must come back as Max, and a saved numeric step must not. Losing the
/// sentinel on the wire shim would drop the user's choice to the default of 3 on every launch.
#[test]
fn max_zone_survives_a_save_and_reload_and_numeric_steps_stay_numeric() {
    let cfg = CandleViewCfg {
        tf_min: 1,
        mode: CANDLE_MODE_FILLED,
        trade_candles: CANDLE_ZONE_MAX,
        hide_candles: CANDLE_ZONE_MAX,
        trades_limit: 50_000,
        outline_px: 2.0,
        wicks_in_zone: false,
        neutral_in_zone: true,
        carried_lines: None,
    };
    let toml_back: CandleViewCfg =
        toml::from_str(&toml::to_string(&cfg).expect("serializes")).expect("reloads");
    assert_eq!(toml_back.trade_candles, CANDLE_ZONE_MAX);
    assert_eq!(toml_back.hide_candles, CANDLE_ZONE_MAX);
    assert_eq!(toml_back, cfg);
    let json_back: CandleViewCfg =
        serde_json::from_str(&serde_json::to_string(&cfg).expect("serializes")).expect("reloads");
    assert_eq!(json_back, cfg);

    let numeric: CandleViewCfg =
        toml::from_str("trade_candles = 50\nhide_candles = 3\n").expect("a numeric file loads");
    assert_eq!(numeric.trade_candles, 50);
    assert_eq!(numeric.hide_candles, 3);
    assert!(!is_candle_zone_max(numeric.trade_candles));
}

/// `max_trade_zone_edge_rel` must sit on the open of the bucket that holds the oldest trade.
/// Returning the trade's own timestamp leaves that bucket drawn as an ordinary candle over
/// its crosses, which is a one-candle gap in front of the zone.
#[test]
fn max_zone_edge_is_the_open_of_the_oldest_trades_bucket() {
    let tf_ms = 300_000_i64;
    let epoch_ms = 1_700_000_000_000.0;
    // 23s into a bucket, so a raw timestamp and the bucket open differ.
    let oldest_rel = 90_000.0 + 23_000.0;
    let abs = epoch_ms + oldest_rel as f64;
    let expected_open = (abs / tf_ms as f64).floor() * tf_ms as f64;
    let expected_rel = (expected_open - epoch_ms) as f32;

    let edge = max_trade_zone_edge_rel(oldest_rel, tf_ms, epoch_ms);
    assert_eq!(edge, expected_rel);
    assert!(
        edge <= oldest_rel,
        "the edge cannot sit to the right of its trade"
    );
    assert!(
        oldest_rel < edge + tf_ms as f32,
        "the trade must fall inside the bucket the edge opens"
    );
    // The previous bucket's open is one timeframe left, so it stays an ordinary candle.
    let previous_open = (expected_open - tf_ms as f64 - epoch_ms) as f32;
    assert!(previous_open < edge);
}

/// No retained trade must not punch an empty Max zone. A finite edge with nothing to draw
/// there hides candles (or starts a trade zone) over empty space.
#[test]
fn max_zone_is_off_when_no_trade_is_retained() {
    let tf_ms = 300_000_i64;
    let epoch_ms = 1_700_000_000_000.0;
    let now_ms = epoch_ms + 637_474.0;
    assert!(
        !max_trade_zone_edge_rel(f32::NAN, tf_ms, epoch_ms).is_finite(),
        "a trade zone with no trade is not a zone"
    );
    assert_eq!(
        hide_zone_start_rel(CANDLE_ZONE_MAX, now_ms, tf_ms, epoch_ms, f32::NAN),
        f32::MAX,
        "hide Max with no trade leaves every candle drawn"
    );
    assert!(!trade_zone_start_rel(CANDLE_ZONE_MAX, now_ms, tf_ms, epoch_ms, f32::NAN).is_finite());
}

/// Hide Max and the Max trade zone are one boundary. Diverging them hides candles where no
/// crosses are drawn, or leaves candles on top of the trade stretch.
#[test]
fn hide_max_matches_the_max_trade_zone_edge() {
    let tf_ms = 300_000_i64;
    let epoch_ms = 1_700_000_000_000.0;
    let now_ms = epoch_ms + 637_474.0;
    let oldest_rel = 120_000.0;
    let trade = trade_zone_start_rel(CANDLE_ZONE_MAX, now_ms, tf_ms, epoch_ms, oldest_rel);
    let hide = hide_zone_start_rel(CANDLE_ZONE_MAX, now_ms, tf_ms, epoch_ms, oldest_rel);
    assert!(trade.is_finite());
    assert_eq!(hide, trade);
}

/// The Max read asks for the whole ring. Feeding it the drawn edge would stop the next copy
/// at last frame's oldest trade, so a backfill could never extend the zone.
#[test]
fn max_trade_read_is_unbounded_while_a_numeric_read_keeps_its_window() {
    let tf_ms = 300_000_i64;
    let epoch_ms = 1_700_000_000_000.0;
    let now_ms = epoch_ms + 637_474.0;
    let unbound = trades_read_from_rel(CANDLE_ZONE_MAX, now_ms, tf_ms, epoch_ms);
    assert_eq!(unbound, TRADES_FROM_UNBOUNDED);
    assert!(
        unbound.is_finite(),
        "a non-finite read bound hides trades entirely"
    );

    assert!(!trades_read_from_rel(0, now_ms, tf_ms, epoch_ms).is_finite());

    let three = trades_read_from_rel(3, now_ms, tf_ms, epoch_ms);
    let open = (now_ms / tf_ms as f64).floor() * tf_ms as f64;
    let expected = (open - 2.0 * tf_ms as f64 - epoch_ms) as f32;
    assert_eq!(three, expected);
    assert_ne!(three, TRADES_FROM_UNBOUNDED);
}

/// A Max recopy rebuilds the candle series, so a live print inside the same oldest bucket
/// must not ask for one. Dropping the bucket compare would reset on every eviction.
#[test]
fn max_zone_recopies_on_a_new_oldest_bucket_or_older_history_only() {
    let tf_ms = 300_000_i64;
    let bucket = 1_700_000_100_000_i64;
    assert!(
        !max_zone_recopy_due(None, None, tf_ms),
        "nothing retained and nothing drawn is already in agreement"
    );
    assert!(!max_zone_recopy_due(
        Some(bucket + 1_000),
        Some(bucket + 20_000),
        tf_ms
    ));
    assert!(
        max_zone_recopy_due(Some(bucket + 1_000), Some(bucket + tf_ms), tf_ms),
        "eviction into the next bucket moves the drawn edge"
    );
    assert!(
        max_zone_recopy_due(Some(bucket + 10_000), Some(bucket - tf_ms), tf_ms),
        "older history has to extend the zone"
    );
    assert!(max_zone_recopy_due(Some(bucket), None, tf_ms));
    assert!(max_zone_recopy_due(None, Some(bucket), tf_ms));
}
