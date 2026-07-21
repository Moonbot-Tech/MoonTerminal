use super::*;

#[test]
fn variant_where_whitelists_fields() {
    let v = Variant {
        bounds: vec![
            Bound {
                field: "d1h".into(),
                from: Some(1.5),
                to: Some(10.0),
            },
            Bound {
                field: "evil\"; DROP TABLE x;--".into(),
                from: Some(1.0),
                to: None,
            },
            Bound {
                field: "hvol".into(),
                from: None,
                to: None,
            },
        ],
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("COALESCE(o.\"d1h\",0) >= 1.5"));
    assert!(w.contains("<= 10"));
    assert!(!w.contains("DROP"));
    assert!(!w.contains("hvol"), "пустые границы не добавляют условий");
}

/// The variant's only STRING condition: a coin name reaches SQL as a literal,
/// so its apostrophe must be doubled — otherwise one such coin breaks the whole
/// WHERE and "Fact vs v1" quietly scores a different set of trades.
#[test]
fn variant_coins_quote_is_escaped() {
    let v = Variant {
        coins_in: Some(vec!["BTC".into(), "O'BRIEN' OR 1=1--".into()]),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("IN ('BTC','O''BRIEN'' OR 1=1--')"), "w={w}");
    assert!(!v.is_empty(), "a coin variant is not the fact");
    // No dangling quote: their count in the condition stays even.
    assert_eq!(w.matches('\'').count() % 2, 0, "unbalanced quote: {w}");
    assert!(
        Variant::default().where_sql().is_empty(),
        "an empty coin list adds no condition"
    );
    // The blacklist side EXCLUDES, and both sides may apply at once.
    let both = Variant {
        coins_in: Some(vec!["BTC".into()]),
        coins_out: vec!["ETH".into()],
        ..Default::default()
    };
    let w = both.where_sql();
    assert!(w.contains("IN ('BTC')"), "w={w}");
    assert!(w.contains("NOT IN ('ETH')"), "w={w}");

    // A whitelist that no traded coin satisfies keeps NOTHING — it must not quietly
    // degrade into "no whitelist", which would score the untouched fact as the plan.
    let unmatched = Variant {
        coins_in: Some(Vec::new()),
        ..Default::default()
    };
    assert!(unmatched.where_sql().contains("0=1"));
    assert!(
        !unmatched.is_empty(),
        "an unmatched whitelist is not the fact"
    );
}

#[test]
fn variant_week_span_predicate() {
    // Пн 00:00 → Сб 23:59 (мин недели 0..8639): непрерывный → BETWEEN, исключает Вс.
    let v = Variant {
        week_span: Some((0, 8639)),
        ..Default::default()
    };
    let w = v.where_sql();
    // По времени ОТКРЫТИЯ (buydate), не closedate.
    assert!(
        w.contains("BETWEEN 0 AND 8639") && w.contains("buydate"),
        "w={w}"
    );
    assert!(!v.is_empty(), "week_span-вариант не равен «Факту»");

    // Через воскресенье→понедельник (от > до): Сб 12:00 (8640-720=7920) → Пн 12:00 (720).
    let v = Variant {
        week_span: Some((7920, 720)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(
        w.contains("<= 720 OR"),
        "через воскресенье — до Пн 12:00: {w}"
    );
    assert!(
        w.contains(">= 7920"),
        "через воскресенье — от Сб 12:00: {w}"
    );
    assert!(!w.contains("BETWEEN"), "перевёрнутое окно не BETWEEN: {w}");
}

#[test]
fn variant_time_window_predicate() {
    // WorkingTime «Day» 09:00–21:00 → минута суток BETWEEN.
    let v = Variant {
        tod: Some(TimeWindow::Day(9 * 60, 21 * 60)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(
        w.contains("BETWEEN 540 AND 1260") && w.contains("buydate"),
        "w={w}"
    );
    assert!(!v.is_empty());

    // «Day» через полночь (22:00–06:00) → «<= 360 OR >= 1320».
    let v = Variant {
        tod: Some(TimeWindow::Day(22 * 60, 6 * 60)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("<= 360 OR"), "через полночь до 06:00: {w}");
    assert!(w.contains(">= 1320"), "через полночь от 22:00: {w}");

    // WorkingTime «Hour» 1–50 → минута В ЧАСЕ (mod 60) BETWEEN.
    let v = Variant {
        tod: Some(TimeWindow::Hour(1, 50)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("% 60) BETWEEN 1 AND 50"), "w={w}");

    // week_span И tod складываются в один WHERE (обе оси).
    let v = Variant {
        week_span: Some((0, 8639)),
        tod: Some(TimeWindow::Day(1, 1430)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("BETWEEN 0 AND 8639"));
    assert!(w.contains("BETWEEN 1 AND 1430"));
}

#[test]
fn working_time_format() {
    // Минута недели: Пн 00:00 (0) → Сб 23:59 (8639) — края суток пишем коротко → «1-6».
    assert_eq!(format_week_span((0, 8639)), "1-6");
    // С временем: Пн 23:44 (1424) → Сб 22:22 (5*1440+1342=8542) → «1.23:44-6.22:22».
    assert_eq!(format_week_span((1424, 8542)), "1.23:44-6.22:22");
    // WorkingTime Day → «чч:мм-чч:мм»; Hour → «N-M».
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
    // Оси недели и времени по отдельности «улучшают», но их пересечение могло бы
    // выкинуть разные плюсовые сделки → профит НИЖЕ базового. Проверяем, что
    // подбор этого не допускает (база — кандидат).
    let mut rows: Vec<(i64, i64, f64)> = Vec::new();
    for wd in 0..6i64 {
        for mn in (0..1440).step_by(20) {
            rows.push((wd, mn, if mn / 60 == 3 { -2.0 } else { 1.0 })); // час 3 в минусе
        }
    }
    for mn in (0..1440).step_by(20) {
        rows.push((6, mn, -1.0)); // Вс в минусе
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

#[test]
fn slider_profiles_bucketize() {
    // Пн (wd0) 00:00 +2; Вт (wd1) 05:30 -4 → раскладка по трём осям.
    let rows = vec![(0i64, 0i64, 2.0), (1i64, 5 * 60 + 30, -4.0)];
    let p = slider_profiles_from_rows(&rows);
    assert_eq!(p.week[0], 2.0, "неделя: Пн·00ч (wd0*24+0)");
    assert_eq!(p.week[24 + 5], -4.0, "неделя: Вт·05ч (wd1*24+5)");
    assert_eq!(p.day[0], 2.0, "сутки: минута 0");
    assert_eq!(p.day[5 * 60 + 30], -4.0, "сутки: минута 330 (05:30)");
    assert_eq!(p.hour[0], 2.0);
    assert_eq!(p.hour[30], -4.0);
}

#[test]
fn best_range_skips_noop_full_range() {
    // Все сделки прибыльные: никакой поддиапазон не лучше «без фильтра» —
    // пару min/max-пустышку предлагать нельзя.
    let mut vals: Vec<(f64, f64)> = (0..100).map(|i| (i as f64, 1.0)).collect();
    assert!(best_range(&mut vals, 1, 16, false).is_none());
    // Нижняя треть в минусе: диапазон предлагается и он НЕ весь размах.
    let mut vals: Vec<(f64, f64)> = (0..99)
        .map(|i| (i as f64, if i < 33 { -1.0 } else { 1.0 }))
        .collect();
    let s = best_range(&mut vals, 1, 16, false).expect("фильтр должен найтись");
    assert!(
        s.from.unwrap() > 0.0,
        "нижняя граница должна отрезать минус"
    );
    assert_eq!(s.to.unwrap(), 98.0, "верхняя пара — фактический max данных");
}

#[test]
fn round_falls_back_when_pair_stops_cutting() {
    // Диапазон режет только два хвостовых минуса у самых краёв данных:
    // округление наружу увело бы ОБЕ границы за min/max (пустышка) —
    // должны остаться сырые режущие значения.
    let mut vals: Vec<(f64, f64)> = (0..100)
        .map(|i| (1000.0 + i as f64, if i < 2 { -5.0 } else { 1.0 }))
        .collect();
    let s = best_range(&mut vals, 1, 50, true).expect("фильтр должен найтись");
    let from = s.from.unwrap();
    assert!(from > 1000.0, "граница обязана резать данные, got {from}");
}

#[test]
fn empty_variant_is_fact() {
    assert!(Variant::default().is_empty());
    let v = Variant {
        bounds: vec![Bound {
            field: "d1h".into(),
            from: Some(0.0),
            to: None,
        }],
        ..Default::default()
    };
    assert!(!v.is_empty());
}
