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
        tick(2.0 * M, 30.0, 1.0), // поздний resend в первый бакет
    ];
    let mut out = Vec::new();
    aggregate_trades(&trades, TF5, &mut out);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].high, 30.0);
    assert_eq!(out[0].volume, 2.0);
    // close первого бакета остаётся 10.0 (порядок не восстанавливаем).
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
    // Сервер: бакеты 0 и 5м. Трейды начинаются ПОСРЕДИ бакета 5м (частичный) и
    // покрывают бакет 10м целиком.
    let server = [
        candle(0.0, 1.0, 2.0, 0.5, 1.5, 10.0),
        candle(5.0 * M, 1.5, 3.0, 1.0, 2.0, 10.0),
    ];
    let trades = [
        tick(7.0 * M, 100.0, 1.0), // частичный бакет 5м — врёт (o=100)
        tick(10.0 * M, 2.0, 1.0),
        tick(12.0 * M, 2.5, 1.0),
    ];
    let mut s = CandleSeries::default();
    s.rebuild(TF5, &server, TF5, &trades);
    let c = s.candles();
    assert_eq!(c.len(), 3);
    // Бакет 5м — серверный (частичный локальный отброшен).
    assert_eq!(c[1], server[1]);
    // Бакет 10м — локальный.
    assert_eq!(c[2].t_open_ms, 10.0 * M);
    assert_eq!(c[2].open, 2.0);
    assert_eq!(c[2].close, 2.5);
}

#[test]
fn rebuild_without_server_takes_partial_first() {
    let trades = [tick(7.0 * M, 5.0, 1.0), tick(12.0 * M, 6.0, 1.0)];
    let mut s = CandleSeries::default();
    s.rebuild(60_000, &[], TF5, &trades); // 1м ТФ — базы нет
    assert_eq!(s.candles().len(), 2);
    assert_eq!(s.candles()[0].t_open_ms, 7.0 * M);
}

#[test]
fn rebuild_resamples_native_base_tf() {
    // База родного ТФ 30м, серия 60м → ресемпл пар.
    let base = [
        candle(0.0, 1.0, 3.0, 0.5, 2.0, 1.0),
        candle(30.0 * M, 2.0, 4.0, 1.5, 3.0, 1.0),
    ];
    let mut s = CandleSeries::default();
    s.rebuild(60 * 60_000, &base, 30 * 60_000, &[]);
    assert_eq!(s.candles(), &[candle(0.0, 1.0, 4.0, 0.5, 3.0, 2.0)]);
    // Некратная база (серия 5м, база 30м) — игнорируется.
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
    assert!(s.push_trades(&[tick(6.0 * M, 8.0, 1.0)])); // кросс-бакет → новая свеча
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
    // Окно только по второму бакету — экстремумы первого не попадают.
    let r = s.price_range(5.0 * M, 9.0 * M).unwrap();
    assert_eq!(r, (1.5, 3.0));
    assert_eq!(s.price_range(0.0, 20.0 * M).unwrap(), (0.5, 100.0));
}

#[test]
fn normalize_ohlc_unswaps_server_wire_order() {
    // Корректная свеча — как есть.
    assert_eq!(
        normalize_ohlc(10.0, 12.0, 9.0, 11.0),
        (10.0, 12.0, 9.0, 11.0)
    );
    // Свап (o,c,h,l) = (high, low, open, close): реальная свеча o=10 h=12 l=9 c=11
    // приходит как o=12, c=9, h=10, l=11.
    let (o, h, l, c) = normalize_ohlc(12.0, 10.0, 11.0, 9.0);
    assert_eq!((o, h, l, c), (10.0, 12.0, 9.0, 11.0));
    // Чистое падение без фитилей (o==h, c==l) — неотличимо от корректной и не ломается.
    assert_eq!(normalize_ohlc(12.0, 12.0, 9.0, 9.0), (12.0, 12.0, 9.0, 9.0));
    // Плоская свеча.
    assert_eq!(normalize_ohlc(5.0, 5.0, 5.0, 5.0), (5.0, 5.0, 5.0, 5.0));
    // Мусор: диапазон растягивается, o/c как есть.
    let (o, h, l, c) = normalize_ohlc(10.0, 11.0, 12.0, 9.5);
    assert_eq!((o, c), (10.0, 9.5));
    assert_eq!((h, l), (12.0, 9.5));
}

#[test]
fn orient_range_rows_by_direction() {
    // Снимочные строки «только диапазон» (open==high, close==low).
    let mut rows = vec![
        candle(0.0, 12.0, 12.0, 10.0, 10.0, 1.0), // первая — как пришла (вниз)
        candle(5.0 * M, 14.0, 14.0, 12.0, 12.0, 1.0), // mid 13 > 11 → вверх
        candle(10.0 * M, 12.0, 12.0, 9.0, 9.0, 1.0), // mid 10.5 < 13 → вниз
    ];
    orient_range_rows(&mut rows);
    assert_eq!((rows[0].open, rows[0].close), (12.0, 10.0));
    assert_eq!((rows[1].open, rows[1].close), (12.0, 14.0)); // развёрнута вверх
    assert_eq!((rows[2].open, rows[2].close), (12.0, 9.0));
    // Честная свеча (с фитилями) не трогается.
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
    // Неизвестный/убранный ТФ клампится к 5м (легаси 15м из старых конфигов).
    let bad = CandleViewCfg { tf_min: 15, ..cfg };
    assert_eq!(bad.tf_ms(), TF5);
    // Легаси 30с (код 0) сведён к 1м; сутки.
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
