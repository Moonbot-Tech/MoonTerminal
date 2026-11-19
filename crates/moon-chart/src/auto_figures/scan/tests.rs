use super::super::test_bars::{MONDAY_2024_01_01_MS, ace, bar, range};
use super::*;
use moon_core::config::auto_figures::{
    AutoFiguresCfg, ExtremePeriod, PeriodExtremeParams, RoundGrid, RoundLevelsParams, SwingParams,
    VolumeProfileParams,
};

const HOUR_MS: f64 = 3_600_000.0;
const MIN_MS: f64 = 60_000.0;

/// Twelve flat 5-minute rows per hour from [`MONDAY_2024_01_01_MS`] (also a 4-hour boundary), each
/// hour's rows at its close — so the hour's resampled bar is that price on open, high, low and
/// close.
fn hourly(closes: &[f32]) -> Vec<ChartCandle> {
    closes
        .iter()
        .enumerate()
        .flat_map(|(h, &c)| (0..12).map(move |j| bar(h * 12 + j, c, c, c, c)))
        .collect()
}

/// `RoundLevels` on the half-order grid within 10 %, two lines per side, nearest first.
///
/// On a last closed close of 3.55 or 3.45 (and 3.4) the only multiple of 0.5 inside ±10 % is 3.5:
/// 3.0 and 4.0 lie outside [3.195, 3.905], [3.105, 3.795] and [3.06, 3.74].
fn round_row(row_ix: usize, tf: TfId, marks: EventMarks) -> ScanRow {
    ScanRow {
        row_ix,
        params: DetectorParams::RoundLevels(RoundLevelsParams {
            grid: RoundGrid::Half,
            range_pct: 10.0,
        }),
        tf,
        take: Take::default(),
        marks,
    }
}

/// Marks with only `events` on, the distances at their defaults.
fn only(events: &[AutoEvent]) -> EventMarks {
    let mut m = EventMarks {
        approach: false,
        squeeze: false,
        breakout: false,
        false_break: false,
        bounce: false,
        role_flip: false,
        stop_hunt: false,
        confluence: false,
        ..EventMarks::default()
    };
    for e in events {
        match e {
            AutoEvent::Approach => m.approach = true,
            AutoEvent::Squeeze => m.squeeze = true,
            AutoEvent::Breakout => m.breakout = true,
            AutoEvent::FalseBreak => m.false_break = true,
            AutoEvent::Bounce => m.bounce = true,
            AutoEvent::RoleFlip => m.role_flip = true,
            AutoEvent::StopHunt => m.stop_hunt = true,
            AutoEvent::Confluence => m.confluence = true,
        }
    }
    m
}

/// The public answer of a scan, for comparing two of them.
type Answer = (
    Option<ScanHit>,
    Option<ScanLine>,
    u32,
    u32,
    u32,
    bool,
    Option<f64>,
);

fn answer(r: &ScanResult) -> Answer {
    (
        r.best,
        r.nearest,
        r.lines,
        r.rows_scanned,
        r.rows_short,
        r.short_history,
        r.last_price,
    )
}

fn scan(rows: &[ScanRow], bars: &[ChartCandle], now_ms: f64) -> ScanResult {
    let mut out = ScanResult::default();
    scan_market(rows, bars, now_ms, &mut out);
    out
}

/// Four closed hours at 3.4, 3.4, 3.4, 3.55, judged at the end of the fourth.
fn breakout_series() -> (Vec<ChartCandle>, f64) {
    (
        hourly(&[3.4, 3.4, 3.4, 3.55]),
        MONDAY_2024_01_01_MS + 4.0 * HOUR_MS,
    )
}

/// The 3.5 level crossed by the last hourly close: one kept line, a breakout, 1.408 % below price.
///
/// By hand: the hourly closes 3.4 → 3.55 cross 3.5 on the last bar, with the run before it on the
/// other side for three bars and no close back — a breakout, not a false break. Distance
/// (3.5 − 3.55) / 3.55 = −1.408 %; 1.408 % is past the 0.3 % approach, so no approach.
///
/// Breakage: the scan not reaching `line_events`, the distance sign flipped, or measured from the
/// line instead of the price.
#[test]
fn a_crossed_round_level_is_the_breakout() {
    let (bars, now) = breakout_series();
    let r = scan(&[round_row(0, TfId::H1, EventMarks::default())], &bars, now);
    let best = r.best.expect("a breakout");
    assert_eq!(best.event, AutoEvent::Breakout);
    assert_eq!(best.line.price, 3.5);
    assert_eq!(best.line.row_ix, 0);
    let nearest = r.nearest.expect("a line");
    assert!(
        (nearest.distance_pct + 1.408_45).abs() < 1e-4,
        "{}",
        nearest.distance_pct
    );
    assert_eq!(r.lines, 1);
    assert_eq!(
        (r.rows_scanned, r.rows_short, r.short_history),
        (1, 0, false)
    );
    assert!(matches(&r, &ScanFilter::Breakout));
    assert!(!matches(&r, &ScanFilter::FalseBreak));
    assert!(!matches(&r, &ScanFilter::Squeeze));
    assert!(matches(&r, &ScanFilter::Any));
}

/// Closes 3.4, 3.4, 3.55, 3.45: one bar above 3.5 and straight back — a false break, the line
/// 1.449 % ABOVE the price ((3.5 − 3.45) / 3.45).
///
/// Breakage: the false break reported as a breakout, or the sign of an above-price line lost.
#[test]
fn a_close_back_inside_is_the_false_break() {
    let bars = hourly(&[3.4, 3.4, 3.55, 3.45]);
    let now = MONDAY_2024_01_01_MS + 4.0 * HOUR_MS;
    let r = scan(&[round_row(0, TfId::H1, EventMarks::default())], &bars, now);
    assert_eq!(r.best.map(|b| b.event), Some(AutoEvent::FalseBreak));
    let d = r.nearest.expect("a line").distance_pct;
    assert!((d - 1.449_28).abs() < 1e-4, "{d}");
    assert!(matches(&r, &ScanFilter::FalseBreak));
    assert!(!matches(&r, &ScanFilter::Breakout));
}

/// The approach filter reads the nearest line's distance against its OWN percentage, whatever the
/// rows mark: the breakout's line is 1.408 % away, so 1.5 % passes and 1.4 % does not — though the
/// row's own approach (0.3 %) marks nothing.
///
/// Breakage: the filter reading the rows' `Approach` event, or the signed distance (−1.408 is
/// below every positive bound).
#[test]
fn the_approach_filter_reads_the_distance() {
    let (bars, now) = breakout_series();
    let r = scan(&[round_row(0, TfId::H1, EventMarks::default())], &bars, now);
    assert!(matches(&r, &ScanFilter::Approach { max_pct: 1.5 }));
    assert!(!matches(&r, &ScanFilter::Approach { max_pct: 1.4 }));
    assert!(!matches(&r, &ScanFilter::Approach { max_pct: f32::NAN }));
    assert!(!matches(&r, &ScanFilter::Approach { max_pct: -5.0 }));
}

/// Two rows on the same line: one marks only approach, at 2 % (so the 1.408 % line is an
/// approach), the other marks everything at the defaults (a breakout). The breakout wins whichever
/// row comes first, and reports ITS row.
///
/// Breakage: the first event found kept instead of the best one, or the order of `BEST_ORDER`
/// lost.
#[test]
fn the_best_event_wins_over_rows() {
    let (bars, now) = breakout_series();
    let approach = EventMarks {
        approach_pct: 2.0,
        ..only(&[AutoEvent::Approach])
    };
    let alone = scan(&[round_row(0, TfId::H1, approach)], &bars, now);
    assert_eq!(alone.best.map(|b| b.event), Some(AutoEvent::Approach));

    let rows = [
        round_row(0, TfId::H1, approach),
        round_row(1, TfId::H1, EventMarks::default()),
    ];
    let r = scan(&rows, &bars, now);
    let best = r.best.expect("an event");
    assert_eq!((best.event, best.line.row_ix), (AutoEvent::Breakout, 1));
    assert_eq!(r.lines, 2);

    let rows = [
        round_row(0, TfId::H1, EventMarks::default()),
        round_row(1, TfId::H1, approach),
    ];
    let best = scan(&rows, &bars, now).best.expect("an event");
    assert_eq!((best.event, best.line.row_ix), (AutoEvent::Breakout, 0));
}

/// The same 3.5 level on H1 and H4 is a confluence on both; on H1 twice it is not (the rule wants
/// a DIFFERENT timeframe). With only confluence marked, the best event is that, on the lower row.
///
/// Breakage: the confluence pass not run, run per row, or its flag set without the row's mark.
#[test]
fn confluence_spans_rows_of_different_timeframes() {
    let (bars, now) = breakout_series();
    let marks = only(&[AutoEvent::Confluence]);
    let r = scan(
        &[round_row(0, TfId::H1, marks), round_row(1, TfId::H4, marks)],
        &bars,
        now,
    );
    let best = r.best.expect("a confluence");
    assert_eq!((best.event, best.line.row_ix), (AutoEvent::Confluence, 0));
    let same_tf = scan(
        &[round_row(0, TfId::H1, marks), round_row(1, TfId::H1, marks)],
        &bars,
        now,
    );
    assert_eq!(same_tf.best, None);
    let unmarked = only(&[]);
    let r = scan(
        &[
            round_row(0, TfId::H1, unmarked),
            round_row(1, TfId::H4, unmarked),
        ],
        &bars,
        now,
    );
    assert_eq!(r.best, None);
    assert_eq!(r.lines, 2);
}

/// The hour still forming is not read: three closed hours at 3.4 and half of a fourth at 3.55.
/// The level is still 3.5 (from the last CLOSED close, 3.4), no breakout, and the last price is
/// the newest 5-minute close, 3.55 — so the line sits BELOW it.
///
/// Breakage: the forming bucket resampled into the bars (a breakout would appear), or the last
/// price taken from the row's bars instead of the 5-minute rows.
#[test]
fn the_forming_bucket_is_not_read() {
    let mut bars = hourly(&[3.4, 3.4, 3.4]);
    bars.extend((36..42).map(|i| bar(i, 3.55, 3.55, 3.55, 3.55)));
    let now = MONDAY_2024_01_01_MS + 3.5 * HOUR_MS;
    let r = scan(&[round_row(0, TfId::H1, EventMarks::default())], &bars, now);
    assert_eq!(r.best, None);
    assert_eq!(r.last_price, Some(f64::from(3.55_f32)));
    let line = r.nearest.expect("a line");
    assert_eq!(line.price, 3.5);
    assert!(line.distance_pct < 0.0);
}

/// The newest hour missing its last 5-minute row is left out for five minutes after its close
/// (the recorder may still write it), then taken as it is.
///
/// Breakage: a lagging bucket used at once (breakout at 4:01) or never (none at 4:06).
#[test]
fn a_lagging_newest_bucket_waits_up_to_the_cap() {
    let (mut bars, close) = breakout_series();
    bars.pop();
    let rows = [round_row(0, TfId::H1, EventMarks::default())];
    let early = scan(&rows, &bars, close + MIN_MS);
    assert_eq!(early.best, None);
    let late = scan(&rows, &bars, close + 6.0 * MIN_MS);
    assert_eq!(late.best.map(|b| b.event), Some(AutoEvent::Breakout));
}

/// 90 % of `bars_needed` is enough, less is "short": a 10-bar swing row runs on 9 closed hours and
/// not on 8 (0.9 × 10 = 9). A market where no row ran is `short_history`; one where another row
/// ran is not.
///
/// Breakage: the threshold read as 100 %, or `short_history` set by a single short row.
#[test]
fn short_history_is_ninety_percent_of_the_need() {
    assert!(history_full(9, 10));
    assert!(!history_full(8, 10));
    let swing = ScanRow {
        row_ix: 0,
        params: DetectorParams::Swing(SwingParams {
            lookback_bars: 10,
            ..SwingParams::default()
        }),
        tf: TfId::H1,
        take: Take::default(),
        marks: EventMarks::default(),
    };
    let nine = hourly(&[3.4; 9]);
    let r = scan(&[swing], &nine, MONDAY_2024_01_01_MS + 9.0 * HOUR_MS);
    assert_eq!(
        (r.rows_scanned, r.rows_short, r.short_history),
        (1, 0, false)
    );
    let eight = hourly(&[3.4; 8]);
    let r = scan(&[swing], &eight, MONDAY_2024_01_01_MS + 8.0 * HOUR_MS);
    assert_eq!(
        (r.rows_scanned, r.rows_short, r.short_history),
        (0, 1, true)
    );
    assert_eq!((r.best, r.nearest, r.lines), (None, None, 0));
    assert!(matches(&r, &ScanFilter::Any));
    assert!(!matches(&r, &ScanFilter::Approach { max_pct: 100.0 }));

    // Last week's extreme needs 14 days of hours; four hours are far short, the round row is not.
    let week = ScanRow {
        params: DetectorParams::PeriodExtreme(PeriodExtremeParams {
            period: ExtremePeriod::PrevWeek,
            ..PeriodExtremeParams::default()
        }),
        ..swing
    };
    let (bars, now) = breakout_series();
    let r = scan(
        &[week, round_row(1, TfId::H1, EventMarks::default())],
        &bars,
        now,
    );
    assert_eq!(
        (r.rows_scanned, r.rows_short, r.short_history),
        (1, 1, false)
    );
    assert_eq!(r.best.map(|b| b.event), Some(AutoEvent::Breakout));
}

/// Empty input never panics: no rows is "nothing to say", no bars is "short of history".
#[test]
fn empty_input_is_no_answer() {
    let (bars, now) = breakout_series();
    let r = scan(&[], &bars, now);
    assert_eq!(answer(&r).0, None);
    assert_eq!((r.lines, r.rows_scanned, r.rows_short), (0, 0, 0));
    assert!(!r.short_history);

    let r = scan(&[round_row(0, TfId::H1, EventMarks::default())], &[], now);
    assert!(r.short_history);
    assert_eq!((r.best, r.nearest, r.last_price), (None, None, None));

    let r = scan(
        &[round_row(0, TfId::H1, EventMarks::default())],
        &bars,
        f64::NAN,
    );
    assert!(r.short_history);
}

/// Unreadable 5-minute rows — zero, negative and NaN prices, `high < low`, a row repeating the last
/// open time, one from the past, one with a NaN open time — are skipped: the answer equals the
/// clean series'. Each is appended after the LAST clean row and judged two minutes past the
/// hour, where a row it let in would be the last price, or close the last hour (one minute after
/// the last clean row, it has closed by then). A series of nothing but bad rows is short of
/// history, with no price.
///
/// Breakage: a bad row taken as the last price or folded into the last bucket, or a panic.
#[test]
fn unreadable_rows_are_skipped() {
    let (clean, close) = breakout_series();
    let now = close + 2.0 * MIN_MS;
    let rows = [round_row(0, TfId::H1, EventMarks::default())];
    let want = answer(&scan(&rows, &clean, now));
    assert_eq!(want.0.map(|b| b.event), Some(AutoEvent::Breakout));
    let tail = clean[clean.len() - 1];
    let one_min_later = |o: f32, h: f32, l: f32, c: f32| ChartCandle {
        t_open_ms: tail.t_open_ms + MIN_MS,
        open: o,
        high: h,
        low: l,
        close: c,
        ..tail
    };
    let mut no_time = one_min_later(1.0, 1.0, 1.0, 1.0);
    no_time.t_open_ms = f64::NAN;
    let junk = [
        one_min_later(0.0, 0.0, 0.0, 0.0),
        one_min_later(3.4, 3.4, -1.0, 3.4),
        one_min_later(3.4, 3.4, 3.4, f32::NAN),
        one_min_later(3.4, 3.0, 3.8, 3.4),
        ChartCandle {
            close: 1.0,
            low: 1.0,
            ..tail
        },
        bar(2, 1.0, 1.0, 1.0, 1.0),
        no_time,
    ];
    for j in junk {
        let mut dirty = clean.clone();
        dirty.push(j);
        assert_eq!(answer(&scan(&rows, &dirty, now)), want, "{j:?}");
    }

    let bad: Vec<ChartCandle> = (0..48).map(|i| bar(i, 0.0, 0.0, 0.0, 0.0)).collect();
    let r = scan(&rows, &bad, now);
    assert!(r.short_history);
    assert_eq!(r.last_price, None);
}

/// From a set row: `Auto` is H1, `Fixed` is kept, both capped by the detector's coarsest
/// timeframe; hidden rows and M1 rows are skipped, and the set index is carried.
///
/// Breakage: `Auto` resolved to something else, the cap missed (a `Day` extreme on D1 never sees a
/// closed day), or `from_rows` renumbering past a skipped row.
#[test]
fn rows_resolve_their_timeframe() {
    let swing = AutoFigureRow::new(DetectorParams::Swing(SwingParams::default()));
    assert_eq!(swing.tf, TfSource::Auto);
    assert_eq!(ScanRow::from_row(0, &swing).map(|r| r.tf), Some(TfId::H1));

    let profile = AutoFigureRow::new(DetectorParams::VolumeProfile(VolumeProfileParams::default()));
    assert_eq!(ScanRow::from_row(0, &profile).map(|r| r.tf), Some(TfId::H1));

    let day = AutoFigureRow {
        tf: TfSource::Fixed(TfId::D1),
        ..AutoFigureRow::new(DetectorParams::PeriodExtreme(PeriodExtremeParams::default()))
    };
    assert_eq!(ScanRow::from_row(0, &day).map(|r| r.tf), Some(TfId::H4));

    let m15 = AutoFigureRow {
        tf: TfSource::Fixed(TfId::M15),
        ..swing.clone()
    };
    assert_eq!(ScanRow::from_row(0, &m15).map(|r| r.tf), Some(TfId::M15));

    let m1 = AutoFigureRow {
        tf: TfSource::Fixed(TfId::M1),
        ..swing.clone()
    };
    assert_eq!(ScanRow::from_row(0, &m1), None);

    let hidden = AutoFigureRow {
        visible: false,
        ..swing.clone()
    };
    assert_eq!(ScanRow::from_row(0, &hidden), None);

    let set = [swing.clone(), hidden, m15];
    let ix: Vec<usize> = ScanRow::from_rows(&set).iter().map(|r| r.row_ix).collect();
    assert_eq!(ix, [0, 2]);
}

/// On the real `chart-ace` 5-minute candles an `Auto` swing row scans exactly as the same row
/// pinned to H1, finds levels inside the candles' range, and a buffer that scanned another market
/// first gives the same answer as a fresh one. The shipped set runs on it without a panic.
///
/// Breakage: `Auto` not resolved to H1, scratch leaking from one market into the next, or the
/// cache's real holes and partial buckets breaking the bar assembly.
#[test]
fn bench_scan_is_stable_and_auto_is_h1() {
    let bars = ace(5);
    let first = bars[0].t_open_ms;
    let last = bars[bars.len() - 1].t_open_ms;
    let now = last + 5.0 * MIN_MS;
    // Half the hours the bench covers: comfortably inside its history.
    let lookback = ((last - first) / HOUR_MS / 2.0) as u32;
    assert!(lookback >= 20, "the bench covers {lookback} × 2 hours");
    let auto = AutoFigureRow::new(DetectorParams::Swing(SwingParams {
        lookback_bars: lookback,
        ..SwingParams::default()
    }));
    let fixed = AutoFigureRow {
        tf: TfSource::Fixed(TfId::H1),
        ..auto.clone()
    };
    let auto_rows = ScanRow::from_rows(&[auto]);
    let fixed_rows = ScanRow::from_rows(&[fixed]);

    let fresh = scan(&auto_rows, &bars, now);
    assert_eq!(answer(&fresh), answer(&scan(&fixed_rows, &bars, now)));
    assert_eq!((fresh.rows_scanned, fresh.short_history), (1, false));
    assert!(fresh.lines > 0, "the bench holds H1 levels");
    let (lo, hi) = range(&bars);
    let line = fresh.nearest.expect("a level");
    assert!(
        (lo..=hi).contains(&line.price),
        "{} in {lo}..{hi}",
        line.price
    );

    let mut reused = ScanResult::default();
    let (other, other_now) = breakout_series();
    scan_market(
        &[round_row(0, TfId::H1, EventMarks::default())],
        &other,
        other_now,
        &mut reused,
    );
    scan_market(&auto_rows, &bars, now, &mut reused);
    assert_eq!(answer(&reused), answer(&fresh));
    scan_market(&auto_rows, &bars, now, &mut reused);
    assert_eq!(answer(&reused), answer(&fresh));

    let shipped = ScanRow::from_rows(&AutoFiguresCfg::shipped().rows);
    assert_eq!(shipped.len(), AutoFiguresCfg::shipped().rows.len());
    let r = scan(&shipped, &bars, now);
    assert_eq!(r.rows_scanned + r.rows_short, shipped.len() as u32);
}

/// The copied-out answer carries every answer field of the result and filters the same way.
///
/// Breakage: `ScanResult::answer` dropping or swapping a field (the screener would show one
/// market's nearest line as its best, or lose the short-history mark), or `ScanAnswer::matches`
/// drifting from `matches`.
#[test]
fn the_copied_answer_is_the_result() {
    let (bars, now) = breakout_series();
    let r = scan(&[round_row(0, TfId::H1, EventMarks::default())], &bars, now);
    let copied = r.answer();
    assert_eq!(
        (
            copied.best,
            copied.nearest,
            copied.lines,
            copied.rows_scanned,
            copied.rows_short,
            copied.short_history,
            copied.last_price,
        ),
        answer(&r)
    );
    for filter in [
        ScanFilter::Any,
        ScanFilter::Squeeze,
        ScanFilter::Breakout,
        ScanFilter::FalseBreak,
        ScanFilter::Approach { max_pct: 1.5 },
        ScanFilter::Approach { max_pct: 1.4 },
    ] {
        assert_eq!(copied.matches(&filter), matches(&r, &filter), "{filter:?}");
    }
}
