use super::super::test_bars::{MONDAY_2024_01_01_MS, ace, bar, range};
use super::*;

const HOUR_MS: f64 = 3_600_000.0;

fn at(t_ms: f64, high: f32, low: f32, close: f32) -> ChartCandle {
    ChartCandle {
        t_open_ms: t_ms,
        ..bar(0, low, high, low, close)
    }
}

fn run(bars: &[ChartCandle], period: ExtremePeriod, which: ExtremeWhich) -> Vec<Found> {
    PeriodExtreme.detect(
        bars,
        &DetectorParams::PeriodExtreme(PeriodExtremeParams { period, which }),
        &DetectCtx { tf_ms: 3_600_000 },
    )
}

fn prices(found: &[Found]) -> Vec<f64> {
    found
        .iter()
        .map(|f| match f.shape {
            FoundShape::Horizontal { price } => price,
            other => panic!("extremes are horizontal, got {other:?}"),
        })
        .collect()
}

/// Calendar facts, independent of the code: 1970-01-01 was a Thursday, 2024-01-01 a Monday, and
/// 2024-01-07 the Sunday closing that ISO week.
///
/// Breakage: a week starting on Sunday or Thursday, or a truncating division on negative days.
#[test]
fn iso_weeks_start_on_monday_utc() {
    let monday = day_of(MONDAY_2024_01_01_MS);
    assert_eq!(week_start_day(monday), monday);
    assert_eq!(week_start_day(monday + 6), monday);
    assert_eq!(week_start_day(monday + 7), monday + 7);
    assert_eq!(
        week_start_day(0),
        -3,
        "1970-01-01 (Thu) belongs to the week of Mon 1969-12-29"
    );
    assert_eq!(day_of(-1.0), -1, "a moment before the epoch is in day -1");
}

/// Two days of hourly bars: the current day's and the previous day's high and low are the ones
/// written into the series, and a close beyond the previous high is reported as a break.
///
/// Breakage: a day cut at local time or at the first bar, the two days swapped, or `which`
/// ignored.
#[test]
fn current_and_previous_day() {
    let mut bars = Vec::new();
    for h in 0..48 {
        let t = MONDAY_2024_01_01_MS + f64::from(h) * HOUR_MS;
        let (high, low) = match h {
            5 => (120.0, 100.0),  // Monday's high
            9 => (105.0, 80.0),   // Monday's low
            30 => (130.0, 100.0), // Tuesday's high — and a close beyond Monday's 120
            40 => (105.0, 90.0),  // Tuesday's low
            _ => (110.0, 100.0),
        };
        let close = if h == 30 { 125.0 } else { 105.0 };
        bars.push(at(t, high, low, close));
    }
    assert_eq!(
        prices(&run(&bars, ExtremePeriod::Day, ExtremeWhich::Both)),
        vec![130.0, 90.0]
    );
    let prev = run(&bars, ExtremePeriod::PrevDay, ExtremeWhich::Both);
    assert_eq!(prices(&prev), vec![120.0, 80.0]);
    assert_eq!(prev[0].touches[0].t_ms, bars[5].t_open_ms);
    let broke = prev[0].broken.expect("Tuesday closed above Monday's high");
    assert_eq!(broke.at_ms, bars[30].t_open_ms);
    assert!(broke.returned, "and closed back below it later");
    assert!(
        prev[1].broken.is_none(),
        "nothing closed under Monday's low"
    );
    assert_eq!(
        prices(&run(&bars, ExtremePeriod::Day, ExtremeWhich::Low)),
        vec![90.0]
    );
    assert_eq!(
        prices(&run(&bars, ExtremePeriod::Week, ExtremeWhich::High)),
        vec![130.0]
    );
}

/// A period the series does not cover from its start is not reported — a partial extreme would
/// be drawn as if it were the real one.
///
/// Breakage: reporting the extreme of whatever bars happen to be present.
#[test]
fn a_partly_covered_period_is_not_reported() {
    let bars: Vec<ChartCandle> = (6..30)
        .map(|h| {
            at(
                MONDAY_2024_01_01_MS + f64::from(h) * HOUR_MS,
                110.0,
                100.0,
                105.0,
            )
        })
        .collect();
    assert!(run(&bars, ExtremePeriod::PrevDay, ExtremeWhich::Both).is_empty());
    assert!(run(&bars, ExtremePeriod::Week, ExtremeWhich::Both).is_empty());
    assert!(!run(&bars, ExtremePeriod::Day, ExtremeWhich::Both).is_empty());
    assert!(run(&[], ExtremePeriod::Day, ExtremeWhich::Both).is_empty());
}

/// On the real `chart-ace` candles: the current and previous day's extremes are the max high and
/// min low of exactly the bars inside that UTC day, computed here independently.
///
/// Breakage: a period boundary off by a bar, or a NaN/zero bar winning the minimum.
#[test]
fn real_candles_match_an_independent_scan() {
    let bars = ace(5);
    let last_day = (bars[bars.len() - 1].t_open_ms / 86_400_000.0).floor();
    for (period, day) in [
        (ExtremePeriod::Day, last_day),
        (ExtremePeriod::PrevDay, last_day - 1.0),
    ] {
        let (start, end) = (day * 86_400_000.0, (day + 1.0) * 86_400_000.0);
        let inside: Vec<ChartCandle> = bars
            .iter()
            .copied()
            .filter(|b| b.t_open_ms >= start && b.t_open_ms < end)
            .collect();
        let (lo, hi) = range(&inside);
        assert_eq!(
            prices(&run(&bars, period, ExtremeWhich::Both)),
            vec![hi, lo],
            "{period:?}"
        );
    }
}
