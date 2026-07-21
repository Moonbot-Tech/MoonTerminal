use super::*;

fn candle(t: f64, p: f32) -> ChartCandle {
    ChartCandle {
        t_open_ms: t,
        open: p,
        high: p + 1.0,
        low: p - 1.0,
        close: p + 0.5,
        volume: 10.0,
    }
}

#[test]
fn pack_unpack_roundtrip() {
    let day_start = 19_000i64 * DAY_MS;
    let rows = vec![
        candle(day_start as f64, 5.0),
        candle((day_start + 60_000) as f64, 6.0),
    ];
    let blob = pack_rows(rows.iter(), day_start);
    assert_eq!(blob.len(), 2 * ROW_BYTES);
    let back = unpack_rows(&blob, day_start);
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].t_open_ms, rows[0].t_open_ms);
    assert_eq!(back[1].open, 6.0);
    assert_eq!(back[1].close, 6.5);
}

#[test]
fn merge_dedups_and_read_filters() {
    let dir = std::env::temp_dir().join(format!("kline-cache-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("klines-test.sqlite");
    let _ = std::fs::remove_file(&path);
    let cache = KlineCache::open(path.clone()).expect("open cache");
    let day = (now_unix_ms() / DAY_MS) * DAY_MS;
    // Две записи с перекрытием: вторая заливка обновляет свечу t=day.
    cache.merge(
        "7:0".into(),
        "BTCUSDT".into(),
        1,
        vec![candle(day as f64, 5.0), candle((day + 60_000) as f64, 6.0)],
    );
    cache.merge(
        "7:0".into(),
        "BTCUSDT".into(),
        1,
        vec![candle(day as f64, 9.0)],
    );
    // Дать потоку прожевать очередь: read идёт тем же каналом, порядок сохранён.
    let rows = cache.read_range("7:0", "BTCUSDT", 1, day, day + DAY_MS);
    assert_eq!(rows.len(), 2, "дедуп по t_open внутри дня");
    assert_eq!(rows[0].open, 9.0, "поздняя заливка авторитетнее");
    // Чужой kind/рынок не видны.
    assert!(cache
        .read_range("7:0", "BTCUSDT", 5, day, day + DAY_MS)
        .is_empty());
    assert!(cache
        .read_range("7:0", "ETHUSDT", 1, day, day + DAY_MS)
        .is_empty());
    let _ = std::fs::remove_file(&path);
}
