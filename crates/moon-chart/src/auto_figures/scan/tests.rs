use super::super::test_bars::{MONDAY_2024_01_01_MS, ace, bar, range};
use super::*;
use moon_core::config::auto_figures::{
    AutoFiguresCfg, ExtremePeriod, FormationParams, PeriodExtremeParams, ProfilePeriod, RoundGrid,
    RoundLevelsParams, SwingParams, TakeSide, VolumeProfileParams,
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
    scan_with_ring(rows, bars, &[], now_ms)
}

/// [`scan`] with `ring` as the market's rows of the core's ring kind beside its real candles.
fn scan_with_ring(
    rows: &[ScanRow],
    bars: &[ChartCandle],
    ring: &[ChartCandle],
    now_ms: f64,
) -> ScanResult {
    let mut out = ScanResult::default();
    scan_market(rows, bars, ring, now_ms, None, &mut out);
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

/// A quiet hour is a bar: the cache holds a row only where something traded, so a fourth hour of
/// five 5-minute rows at 3.55 after three whole hours at 3.4, judged past the lag cap, is the bar of
/// those five trades, and its close breaks 3.5 — 1.408 % below price ((3.5 − 3.55) / 3.55), the
/// answer of the whole hour.
///
/// Breakage: a share-of-rows rule dropping the quiet hour (no breakout on a market that traded
/// through the level), or the scan resampling differently from the chart's job.
#[test]
fn a_quiet_hour_of_few_rows_is_a_bar() {
    let rows = [round_row(0, TfId::H1, EventMarks::default())];
    let now = MONDAY_2024_01_01_MS + 4.0 * HOUR_MS + 6.0 * MIN_MS;
    let mut bars = hourly(&[3.4, 3.4, 3.4]);
    bars.extend([36, 38, 41, 44, 47].map(|i| bar(i, 3.55, 3.55, 3.55, 3.55)));
    let r = scan(&rows, &bars, now);
    assert_eq!(r.best.map(|b| b.event), Some(AutoEvent::Breakout), "{r:?}");
    let d = r.nearest.expect("the 3.5 line").distance_pct;
    assert!((d + 1.408_45).abs() < 1e-4, "{d}");
}

/// A quiet day is profiled: Wednesday with one 5-minute row every half hour (8 of each H4 bucket's
/// 48) gives six H4 bars of what it traded, so the H4 profile of the previous day is Wednesday's,
/// at 3.59–3.61 — not nothing, and not Tuesday's 3.4.
///
/// By hand: Monday and Tuesday whole at 3.4, Wednesday's rows at 3.6 (range 3.59–3.61), Thursday's
/// first four hours whole at 3.4, judged just past them. The row needs 13 H4 bars (two days and
/// the bar after); Monday's 6, Tuesday's 6, Wednesday's 6 and Thursday's first give 19. Every line
/// of a profile lies inside its day's range.
///
/// Breakage: a share-of-rows rule dropping the quiet buckets (the day has no bars, the profile
/// nothing), or the coverage check reading the empty 5-minute slots as holes.
#[test]
fn a_quiet_day_is_profiled() {
    let profile = ScanRow {
        row_ix: 0,
        params: DetectorParams::VolumeProfile(VolumeProfileParams {
            period: ProfilePeriod::PrevDay,
            naked_poc_days: 0,
            ..VolumeProfileParams::default()
        }),
        tf: TfId::H4,
        take: Take::default(),
        marks: EventMarks::default(),
    };
    assert_eq!(profile.params.bars_needed(TfId::H4), 13);
    let day = 288;
    let flat = |i: usize, p: f32| bar(i, p, p + 0.01, p - 0.01, p);
    let mut bars: Vec<ChartCandle> = (0..2 * day).map(|i| flat(i, 3.4)).collect();
    bars.extend((0..48).map(|k| flat(2 * day + k * 6, 3.6)));
    bars.extend((3 * day..3 * day + 48).map(|i| flat(i, 3.4)));
    let now = MONDAY_2024_01_01_MS + 3.0 * 24.0 * HOUR_MS + 4.0 * HOUR_MS + 6.0 * MIN_MS;
    let r = scan(&[profile], &bars, now);
    assert_eq!((r.rows_scanned, r.rows_short), (1, 0), "{r:?}");
    assert!(r.lines > 0, "{r:?}");
    let price = r.nearest.expect("a profile line").price;
    assert!(
        (f64::from(3.59f32)..=f64::from(3.61f32)).contains(&price),
        "{price}"
    );
}

/// 90 % of `bars_needed` is enough, less is "short": a 10-bar swing row runs on 9 closed hours and
/// not on 8 (0.9 × 10 = 9). A market where no row ran is `short_history`; one where another row
/// ran is not.
///
/// Breakage: the threshold read as 100 %, or `short_history` set by a single short row.
#[test]
fn short_history_is_ninety_percent_of_the_need() {
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

/// A calendar row needs EVERY bar of its period, as on the chart: today's extremes on H1 need 25
/// closed hours, and 23 of them (92 %, past a sampling row's 90 %) leave the row short — the
/// column must not show a day high the chart, on the same bars, calls "not enough history".
///
/// Breakage: the scan reading one 90 % share for every row (`history_full` without the params).
#[test]
fn a_calendar_row_needs_its_whole_period() {
    let day = ScanRow {
        row_ix: 0,
        params: DetectorParams::PeriodExtreme(PeriodExtremeParams {
            period: ExtremePeriod::Day,
            ..PeriodExtremeParams::default()
        }),
        tf: TfId::H1,
        take: Take::default(),
        marks: EventMarks::default(),
    };
    assert_eq!(day.params.bars_needed(TfId::H1), 25);
    let short = hourly(&[3.4; 23]);
    let r = scan(&[day], &short, MONDAY_2024_01_01_MS + 23.0 * HOUR_MS);
    assert_eq!((r.rows_scanned, r.rows_short), (0, 1));
    let full = hourly(&[3.4; 25]);
    let r = scan(&[day], &full, MONDAY_2024_01_01_MS + 25.0 * HOUR_MS);
    assert_eq!((r.rows_scanned, r.rows_short), (1, 0));
}

/// A cache that ends more than [`STALE_CACHE_MS`] before now is marked with its lag, and its answer
/// passes no event filter — it still finds the breakout, which is of the moment the cache stops.
///
/// By hand: the newest 5-minute row of the breakout series closes at the end of the fourth hour.
/// Judged 15 minutes later the lag is exactly the bound — current; judged 3 hours later the lag is
/// 10 800 000 ms, and the Breakout, Squeeze, FalseBreak and Approach filters (even at 100 %) turn
/// the market away while `Any` keeps it.
///
/// Breakage: the lag measured from the row's open (5 minutes more, so the 15-minute case goes
/// stale), the bound strict the wrong way, or the filters reading `best` without the lag.
#[test]
fn a_lagging_cache_is_marked_and_passes_no_event_filter() {
    let (bars, end) = breakout_series();
    let rows = [round_row(0, TfId::H1, EventMarks::default())];
    let fresh = scan(&rows, &bars, end + 15.0 * MIN_MS);
    assert_eq!(fresh.cache_lag_ms, None);
    assert!(matches(&fresh, &ScanFilter::Breakout));
    let late = scan(&rows, &bars, end + 3.0 * HOUR_MS);
    assert_eq!(late.cache_lag_ms, Some(10_800_000));
    assert_eq!(late.answer().cache_lag_ms, late.cache_lag_ms);
    assert_eq!(late.best.map(|b| b.event), Some(AutoEvent::Breakout));
    for filter in [
        ScanFilter::Breakout,
        ScanFilter::Squeeze,
        ScanFilter::FalseBreak,
        ScanFilter::Approach { max_pct: 100.0 },
    ] {
        assert!(!matches(&late, &filter), "{filter:?}");
    }
    assert!(matches(&late, &ScanFilter::Any));
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

/// From a set row: `Auto` is M15, `Fixed` is kept, both capped by the detector's coarsest
/// timeframe; hidden rows and M1 rows are skipped, and the set index is carried.
///
/// Breakage: `Auto` resolved to something else, the cap missed (a `Day` extreme on D1 never sees a
/// closed day), or `from_rows` renumbering past a skipped row.
#[test]
fn rows_resolve_their_timeframe() {
    let swing = AutoFigureRow::new(DetectorParams::Swing(SwingParams::default()));
    assert_eq!(swing.tf, TfSource::Auto);
    assert_eq!(ScanRow::from_row(0, &swing).map(|r| r.tf), Some(TfId::M15));

    let profile = AutoFigureRow::new(DetectorParams::VolumeProfile(VolumeProfileParams::default()));
    assert_eq!(
        ScanRow::from_row(0, &profile).map(|r| r.tf),
        Some(TfId::M15)
    );

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
/// pinned to M15, finds levels inside the candles' range, and a buffer that scanned another market
/// first gives the same answer as a fresh one. The shipped set runs on it without a panic.
///
/// Breakage: `Auto` not resolved to M15, scratch leaking from one market into the next, or the
/// cache's real holes and partial buckets breaking the bar assembly.
#[test]
fn bench_scan_is_stable_and_auto_is_m15() {
    let bars = ace(5);
    let first = bars[0].t_open_ms;
    let last = bars[bars.len() - 1].t_open_ms;
    let now = last + 5.0 * MIN_MS;
    // Half the quarter-hours the bench covers: comfortably inside its history.
    let lookback = ((last - first) / (15.0 * MIN_MS) / 2.0) as u32;
    assert!(
        lookback >= 20,
        "the bench covers {lookback} × 2 quarter-hours"
    );
    let auto = AutoFigureRow::new(DetectorParams::Swing(SwingParams {
        lookback_bars: lookback,
        ..SwingParams::default()
    }));
    let fixed = AutoFigureRow {
        tf: TfSource::Fixed(TfId::M15),
        ..auto.clone()
    };
    let auto_rows = ScanRow::from_rows(&[auto]);
    let fixed_rows = ScanRow::from_rows(&[fixed]);

    let fresh = scan(&auto_rows, &bars, now);
    assert_eq!(answer(&fresh), answer(&scan(&fixed_rows, &bars, now)));
    assert_eq!((fresh.rows_scanned, fresh.short_history), (1, false));
    assert!(fresh.lines > 0, "the bench holds M15 levels");
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
        &[],
        other_now,
        None,
        &mut reused,
    );
    scan_market(&auto_rows, &bars, &[], now, None, &mut reused);
    assert_eq!(answer(&reused), answer(&fresh));
    scan_market(&auto_rows, &bars, &[], now, None, &mut reused);
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

/// 5-minute bars along the straight path through `vertices` (`(bar index, price)`, ascending, the
/// first at index 0), each opening and closing on the path and spanning ±0.05 % of it — the
/// formation tests' synthetic shape, which the scan reads bar for bar on `M5`.
fn path_5m(vertices: &[(usize, f64)]) -> Vec<ChartCandle> {
    let last = vertices.last().map_or(0, |v| v.0);
    (0..=last)
        .map(|i| {
            let k = vertices.iter().rposition(|v| v.0 <= i).unwrap_or(0);
            let (i0, p0) = vertices[k];
            let p = match vertices.get(k + 1) {
                Some(&(i1, p1)) => p0 + (p1 - p0) * (i - i0) as f64 / (i1 - i0) as f64,
                None => p0,
            };
            let (hi, lo) = (p * 1.0005, p * 0.9995);
            bar(i, p as f32, hi as f32, lo as f32, p as f32)
        })
        .collect()
}

/// A formation row on `M5` over the newest 27 bars, with only the kinds `pick` leaves on.
fn formation_row(row_ix: usize, pick: impl Fn(&mut FormationParams)) -> ScanRow {
    let mut p = FormationParams {
        triangle: false,
        wedge: false,
        channel: false,
        flag: false,
        double: false,
        lookback_bars: 27,
        ..FormationParams::default()
    };
    pick(&mut p);
    ScanRow {
        row_ix,
        params: DetectorParams::Formation(p),
        tf: TfId::M5,
        take: Take::default(),
        marks: EventMarks::default(),
    }
}

/// One formation kept by a "Channels" row and a "Flags" row is ONE line of the market, the flag's:
/// the chart draws it once under the more precise name, and the screener counts and places what
/// the chart draws.
///
/// By hand: the bull flag of the formation tests — a 12 % rise 100 → 112 over bars 0..8, then a
/// channel falling 1/6 per bar (tops 112 / 111 / 110, bottoms 108 / 107 / 106) — is a flag to the
/// flag row and the channel it is to the channel row, the same pair of sides. Each row alone keeps
/// one line; together they keep one, the flag row's, although the channel row comes first and a
/// tie on distance goes to the lower row index.
///
/// Breakage: the scan counting both rows' readings (two lines, the nearest one the channel row's),
/// so the screener disagrees with the chart about what is drawn.
#[test]
fn a_formation_two_rows_find_is_one_line() {
    let bars = path_5m(&[
        (0, 100.0),
        (8, 112.0),
        (11, 108.0),
        (14, 111.0),
        (17, 107.0),
        (20, 110.0),
        (23, 106.0),
        (26, 107.5),
    ]);
    let now = MONDAY_2024_01_01_MS + 27.0 * 5.0 * MIN_MS;
    let channels = formation_row(0, |p| p.channel = true);
    let flags = formation_row(1, |p| p.flag = true);
    for row in [channels, flags] {
        let alone = scan(&[row], &bars, now);
        assert_eq!((alone.rows_scanned, alone.lines), (1, 1), "{alone:?}");
    }
    let both = scan(&[channels, flags], &bars, now);
    assert_eq!(both.lines, 1, "{both:?}");
    assert_eq!(both.nearest.map(|l| l.row_ix), Some(1), "{both:?}");
}

/// A formation the more precisely named row does NOT keep is drawn by the row that does, under its
/// own name: the "Flags" row taking only lines below price, or none at all, leaves the flag to the
/// "Channels" row, and the market still has its one line.
///
/// Breakage: twins judged over the FOUND lines — the flag row's unkept reading takes the formation
/// away from the channel row, and the market loses the line altogether.
#[test]
fn a_twin_the_flag_row_does_not_keep_stays_with_the_channel_row() {
    let bars = path_5m(&[
        (0, 100.0),
        (8, 112.0),
        (11, 108.0),
        (14, 111.0),
        (17, 107.0),
        (20, 110.0),
        (23, 106.0),
        (26, 107.5),
    ]);
    let now = MONDAY_2024_01_01_MS + 27.0 * 5.0 * MIN_MS;
    let channels = formation_row(0, |p| p.channel = true);
    let mut below_only = formation_row(1, |p| p.flag = true);
    below_only.take.side = TakeSide::Below;
    let mut none = formation_row(1, |p| p.flag = true);
    none.take.count = 0;
    for flags in [below_only, none] {
        let alone = scan(&[flags], &bars, now);
        assert_eq!((alone.rows_scanned, alone.lines), (1, 0), "{alone:?}");
        let both = scan(&[channels, flags], &bars, now);
        assert_eq!(both.lines, 1, "{both:?}");
        assert_eq!(both.nearest.map(|l| l.row_ix), Some(0), "{both:?}");
    }
}

/// Twelve 5-minute rows per hour with the ring's placeholder open and close — `open == high`,
/// `close == low` — each hour spanning `(high, low)`.
fn hourly_ring(ranges: &[(f32, f32)]) -> Vec<ChartCandle> {
    ranges
        .iter()
        .enumerate()
        .flat_map(|(h, &(hi, lo))| (0..12).map(move |j| bar(h * 12 + j, hi, hi, lo, lo)))
        .collect()
}

/// A market the cache knows only from the core's ring breaks a level only with a bar that lies
/// wholly beyond it.
///
/// - Three hours spanning 3.51–3.53, above the 3.5 level, then one spanning 3.47–3.56: read as
///   closes at the lows, the last hour would "close" at 3.47 under the level — a breakout made of
///   the ring's placeholders. As a ring bar it straddles 3.5 and has no side: no event. The same
///   rows read as REAL candles do break it: the mark comes from the kind the rows were read from,
///   not from their shape.
/// - Three hours spanning 3.38–3.45, then one spanning 3.52–3.58: the last hour lies wholly above
///   3.5, so wherever it closed, it closed above — the breakout real candles of those ranges show.
/// - Real candles for the last two hours of the first ring (closing 3.52, then 3.45) win their
///   buckets, and the breakout down is theirs.
///
/// By hand: the round level is placed from the last price — the middle of the last ring row's
/// range, 3.515 and 3.55, or the real close of 3.45 — and 3.5 is the only half-order level within
/// 10 % of each. The straddling case is (3.5 − 3.515) / 3.515 = −0.427 % away, past the 0.3 %
/// approach. Distances: (3.5 − 3.55) / 3.55 = −1.408 %, (3.5 − 3.45) / 3.45 = +1.449 %.
///
/// Breakage: the ring's placeholder lows taken as closes (a breakout made of ring bars), a ring bar
/// that never breaks anything (the level hangs unbroken over a market that left it), the mark taken
/// from the shape, a ring row shadowing a real candle, or the ring dropped (no level, no history).
#[test]
fn a_ring_breaks_a_level_only_with_a_bar_wholly_beyond_it() {
    let rows = [round_row(0, TfId::H1, EventMarks::default())];
    let now = MONDAY_2024_01_01_MS + 4.0 * HOUR_MS;
    let straddling = hourly_ring(&[(3.53, 3.51), (3.53, 3.51), (3.53, 3.51), (3.56, 3.47)]);
    let r = scan_with_ring(&rows, &[], &straddling, now);
    assert_eq!((r.rows_scanned, r.lines), (1, 1), "{r:?}");
    assert_eq!(r.best, None, "{r:?}");
    let as_real = scan(&rows, &straddling, now);
    assert_eq!(
        as_real.best.map(|b| b.event),
        Some(AutoEvent::Breakout),
        "{as_real:?}"
    );

    let beyond = hourly_ring(&[(3.45, 3.38), (3.45, 3.38), (3.45, 3.38), (3.58, 3.52)]);
    let r = scan_with_ring(&rows, &[], &beyond, now);
    assert_eq!(r.last_price, Some(f64::from((3.58f32 + 3.52f32) / 2.0)));
    assert_eq!(r.best.map(|b| b.event), Some(AutoEvent::Breakout), "{r:?}");
    let d = r.nearest.expect("the 3.5 line").distance_pct;
    assert!((d + 1.408_45).abs() < 1e-3, "{d}");

    let real: Vec<ChartCandle> = hourly(&[3.52, 3.52, 3.52, 3.45])
        .into_iter()
        .skip(24)
        .collect();
    let r = scan_with_ring(&rows, &real, &straddling, now);
    assert_eq!(r.best.map(|b| b.event), Some(AutoEvent::Breakout), "{r:?}");
    let d = r.nearest.expect("the 3.5 line").distance_pct;
    assert!((d - 1.449_28).abs() < 1e-3, "{d}");
}

/// The market's live price, when the caller has one, is what sides and distances are measured
/// from: the 3.5 line under a live 3.6 is (3.5 - 3.6) / 3.6 = -2.778 % away; a live price that is
/// not a positive number falls back to the cache's.
///
/// Breakage: the live price ignored, or a zero live price used.
#[test]
fn a_live_price_is_the_reference() {
    let rows = [round_row(0, TfId::H1, EventMarks::default())];
    let (bars, now) = breakout_series();
    let mut out = ScanResult::default();
    scan_market(&rows, &bars, &[], now, Some(3.6), &mut out);
    assert_eq!(out.last_price, Some(3.6));
    let d = out.nearest.expect("the 3.5 line").distance_pct;
    assert!((d + 2.777_78).abs() < 1e-4, "{d}");
    scan_market(&rows, &bars, &[], now, Some(0.0), &mut out);
    assert_eq!(out.last_price, Some(f64::from(3.55f32)));
}
