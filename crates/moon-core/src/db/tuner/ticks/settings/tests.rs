use super::*;

#[test]
fn the_defaults_are_the_measured_constants() {
    let d = ModelSettings::default();
    assert_eq!(d.entry_method, EntryMethod::Model);
    assert_eq!(d.latency_ms, DEFAULT_LATENCY_MS);
    assert_eq!(d.ticker_period_ms, TICKER_PERIOD_MS);
    assert_eq!(d.point_time_ms, POINT_TIME_TOLERANCE_MS);
    // The verdict compared `d.abs() <= PRICE_TOLERANCE * 100.0`; the setting holds that product.
    assert_eq!(d.price_pct, super::super::PRICE_TOLERANCE * 100.0);
    assert_eq!(d.stop_price_pct, STOP_PRICE_TOLERANCE * 100.0);
}

#[test]
fn sanitizing_keeps_the_clocks_off_zero_and_the_rest_off_negative() {
    let s = ModelSettings {
        ticker_period_ms: 0,
        series_tick_ms: -5,
        latency_ms: f64::NAN,
        price_pct: -1.0,
        shift_window_ms: -10,
        ..ModelSettings::default()
    }
    .sanitized();
    assert_eq!(s.ticker_period_ms, 1);
    assert_eq!(s.series_tick_ms, 1);
    assert_eq!(s.latency_ms, DEFAULT_LATENCY_MS);
    assert_eq!(s.price_pct, 0.0);
    assert_eq!(s.shift_window_ms, 0);
}

#[test]
fn a_file_missing_fields_reads_them_at_default() {
    let s: ModelSettings =
        serde_json::from_str(r#"{"latency_ms": 250.0, "entry_method": "Shift"}"#).unwrap();
    assert_eq!(s.latency_ms, 250.0);
    assert_eq!(s.entry_method, EntryMethod::Shift);
    assert_eq!(s.ticker_period_ms, TICKER_PERIOD_MS);
}

/// A finite value too large to add to a timestamp is clamped, not taken: the walk adds every
/// time to the trade's own stamps.
#[test]
fn sanitizing_caps_what_would_overflow_a_timestamp() {
    let s = ModelSettings {
        latency_ms: 1e30,
        ticker_period_ms: i64::MAX,
        pump_peak_lookback_ms: i64::MAX,
        price_pct: 1e9,
        ..ModelSettings::default()
    }
    .sanitized();
    assert_eq!(s.latency_ms, MAX_SETTING_MS as f64);
    assert_eq!(s.ticker_period_ms, MAX_SETTING_MS);
    assert_eq!(s.pump_peak_lookback_ms, MAX_SETTING_MS);
    assert_eq!(s.price_pct, MAX_SETTING_PCT);
    let raw = ModelSettings {
        latency_ms: 1e30,
        ..ModelSettings::default()
    };
    assert_eq!(raw.latency_whole_ms(), MAX_SETTING_MS);
}
