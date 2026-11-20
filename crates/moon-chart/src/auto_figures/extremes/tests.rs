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
        &DetectCtx {
            tf_ms: 3_600_000,
            now_ms: 0.0,
        },
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

/// On the real `chart-ace` 5-minute candles, read as 5-minute bars: the current and previous day's
/// extremes are the max high and min low of exactly the bars inside that UTC day, computed here
/// independently — on the series cut after day 20680, whose two last days are whole (288 bars
/// each). The bench's own last day, 20681, has 163 bars with empty stretches of 4 h 25 min
/// (06:25–10:50) and 5 h 50 min (12:05–17:55): it reports nothing.
///
/// Breakage: a period boundary off by a bar, a NaN/zero bar winning the minimum, or a recorded day
/// with hours missing reported as if it were whole.
#[test]
fn real_candles_match_an_independent_scan() {
    let day_ms = 86_400_000.0;
    let all = ace(5);
    let run_m5 = |bars: &[ChartCandle], period, now_ms| {
        prices(&PeriodExtreme.detect(
            bars,
            &DetectorParams::PeriodExtreme(PeriodExtremeParams {
                period,
                which: ExtremeWhich::Both,
            }),
            &DetectCtx {
                tf_ms: 300_000,
                now_ms,
            },
        ))
    };
    let cut = 20_681.0 * day_ms;
    let bars: Vec<ChartCandle> = all.iter().copied().filter(|b| b.t_open_ms < cut).collect();
    for (period, day) in [
        (ExtremePeriod::Day, 20_680.0),
        (ExtremePeriod::PrevDay, 20_679.0),
    ] {
        let (start, end) = (day * day_ms, (day + 1.0) * day_ms);
        let inside: Vec<ChartCandle> = bars
            .iter()
            .copied()
            .filter(|b| b.t_open_ms >= start && b.t_open_ms < end)
            .collect();
        assert_eq!(inside.len(), 288, "{period:?}: a whole bench day");
        let (lo, hi) = range(&inside);
        // A millisecond before midnight: day 20680 is still "today".
        assert_eq!(run_m5(&bars, period, cut - 1.0), vec![hi, lo], "{period:?}");
    }
    let last = all.last().expect("bars").t_open_ms;
    assert_eq!((last / day_ms).floor(), 20_681.0);
    assert!(run_m5(&all, ExtremePeriod::Day, last).is_empty());
}

/// "Today" is the reference time's day, not the last bar's: ten minutes after UTC midnight, with
/// no bar of the new day closed yet, `Day` has nothing and `PrevDay` is the day that just ended. A
/// stale reference earlier than the last bar falls back to that bar.
///
/// Breakage: reading "today" from the last closed bar, which keeps drawing yesterday's range as
/// today's until the first bar of the new day closes.
#[test]
fn today_follows_the_reference_time() {
    let monday: Vec<ChartCandle> = (0..24)
        .map(|h| {
            let high = if h == 7 { 130.0 } else { 110.0 };
            at(
                MONDAY_2024_01_01_MS + f64::from(h) * HOUR_MS,
                high,
                100.0,
                105.0,
            )
        })
        .collect();
    let run_at = |period, now_ms| {
        PeriodExtreme.detect(
            &monday,
            &DetectorParams::PeriodExtreme(PeriodExtremeParams {
                period,
                which: ExtremeWhich::High,
            }),
            &DetectCtx {
                tf_ms: 3_600_000,
                now_ms,
            },
        )
    };
    let tuesday_0010 = MONDAY_2024_01_01_MS + 24.0 * HOUR_MS + 600_000.0;
    assert!(run_at(ExtremePeriod::Day, tuesday_0010).is_empty());
    assert_eq!(
        prices(&run_at(ExtremePeriod::PrevDay, tuesday_0010)),
        vec![130.0]
    );
    // Monday afternoon as the reference — and a stale one from before the series — both read
    // Monday as today.
    let monday_1500 = MONDAY_2024_01_01_MS + 15.0 * HOUR_MS;
    assert_eq!(
        prices(&run_at(ExtremePeriod::Day, monday_1500)),
        vec![130.0]
    );
    assert_eq!(prices(&run_at(ExtremePeriod::Day, 1.0)), vec![130.0]);
    assert_eq!(prices(&run_at(ExtremePeriod::Day, f64::NAN)), vec![130.0]);
}

const M5: f64 = 300_000.0;
const DAY: f64 = 86_400_000.0;

/// 5-minute bars spanning 100–110 and closing at 105, from `from` to `to` (exclusive), both ms
/// after Monday 2024-01-01 00:00 UTC.
fn m5_bars(from: f64, to: f64) -> Vec<ChartCandle> {
    let n = ((to - from) / M5).round() as usize;
    (0..n)
        .map(|i| {
            at(
                MONDAY_2024_01_01_MS + from + i as f64 * M5,
                110.0,
                100.0,
                105.0,
            )
        })
        .collect()
}

/// The prices `period` reports on 5-minute `bars` judged at `now` ms after Monday 00:00.
fn run_5m(bars: &[ChartCandle], period: ExtremePeriod, now: f64) -> Vec<f64> {
    prices(&PeriodExtreme.detect(
        bars,
        &DetectorParams::PeriodExtreme(PeriodExtremeParams {
            period,
            which: ExtremeWhich::Both,
        }),
        &DetectCtx {
            tf_ms: M5 as i64,
            now_ms: MONDAY_2024_01_01_MS + now,
        },
    ))
}

/// A period is judged by ITS OWN bars, not by where the input starts: a Tuesday the cache holds
/// three bars of (10:35, 15:00, 19:35 — the BTCUSDT cache of 19.11.2026, whose "yesterday's high"
/// came out 1.1 % under the real one) reports nothing, and neither does the week those three bars
/// leave short, while Wednesday's whole first six hours are its current day.
///
/// By hand: Monday has 288 bars, Tuesday 3, Wednesday 72 (to 06:00). Tuesday's first bar is 10:35,
/// ten hours after its start. The week so far, Monday 00:00 to Wednesday 06:00, has an empty
/// stretch of 10 h 35 min from Monday's last close to Tuesday's first bar. Wednesday up to its last
/// closed bar: no empty stretch at all.
///
/// Breakage: completeness read from the input's first bar (Monday covers every period that starts
/// after it), or a period's bars never counted against what it should hold.
#[test]
fn a_period_is_judged_by_its_own_bars() {
    let mut bars = m5_bars(0.0, DAY);
    for t in [
        10.0 * HOUR_MS + 35.0 * 60_000.0,
        15.0 * HOUR_MS,
        19.0 * HOUR_MS + 35.0 * 60_000.0,
    ] {
        bars.push(at(MONDAY_2024_01_01_MS + DAY + t, 104.0, 101.0, 102.0));
    }
    bars.extend(m5_bars(2.0 * DAY, 2.0 * DAY + 6.0 * HOUR_MS));
    let now = 2.0 * DAY + 6.0 * HOUR_MS;
    assert!(run_5m(&bars, ExtremePeriod::PrevDay, now).is_empty());
    assert!(run_5m(&bars, ExtremePeriod::Week, now).is_empty());
    assert_eq!(run_5m(&bars, ExtremePeriod::Day, now), vec![110.0, 100.0]);
}

/// A period missing its start, its end or a stretch inside reports nothing, each with the day
/// before it whole: Wednesday from 07:50; Tuesday from 03:00; Tuesday with no bars from 06:00 to
/// 14:00 (an eight-hour stretch); Tuesday with nothing after 22:55 (more than an hour to its end).
///
/// Breakage: a period checked only at its start (the hole inside and the missing end pass), or
/// only against the input's first bar (every one of them passes).
#[test]
fn a_period_missing_its_start_end_or_middle_is_not_reported() {
    let h = |x: f64| x * HOUR_MS;
    let mut late_start = m5_bars(0.0, 2.0 * DAY);
    late_start.extend(m5_bars(
        2.0 * DAY + h(7.0) + 50.0 * 60_000.0,
        2.0 * DAY + h(12.0),
    ));
    let noon = 2.0 * DAY + h(12.0);
    assert!(run_5m(&late_start, ExtremePeriod::Day, noon).is_empty());
    assert_eq!(
        run_5m(&late_start, ExtremePeriod::PrevDay, noon),
        vec![110.0, 100.0],
        "Tuesday is whole"
    );

    let one_am = 2.0 * DAY + h(1.0);
    let mut no_night = m5_bars(0.0, DAY);
    no_night.extend(m5_bars(DAY + h(3.0), 2.0 * DAY + h(1.0)));
    assert!(run_5m(&no_night, ExtremePeriod::PrevDay, one_am).is_empty());

    let mut holed = m5_bars(0.0, DAY + h(6.0));
    holed.extend(m5_bars(DAY + h(14.0), 2.0 * DAY + h(1.0)));
    assert!(run_5m(&holed, ExtremePeriod::PrevDay, one_am).is_empty());

    let mut no_end = m5_bars(0.0, 2.0 * DAY - h(1.0) - M5);
    no_end.extend(m5_bars(2.0 * DAY, 2.0 * DAY + h(1.0)));
    assert!(run_5m(&no_end, ExtremePeriod::PrevDay, one_am).is_empty());
}

/// A quiet coin's day: its first trade at 00:20 and its last bar opening at 23:30 leave less than
/// an hour of nothing at either edge, so the day is whole and its extremes are reported.
///
/// Breakage: the edges held to one bar length again, which hides every illiquid coin's day.
#[test]
fn a_quiet_start_and_end_still_cover_the_day() {
    let h = |x: f64| x * HOUR_MS;
    let mut quiet = m5_bars(0.0, DAY);
    quiet.extend(m5_bars(DAY + 20.0 * 60_000.0, DAY + h(23.5) + M5));
    let one_am = 2.0 * DAY + h(1.0);
    assert_eq!(
        run_5m(&quiet, ExtremePeriod::PrevDay, one_am),
        vec![110.0, 100.0]
    );
}

/// A two-hour hole hides a period even when nearly every bar is there: Tuesday without 12:00–14:00
/// is 264 of its 288 bars (91.7 %), and reports nothing — the real high may sit in those two hours.
/// The limit is an hour of nothing: Tuesday without 12:00–13:00 (twelve bars, an empty hour between
/// 11:55's close and 13:00) reports its high, a 120 bar at 15:00; one bar more, 65 minutes, nothing.
///
/// Breakage: completeness read as a share of the bars present (264 of 288 passes), or the hour
/// limit off by a bar either way.
#[test]
fn a_two_hour_hole_hides_the_period() {
    let h = |x: f64| x * HOUR_MS;
    let without = |from: f64, to: f64| {
        let mut bars = m5_bars(0.0, DAY);
        for mut b in m5_bars(DAY, 2.0 * DAY) {
            let t = b.t_open_ms - MONDAY_2024_01_01_MS - DAY;
            if t >= from && t < to {
                continue;
            }
            if t == h(15.0) {
                b.high = 120.0;
            }
            bars.push(b);
        }
        bars.extend(m5_bars(2.0 * DAY, 2.0 * DAY + HOUR_MS));
        bars
    };
    let one_am = 2.0 * DAY + HOUR_MS;
    let two_hours = without(h(12.0), h(14.0));
    assert_eq!(two_hours.len(), 288 + 264 + 12);
    assert!(run_5m(&two_hours, ExtremePeriod::PrevDay, one_am).is_empty());
    assert!(
        run_5m(&two_hours, ExtremePeriod::Week, one_am).is_empty(),
        "the week holds the same hole"
    );
    let one_hour = without(h(12.0), h(13.0));
    assert_eq!(
        run_5m(&one_hour, ExtremePeriod::PrevDay, one_am),
        vec![120.0, 100.0]
    );
    let one_bar_more = without(h(12.0), h(13.0) + M5);
    assert!(run_5m(&one_bar_more, ExtremePeriod::PrevDay, one_am).is_empty());
}

/// A quiet market's day is covered: the cache holds a bar only where something traded, so a coin
/// that trades in four 5-minute slots out of every ten has 117 of Tuesday's 288 bars (40.6 %), with
/// empty stretches of 30 minutes at most, its first bar at 00:00 and its last at 23:55 — and its
/// high (a 120 bar at 12:40) and low are reported.
///
/// By hand: slots `i % 10 < 4` of 0..288 are 28 × 4 + 4 = 116 bars, plus slot 287 (23:55): 117.
/// Inside, six empty slots (30 min) after every fourth bar, and three (15 min) before 23:55.
///
/// Breakage: completeness read as a share of the bars present (40 % fails any share worth the
/// name), or a quiet stretch read as a hole.
#[test]
fn a_quiet_market_covers_its_day() {
    let mut bars = m5_bars(0.0, DAY);
    for (i, mut b) in m5_bars(DAY, 2.0 * DAY).into_iter().enumerate() {
        if i % 10 >= 4 && i != 287 {
            continue;
        }
        if i == 152 {
            b.high = 120.0;
        }
        bars.push(b);
    }
    assert_eq!(bars.len(), 288 + 117);
    bars.extend(m5_bars(2.0 * DAY, 2.0 * DAY + HOUR_MS));
    let one_am = 2.0 * DAY + HOUR_MS;
    assert_eq!(
        run_5m(&bars, ExtremePeriod::PrevDay, one_am),
        vec![120.0, 100.0]
    );
}

/// The current day is covered up to "now": Wednesday whole from 00:00 to 04:00 and nothing after
/// reports its high and low at 05:00 — an hour since the last close, which a quiet market leaves —
/// and nothing at 06:00, two hours of nothing up to now, where today's real high may sit.
///
/// Breakage: the current period measured only up to its own last bar (a stale "today's high"
/// reported after hours of a missing record), or the tail measured without the bar still forming.
#[test]
fn the_current_day_needs_its_recent_bars() {
    let h = |x: f64| x * HOUR_MS;
    let mut bars = m5_bars(0.0, 2.0 * DAY);
    bars.extend(m5_bars(2.0 * DAY, 2.0 * DAY + h(4.0)));
    assert_eq!(
        run_5m(&bars, ExtremePeriod::Day, 2.0 * DAY + h(5.0)),
        vec![110.0, 100.0]
    );
    assert!(run_5m(&bars, ExtremePeriod::Day, 2.0 * DAY + h(6.0)).is_empty());
}
