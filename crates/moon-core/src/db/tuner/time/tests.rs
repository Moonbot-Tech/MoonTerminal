use super::*;

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

    // Unpinned: every day is in the black, so no week window improves on the whole week.
    let free = time_suggest_from_rows(&rows, 1, 64, false, axes_of(true, false, false));
    assert_eq!(
        free.week_span, None,
        "over the full sample no week window beats the baseline"
    );

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
                .map_or(true, |(f, t)| span_ok(wd * 1440 + mn, f as i64, t as i64))
                && s.tod.map_or(true, |tw| match tw {
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
