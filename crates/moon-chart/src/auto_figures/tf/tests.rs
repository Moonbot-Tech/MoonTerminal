use super::*;

const H: f64 = 3_600_000.0;

/// The table row by row at its boundaries.
///
/// Breakage: a `>` where `>=` belongs shifts every boundary, or the table read in the wrong order.
#[test]
fn table_boundaries() {
    let at = |hours: f64| auto_tf(hours * H);
    assert_eq!(at(3.99), TfId::M1);
    assert_eq!(at(4.0), TfId::M5);
    assert_eq!(at(15.99), TfId::M5);
    assert_eq!(at(16.0), TfId::M15);
    assert_eq!(at(48.0), TfId::M30);
    assert_eq!(at(96.0), TfId::H1);
    assert_eq!(at(192.0), TfId::H4);
    assert_eq!(at(767.0), TfId::H4);
    assert_eq!(at(768.0), TfId::D1);
    assert_eq!(auto_tf(f64::NAN), TfId::M1);
    assert_eq!(auto_tf(-1.0), TfId::M1);
}

/// A span oscillating just around a threshold never flips; one moved past the margin does, both
/// ways.
///
/// Breakage: hysteresis applied in one direction only, or a margin of zero.
#[test]
fn hysteresis_holds_around_a_threshold_and_yields_past_the_margin() {
    let mut auto = AutoTf::new();
    assert_eq!(auto.update(15.0 * H), TfId::M5);
    for span in [16.1, 15.9, 17.0, 15.0, 19.9, 13.0] {
        assert_eq!(auto.update(span * H), TfId::M5, "span {span} h");
    }
    assert_eq!(
        auto.update(20.1 * H),
        TfId::M15,
        "16 h × 1.25 = 20 h crossed"
    );
    for span in [15.9, 13.0, 16.5] {
        assert_eq!(auto.update(span * H), TfId::M15, "span {span} h");
    }
    assert_eq!(
        auto.update(12.7 * H),
        TfId::M5,
        "16 h / 1.25 = 12.8 h crossed"
    );
    assert_eq!(auto.current(), Some(TfId::M5));
}

/// A zoom across several thresholds lands on the far timeframe in one step; a bad span changes
/// nothing; `Fixed` ignores the span and leaves the automatic state alone.
///
/// Breakage: stepping one row per update, a NaN span resetting the choice, or `Fixed` writing the
/// automatic state.
#[test]
fn big_jumps_bad_spans_and_fixed_rows() {
    let mut auto = AutoTf::new();
    auto.update(1.0 * H);
    assert_eq!(auto.update(1_000.0 * H), TfId::D1);
    assert_eq!(auto.update(f64::NAN), TfId::D1);
    assert_eq!(auto.update(0.0), TfId::D1);
    assert_eq!(auto.current(), Some(TfId::D1));
}

/// Where the bars come from, case by case.
///
/// Breakage: a chart series used for a finer row, a resample from a series too shallow for the
/// lookback, or a 1-minute row sent to the 5-minute cache.
#[test]
fn bar_sources() {
    let m5 = TfId::M5.ms();
    let day = 24.0 * H;
    let src = |row, chart_tf, history, lookback| bar_source(row, chart_tf, history, lookback, true);
    // 201 five-minute bars cover a 200-bar lookback (+1, as for a resample).
    assert_eq!(
        src(TfId::M5, m5, 201.0 * m5 as f64, 200),
        BarSource::ChartSeries
    );
    assert_eq!(src(TfId::D1, m5, 1e15, 200), BarSource::CacheDaily);
    assert_eq!(
        src(TfId::D1, TfId::D1.ms(), 1e15, 200),
        BarSource::ChartSeries
    );
    // 200 hourly bars = 200 h of history: present, then missing.
    assert_eq!(src(TfId::H1, m5, 9.0 * day, 200), BarSource::ChartResampled);
    assert_eq!(src(TfId::H1, m5, 2.0 * day, 200), BarSource::Cache5m);
    assert_eq!(src(TfId::H1, m5, f64::NAN, 200), BarSource::Cache5m);
    // A 5-minute row on an hourly chart cannot come from the chart: finer than its bars.
    assert_eq!(src(TfId::M5, TfId::H1.ms(), 1e15, 200), BarSource::Cache5m);
    assert_eq!(src(TfId::M1, m5, 1e15, 200), BarSource::Unavailable);
    assert_eq!(src(TfId::M15, 0, 0.0, 200), BarSource::Cache5m);
    // A 1-minute row on a 1-minute chart: nothing else can build it, however short the series.
    assert_eq!(
        src(TfId::M1, TfId::M1.ms(), 0.0, 200),
        BarSource::ChartSeries
    );
}

/// A row on the chart's own timeframe takes the series only when it covers the detector's need;
/// a shorter one hands the row to the cache, which the series' tail is appended to. The stand's
/// case: "Period levels" on M5 with the week open needs 2017 bars, the chart holds 647 — the
/// series never covers the week, the cache does. A row that only the chart can build (M1, or a
/// market with no cache at all) keeps the series whatever its depth.
///
/// Breakage: the old "equal timeframe → chart series" short-cut (the week open is never found
/// once the chart is open), or a fallback that sends an uncacheable row to the cache (it would
/// read nothing — no cache, or no 1-minute rows in it).
#[test]
fn an_equal_timeframe_series_too_short_for_the_row_gives_way_to_the_cache() {
    let m5 = TfId::M5.ms();
    let week_open = 2017;
    let chart = 647.0 * m5 as f64;
    assert_eq!(
        bar_source(TfId::M5, m5, chart, week_open, true),
        BarSource::Cache5m
    );
    // The boundary: the lookback plus one bar of history.
    let lookback = 646;
    assert_eq!(
        bar_source(TfId::M5, m5, chart, lookback, true),
        BarSource::ChartSeries
    );
    assert_eq!(
        bar_source(TfId::M5, m5, chart, lookback + 1, true),
        BarSource::Cache5m
    );
    // A daily row on a daily chart too short for it: the daily cache.
    let d1 = TfId::D1.ms();
    assert_eq!(
        bar_source(TfId::D1, d1, 10.0 * d1 as f64, 30, true),
        BarSource::CacheDaily
    );
    // Nothing but the chart can build these: the series, short or not.
    let m1 = TfId::M1.ms();
    assert_eq!(
        bar_source(TfId::M1, m1, 10.0 * m1 as f64, 200, true),
        BarSource::ChartSeries
    );
    assert_eq!(
        bar_source(TfId::M5, m5, chart, week_open, false),
        BarSource::ChartSeries
    );
    assert_eq!(
        bar_source(TfId::D1, d1, 10.0 * d1 as f64, 30, false),
        BarSource::ChartSeries
    );
}

/// A chart series exactly `lookback` row bars deep does not yield `lookback` COMPLETE row bars
/// when it starts mid-bucket, so it must not be chosen; one bucket more must.
///
/// The oracle counts complete hours of a 5-minute series directly: 200 h of bars starting five
/// minutes past an hour cover 199 whole hours (the first and the last hour are each one bar
/// short of twelve).
///
/// Breakage: sizing the resample's depth as `lookback × row` (the partial first bucket is dropped
/// by the bar assembly and the row silently runs on `lookback - 1` bars).
#[test]
fn a_resampled_chart_must_cover_one_bucket_more_than_the_lookback() {
    let m5 = TfId::M5.ms();
    let hour = TfId::H1.ms();
    let start = hour + m5; // five minutes past an hour
    let n = (200 * hour / m5) as usize; // exactly 200 h of 5-minute bars
    let mut per_hour = std::collections::BTreeMap::<i64, usize>::new();
    for i in 0..n {
        *per_hour
            .entry((start + i as i64 * m5).div_euclid(hour))
            .or_default() += 1;
    }
    let complete = per_hour.values().filter(|&&c| c == 12).count();
    assert_eq!(complete, 199);
    assert_eq!(
        bar_source(TfId::H1, m5, 200.0 * H, 200, true),
        BarSource::Cache5m
    );
    assert_eq!(
        bar_source(TfId::H1, m5, 201.0 * H, 200, true),
        BarSource::ChartResampled
    );
}
