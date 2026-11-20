use super::super::test_bars::{MONDAY_2024_01_01_MS, ace};
use super::*;

/// One hour, ms.
const H1: f64 = 3_600_000.0;

/// An hourly bar opening `hours` after Monday 2024-01-01 00:00 UTC.
fn hour_bar(hours: i64, open: f32, close: f32) -> ChartCandle {
    ChartCandle {
        t_open_ms: MONDAY_2024_01_01_MS + hours as f64 * H1,
        open,
        high: open.max(close) + 0.25,
        low: open.min(close) - 0.25,
        close,
        volume: 1.0,
        quote_volume: 1.0,
    }
}

/// Hourly bars at the given hour offsets, bar `h` opening at `10 + h` and closing at `10.5 + h`, so
/// every open and close names its bar.
fn hours(offsets: impl IntoIterator<Item = i64>) -> Vec<ChartCandle> {
    offsets
        .into_iter()
        .map(|h| hour_bar(h, 10.0 + h as f32, 10.5 + h as f32))
        .collect()
}

const ALL: PeriodLevelParams = PeriodLevelParams {
    prev_day_close: true,
    day_open: true,
    week_open: true,
    prev_week_close: true,
};

fn run(bars: &[ChartCandle], now_ms: f64) -> Vec<(LineTag, f64)> {
    PeriodLevel
        .detect(
            bars,
            &DetectorParams::PeriodLevel(ALL),
            &DetectCtx {
                tf_ms: 3_600_000,
                now_ms,
            },
        )
        .into_iter()
        .map(|f| {
            let FoundShape::Horizontal { price } = f.shape else {
                panic!("a level");
            };
            (f.tag.expect("tagged"), price)
        })
        .collect()
}

/// Monday 00:00 .. Tuesday 05:00, hourly, read at Tuesday 06:00 (2024-01-01 is a Monday). By hand:
/// Monday's last bar is hour 23, closing at 33.5; Tuesday opens with hour 24 at 34; the week opens
/// with hour 0 at 10; the week before holds no bars, so it has no close.
///
/// Breakage: the close read from the wrong bar or as its open, the week cut on Sunday, or a week
/// without bars inventing a close.
#[test]
fn opens_and_closes_by_hand() {
    let bars = hours(0..30);
    let got = run(&bars, MONDAY_2024_01_01_MS + 30.0 * H1);
    assert_eq!(
        got,
        vec![
            (LineTag::PrevDayClose, 33.5),
            (LineTag::DayOpen, 34.0),
            (LineTag::WeekOpen, 10.0),
        ]
    );
}

/// A hole at a period's edge leaves that price out — and only that one: Monday's 23:00 bar missing
/// (its 22:00 bar closes at 23:00, short of midnight), Tuesday's 00:00 missing (its first bar opens
/// an hour late), Monday's 00:00 missing (the week's first bar is gone).
///
/// Breakage: the price of the bar next to a hole reported as the period's open or close.
#[test]
fn a_hole_at_the_edge_gives_nothing_for_that_price() {
    let now = MONDAY_2024_01_01_MS + 30.0 * H1;
    let without = |gone: i64| hours((0..30).filter(move |&h| h != gone));
    assert_eq!(
        run(&without(23), now),
        vec![(LineTag::DayOpen, 34.0), (LineTag::WeekOpen, 10.0)]
    );
    assert_eq!(
        run(&without(24), now),
        vec![(LineTag::PrevDayClose, 33.5), (LineTag::WeekOpen, 10.0)]
    );
    assert_eq!(
        run(&without(0), now),
        vec![(LineTag::PrevDayClose, 33.5), (LineTag::DayOpen, 34.0)]
    );
}

/// The previous week's close is Sunday's last bar: bars from Sunday 22:00 to Monday 02:00 read on
/// Monday at 03:00 give Sunday 23:00's close (−1 hour: open 9, close 9.5); Monday 00:00 is both the
/// day's and the week's open (10). Sunday 23:00 is also the previous day's last bar.
///
/// Breakage: an ISO week cut anywhere but Monday 00:00 UTC.
#[test]
fn the_previous_week_closes_on_sunday() {
    let bars = hours(-2..3);
    let got = run(&bars, MONDAY_2024_01_01_MS + 3.0 * H1);
    assert_eq!(
        got,
        vec![
            (LineTag::PrevDayClose, 9.5),
            (LineTag::DayOpen, 10.0),
            (LineTag::WeekOpen, 10.0),
            (LineTag::PrevWeekClose, 9.5),
        ]
    );
}

/// A sub-cent price comes through unrounded, an unusable bar length finds nothing, an unreadable
/// edge bar counts as missing.
///
/// Breakage: a price rounded to the chart's precision on the way, a zero bar length read as
/// "every bar closes the period", or a NaN open drawn as a level.
#[test]
fn tiny_prices_bad_lengths_and_bad_bars() {
    let mut bars = hours(0..30);
    for b in &mut bars {
        b.open *= 1e-6;
        b.high *= 1e-6;
        b.low *= 1e-6;
        b.close *= 1e-6;
    }
    let now = MONDAY_2024_01_01_MS + 30.0 * H1;
    let got = run(&bars, now);
    assert_eq!(got[0], (LineTag::PrevDayClose, f64::from(33.5f32 * 1e-6)));
    assert!(got[0].1 < 1e-4);

    let zero = PeriodLevel.detect(
        &bars,
        &DetectorParams::PeriodLevel(ALL),
        &DetectCtx {
            tf_ms: 0,
            now_ms: now,
        },
    );
    assert!(zero.is_empty());

    let mut bad = hours(0..30);
    bad[24].open = f32::NAN;
    assert_eq!(
        run(&bad, now),
        vec![(LineTag::PrevDayClose, 33.5), (LineTag::WeekOpen, 10.0)]
    );
}

/// On the real `chart-ace` 5-minute candles: read just after midnight of day 20681 (a Sunday), the
/// previous day's close, the day's open, the week's open (Monday, day 20675) and the previous
/// week's close are the closes and opens of the bars at those exact boundaries, looked up here by
/// time. Read on day 20669, the previous day 20668 — whose recorded bars stop at 21:40 — has no
/// close, a hole in the cache the detector must not paper over.
///
/// Breakage: a day or week cut off by one, a close taken from a bar that does not reach the
/// period's end.
#[test]
fn bench_boundaries_match_the_bars_at_them() {
    let bars = ace(5);
    const DAY: f64 = 86_400_000.0;
    const M5: f64 = 300_000.0;
    let at = |t: f64| {
        bars.iter()
            .find(|b| b.t_open_ms == t)
            .expect("the bench has this bar")
    };
    let now = 20_681.0 * DAY + 1_800_000.0;
    let upto: Vec<ChartCandle> = bars.iter().filter(|b| b.t_open_ms < now).copied().collect();
    let found = PeriodLevel.detect(
        &upto,
        &DetectorParams::PeriodLevel(ALL),
        &DetectCtx {
            tf_ms: 300_000,
            now_ms: now,
        },
    );
    let got: Vec<(LineTag, f64)> = found
        .iter()
        .map(|f| {
            (
                f.tag.expect("tagged"),
                f.shape.price_at(now).expect("price"),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (
                LineTag::PrevDayClose,
                f64::from(at(20_681.0 * DAY - M5).close)
            ),
            (LineTag::DayOpen, f64::from(at(20_681.0 * DAY).open)),
            (LineTag::WeekOpen, f64::from(at(20_675.0 * DAY).open)),
            (
                LineTag::PrevWeekClose,
                f64::from(at(20_675.0 * DAY - M5).close)
            ),
        ]
    );

    let now = 20_669.0 * DAY + 20_000_000.0;
    let upto: Vec<ChartCandle> = bars.iter().filter(|b| b.t_open_ms < now).copied().collect();
    let only_prev_day = DetectorParams::PeriodLevel(PeriodLevelParams {
        prev_day_close: true,
        day_open: false,
        week_open: false,
        prev_week_close: false,
    });
    let ctx = DetectCtx {
        tf_ms: 300_000,
        now_ms: now,
    };
    assert!(PeriodLevel.detect(&upto, &only_prev_day, &ctx).is_empty());
}
