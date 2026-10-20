use super::*;

/// Measures the same neutral 5k-row cohort before and after the search changes.
#[test]
#[ignore = "manual performance measurement"]
fn suggest_5k_timing() {
    let rows: Vec<_> = (0..5000i64)
        .map(|i| ((i * 17 % 7), i * 137 % 1440, (i * 31 % 23) as f64 - 10.0))
        .collect();
    let start = std::time::Instant::now();
    let result = time_suggest_from_rows(&rows, 5, 512, false, axes_of(true, true, true));
    eprintln!(
        "5k suggestion: {:.3} ms, result={result:?}",
        start.elapsed().as_secs_f64() * 1000.0
    );
}

/// Splitting equal minutes invents an unattainable profit and discards the better real window.
#[test]
fn time_suggestion_keeps_all_trades_at_equal_minutes() {
    let rows = [
        (0, 1, 6.0),
        (0, 18, -10.0),
        (0, 18, 20.0),
        (0, 100, 5.0),
        (0, 200, -30.0),
    ];
    let suggestion = time_suggest_from_rows(&rows, 1, 512, false, axes_of(false, true, false));
    // The realizable optimum is 6 - 10 + 20 + 5 = 21. Starting at minute 18 earns only 15.
    assert_eq!(suggestion.tod, Some(TimeWindow::Day(1, 100)));
}

#[test]
fn working_time_format() {
    // Week minute: Mon 00:00 (0) -> Sat 23:59 (8639); shorten day boundaries -> `1-6`.
    assert_eq!(format_week_span((0, 8639)), "1-6");
    // With times: Mon 23:44 (1424) -> Sat 22:22 (5*1440+1342=8542) -> `1.23:44-6.22:22`.
    assert_eq!(format_week_span((1424, 8542)), "1.23:44-6.22:22");
    // WorkingTime Day -> `hh:mm-hh:mm`; Hour -> `N-M`.
    assert_eq!(
        format_working_time(TimeWindow::Day(1, 23 * 60 + 50)),
        "00:01-23:50"
    );
    assert_eq!(format_working_time(TimeWindow::Hour(1, 50)), "1-50");
}

/// Opened axes with nothing pinned — the plain "search these" case.
fn axes_of(week: bool, day: bool, hour: bool) -> TimeAxes {
    TimeAxes {
        week,
        day,
        hour,
        ..Default::default()
    }
}

/// Minute 0..14 of EVERY hour loses. Separable by the minute-of-hour projection;
/// over the minute of the DAY the losses are scattered across the whole range.
fn rows_bad_first_minutes() -> Vec<(i64, i64, f64)> {
    (0..24)
        .flat_map(|h| (0..60).map(move |m| (0i64, h * 60 + m, if m < 15 { -1.0 } else { 1.0 })))
        .collect()
}

#[test]
fn suggest_time_week_span_and_mode() {
    // Sun (wd6) loses, Mon..Sat win → the best week window cuts Sunday off.
    let mut rows: Vec<(i64, i64, f64)> = Vec::new();
    for wd in 0..6 {
        rows.extend((0..30).map(|i| (wd, i * 40, 1.0)));
    }
    rows.extend((0..30).map(|i| (6i64, i * 40, -1.0))); // Sunday in the red
    let s = time_suggest_from_rows(&rows, 1, 64, false, axes_of(true, false, false));
    let (_, to) = s.week_span.expect("a week window must be found");
    assert!(
        to < 6 * 1440,
        "the window must end before Sunday (minute of week < 8640), got {to}"
    );

    // Every box ticked — the "just sweep everything" case: the two WorkingTime formats
    // compete and the sweep keeps the one that actually separates the loss (Hour).
    let rows = rows_bad_first_minutes();
    let s = time_suggest_from_rows(&rows, 1, 32, false, axes_of(true, true, true));
    match s.tod {
        Some(TimeWindow::Hour(f, _t)) => {
            assert!(f > 0, "the Hour window must cut minutes 0..14, from={f}")
        }
        other => panic!("with both formats offered the better (Hour) must win, got {other:?}"),
    }
}

#[test]
fn suggest_time_respects_axes() {
    let rows = rows_bad_first_minutes();

    // No row checked → no sweep at all (and no reason to read the DB).
    let none = time_suggest_from_rows(&rows, 1, 32, false, axes_of(false, false, false));
    assert_eq!(none, TimeSuggest::default(), "no checkbox, no sweep");

    // The gate that matters: on the SAME data "In hour" checked yields an Hour window,
    // while with only "Day" checked that window may not appear — an unchecked format is
    // never produced, however profitable it would be.
    let hour = time_suggest_from_rows(&rows, 1, 32, false, axes_of(false, false, true));
    assert!(
        matches!(hour.tod, Some(TimeWindow::Hour(..))),
        "the checked Hour format must be searched, got {:?}",
        hour.tod
    );
    let day = time_suggest_from_rows(&rows, 1, 32, false, axes_of(false, true, false));
    assert!(
        !matches!(day.tod, Some(TimeWindow::Hour(..))),
        "an unchecked Hour format may not come out of the sweep, got {:?}",
        day.tod
    );

    // "Weekly" unchecked → its field stays untouched even when cutting it clearly pays.
    let mut wk_rows: Vec<(i64, i64, f64)> = Vec::new();
    for wd in 0..6 {
        wk_rows.extend((0..30).map(|i| (wd, i * 40, 1.0)));
    }
    wk_rows.extend((0..30).map(|i| (6i64, i * 40, -1.0)));
    let s = time_suggest_from_rows(&wk_rows, 1, 64, false, axes_of(false, true, true));
    assert_eq!(s.week_span, None, "an unchecked week is not searched");
}

#[test]
fn suggest_time_pins_unchecked_row() {
    // Every day is profitable overall, but inside HOUR 0 Sunday alone loses. This is
    // the reachable UI state: "Weekly" checked while neither WorkingTime row is, so
    // the WorkingTime window already in the grid pins the sweep.
    let rows: Vec<(i64, i64, f64)> = (0..7i64)
        .flat_map(|wd| {
            (0..24i64).map(move |h| {
                let p = match (h, wd) {
                    (0, 6) => -5.0, // Sunday, hour 0 — the only loss
                    (0, _) => 1.0,
                    (_, 6) => 3.0, // Sunday is the best day everywhere else
                    _ => 1.0,
                };
                (wd, h * 60, p)
            })
        })
        .collect();

    // Circular search can now exclude Sunday hour 0 while keeping every other winning hour.
    let free = time_suggest_from_rows(&rows, 1, 64, false, axes_of(true, false, false));
    let (from, to) = free
        .week_span
        .expect("circular search excludes the isolated loss");
    assert!(from > to && from > 6 * 1440 && to < 6 * 1440);

    // Pinned to hour 0 — the sweep must work INSIDE it, where Sunday is the loss.
    let pinned = time_suggest_from_rows(
        &rows,
        1,
        64,
        false,
        TimeAxes {
            week: true,
            day: false,
            hour: false,
            fixed_week: None,
            fixed_tod: Some(TimeWindow::Day(0, 59)),
        },
    );
    let (_, to) = pinned
        .week_span
        .expect("inside the pinned hour, cutting Sunday pays");
    assert!(to < 6 * 1440, "the window must end before Sunday, got {to}");
}

/// Refining the day window inside the selected week beats independently intersected projections.
#[test]
fn joint_week_day_beats_independent() {
    let profits = [-4., -8., 1., -7., 10., 0., -3., 6., -2., 0., -10., 1.];
    let rows: Vec<_> = profits
        .into_iter()
        .enumerate()
        .map(|(i, p)| ((i / 3) as i64, [10, 300, 800][i % 3], p))
        .collect();
    let s = time_suggest_from_rows(&rows, 1, 512, false, axes_of(true, true, false));
    let profit: f64 = rows
        .iter()
        .filter(|&&(wd, mn, _)| accepts(wd, mn, s.week_span, s.tod))
        .map(|r| r.2)
        .sum();
    // Independent week [1740,3180] earns 13; independent day [800,800] earns zero.
    // Refitting day [300,300] within that week retains the two wins, 10 + 6.
    assert_eq!(profit, 16.0);
}

#[test]
fn suggest_time_never_worse_than_base() {
    // The week and time axes each improve profit independently, but their intersection could
    // remove different profitable trades and fall BELOW the baseline. Verify the suggestion
    // prevents that by including the baseline as a candidate.
    let mut rows: Vec<(i64, i64, f64)> = Vec::new();
    for wd in 0..6i64 {
        for mn in (0..1440).step_by(20) {
            rows.push((wd, mn, if mn / 60 == 3 { -2.0 } else { 1.0 })); // Hour 3 loses.
        }
    }
    for mn in (0..1440).step_by(20) {
        rows.push((6, mn, -1.0)); // Sunday loses.
    }
    let base: f64 = rows.iter().map(|r| r.2).sum();
    let s = time_suggest_from_rows(&rows, 1, 64, false, axes_of(true, true, true));
    let span_ok = |v: i64, f: i64, t: i64| {
        if f <= t {
            f <= v && v <= t
        } else {
            v <= t || v >= f
        }
    };
    let got: f64 = rows
        .iter()
        .filter(|&&(wd, mn, _)| {
            s.week_span
                .is_none_or(|(f, t)| span_ok(wd * 1440 + mn, f as i64, t as i64))
                && s.tod.is_none_or(|tw| match tw {
                    TimeWindow::Day(f, t) => span_ok(mn, f as i64, t as i64),
                    TimeWindow::Hour(f, t) => span_ok(mn % 60, f as i64, t as i64),
                })
        })
        .map(|r| r.2)
        .sum();
    assert!(
        got >= base,
        "подбор не должен быть хуже базы: got={got} base={base}"
    );
}

/// `time/mod.rs:SliderProfileAccumulator::push` must update `entry_hours` in the same
/// streaming pass; removing that update restores a second full-period query for the heatmap.
#[test]
fn slider_profiles_bucketize() {
    // Mon (wd0) 00:00 +2; Tue (wd1) 05:30 -4 -> distribution across all three axes.
    let rows = vec![(0i64, 0i64, 2.0), (1i64, 5 * 60 + 30, -4.0)];
    let p = slider_profiles_from_rows(&rows);
    assert_eq!(p.week[0], 2.0, "week: Mon hour 0");
    assert_eq!(p.week[24 + 5], -4.0, "week: Tue hour 5");
    assert_eq!(p.day[0], 2.0, "day: minute 0");
    assert_eq!(p.day[5 * 60 + 30], -4.0, "day: minute 330");
    assert_eq!(p.hour[0], 2.0);
    assert_eq!(p.hour[30], -4.0);
    assert_eq!(p.entry_hours[0].profit, 2.0);
    assert_eq!(p.entry_hours[0].trades, 1);
    assert_eq!(p.entry_hours[0].wins, 1);
    assert_eq!(p.entry_hours[5].profit, -4.0);
    assert_eq!(p.entry_hours[5].trades, 1);
    assert_eq!(p.entry_hours[5].wins, 0);
}
