use super::*;

/// Minutes since midnight for readable fixtures.
fn hm(h: u16, m: u16) -> u16 {
    h * 60 + m
}

#[test]
fn window_wraps_midnight() {
    let (from, to) = (hm(23, 0), hm(7, 0));
    assert!(in_window(hm(23, 0), from, to), "start is inclusive");
    assert!(in_window(hm(3, 30), from, to));
    assert!(!in_window(hm(7, 0), from, to), "end is exclusive");
    assert!(!in_window(hm(12, 0), from, to));
}

#[test]
fn window_within_one_day() {
    let (from, to) = (hm(9, 0), hm(18, 0));
    assert!(in_window(hm(9, 0), from, to));
    assert!(!in_window(hm(8, 59), from, to));
    assert!(!in_window(hm(18, 0), from, to));
}

#[test]
fn equal_bounds_are_an_empty_window() {
    // A cleared config leaves both fields equal; reading that as a whole day would silence the
    // terminal permanently with nothing on screen explaining why.
    assert!(!in_window(hm(4, 0), hm(0, 0), hm(0, 0)));
}

#[test]
fn next_occurrence_is_always_in_the_future() {
    assert_eq!(minutes_until(hm(6, 59), hm(7, 0)), 1);
    assert_eq!(minutes_until(hm(23, 50), hm(0, 10)), 20, "wraps midnight");
    assert_eq!(
        minutes_until(hm(7, 0), hm(7, 0)),
        DAY_MINUTES,
        "switching on AT the end time means the next one, not zero"
    );
}

/// A readable instant: `MIDNIGHT_MS` plus the given minute of day.
fn at(minute: u16) -> i64 {
    MIDNIGHT_MS + i64::from(minute) * 60_000
}

/// Any fixed Unix millisecond value on a midnight boundary; the tests only compare instants.
const MIDNIGHT_MS: i64 = 1_754_784_000_000;

#[test]
fn manual_sleep_expires_at_its_deadline() {
    let mut cfg = QuietCfg {
        manual_on: true,
        manual_until_ms: Some(at(hm(7, 0))),
        schedule_on: true,
        from_min: hm(23, 0),
        to_min: hm(7, 0),
        ..QuietCfg::default()
    };
    assert!(
        !cfg.tick(at(hm(3, 0)), hm(3, 0)),
        "mid-window changes nothing"
    );
    assert!(cfg.manual_on);
    assert!(cfg.tick(at(hm(7, 0)), hm(7, 0)));
    assert!(!cfg.manual_on, "the deadline ends a manual sleep");
    assert_eq!(cfg.manual_until_ms, None);
}

#[test]
fn manual_sleep_expires_even_if_the_terminal_was_closed_for_it() {
    // The regression this deadline exists for: a wall-clock EDGE can only be seen by a process
    // that was running when the minute passed, so a sleep switched on overnight used to survive
    // into the next afternoon and silence everything.
    let mut cfg = QuietCfg {
        manual_on: true,
        manual_until_ms: Some(at(hm(7, 0))),
        schedule_on: true,
        from_min: hm(23, 0),
        to_min: hm(7, 0),
        ..QuietCfg::default()
    };
    assert!(cfg.tick(at(hm(15, 0)), hm(15, 0)), "startup at 15:00");
    assert!(!cfg.manual_on);
    assert!(!cfg.sleeping_at(hm(15, 0)));
}

#[test]
fn manual_sleep_without_a_schedule_never_expires_by_itself() {
    let mut cfg = QuietCfg {
        manual_on: true,
        schedule_on: false,
        ..QuietCfg::default()
    };
    cfg.toggle_at(hm(12, 0), Some(at(hm(19, 0))));
    assert!(!cfg.manual_on, "that click switched it off");
    cfg.toggle_at(hm(12, 0), Some(at(hm(19, 0))));
    assert!(cfg.manual_on);
    assert_eq!(
        cfg.manual_until_ms, None,
        "no schedule, so no end time to honour"
    );
    assert!(!cfg.tick(at(hm(23, 0)), hm(23, 0)));
    assert!(cfg.manual_on);
}

#[test]
fn switching_on_by_hand_takes_the_schedules_end_time() {
    let mut cfg = QuietCfg {
        schedule_on: true,
        from_min: hm(23, 0),
        to_min: hm(7, 0),
        ..QuietCfg::default()
    };
    cfg.toggle_at(hm(21, 30), Some(at(hm(7, 0)) + 86_400_000));
    assert!(cfg.manual_on);
    assert_eq!(cfg.manual_until_ms, Some(at(hm(7, 0)) + 86_400_000));
}

#[test]
fn manual_wake_lasts_one_window_only() {
    let mut cfg = QuietCfg {
        schedule_on: true,
        from_min: hm(23, 0),
        to_min: hm(7, 0),
        ..QuietCfg::default()
    };
    assert!(cfg.sleeping_at(hm(2, 0)), "the schedule sleeps");
    cfg.toggle_at(hm(2, 0), Some(at(hm(7, 0))));
    assert!(!cfg.sleeping_at(hm(2, 0)), "woken by hand");
    assert!(!cfg.tick(at(hm(2, 1)), hm(2, 1)), "still inside the window");
    assert!(!cfg.sleeping_at(hm(2, 1)));
    // Once the window is over the override is spent, so the next night sleeps unaided.
    assert!(cfg.tick(at(hm(7, 30)), hm(7, 30)));
    assert!(!cfg.wake_override);
    assert!(cfg.sleeping_at(hm(23, 30)), "tomorrow sleeps again");
}

#[test]
fn toggle_sleeps_and_wakes_outside_a_schedule() {
    let mut cfg = QuietCfg::default();
    cfg.toggle_at(hm(12, 0), None);
    assert!(cfg.sleeping_at(hm(12, 0)));
    cfg.toggle_at(hm(12, 0), None);
    assert!(!cfg.sleeping_at(hm(12, 0)));
    assert!(!cfg.wake_override, "no schedule, nothing to override");
}

#[test]
fn bypass_selects_only_the_named_events() {
    let cfg = QuietCfg {
        figure_alerts_bypass: true,
        chart_bypass: vec![3],
        ..QuietCfg::default()
    };
    assert!(cfg.allows_detect_sound(true, 0), "figure alerts pass");
    assert!(cfg.allows_detect_sound(false, 3), "AddToChart 3 passes");
    assert!(!cfg.allows_detect_sound(false, 2));
    assert!(!cfg.allows_detect_sound(false, 0));
    let silent = QuietCfg::default();
    assert!(
        !silent.allows_detect_sound(true, 3),
        "nothing bypasses by default"
    );
}

#[test]
fn dropped_core_wakes_by_default() {
    let b = QuietWarnBypass::default();
    assert!(b.conn);
    assert!(!b.cpu && !b.mem && !b.ping && !b.exch && !b.api);
}

#[test]
fn time_parsing_accepts_what_people_type() {
    assert_eq!(parse_hhmm("23:00"), Some(hm(23, 0)));
    assert_eq!(parse_hhmm("23.05"), Some(hm(23, 5)));
    assert_eq!(parse_hhmm(" 7 30 "), Some(hm(7, 30)));
    assert_eq!(parse_hhmm("2300"), Some(hm(23, 0)));
    assert_eq!(parse_hhmm("730"), Some(hm(7, 30)));
    assert_eq!(parse_hhmm("7"), Some(hm(7, 0)));
    assert_eq!(parse_hhmm("24:00"), None);
    assert_eq!(parse_hhmm("12:60"), None);
    assert_eq!(parse_hhmm("abc"), None);
    assert_eq!(parse_hhmm(""), None);
}

#[test]
fn time_round_trips_through_the_field() {
    for minute in [0u16, 1, 419, 420, 1380, 1439] {
        assert_eq!(parse_hhmm(&fmt_hhmm(minute)), Some(minute));
    }
}

#[test]
fn chart_list_is_canonical() {
    assert_eq!(parse_chart_list("3, 5 5;1"), vec![1, 3, 5]);
    assert_eq!(parse_chart_list("0, 2"), vec![2], "0 means no chart");
    assert!(parse_chart_list("").is_empty());
    assert_eq!(fmt_chart_list(&[1, 3]), "1, 3");
}

#[test]
fn hand_edited_times_stay_inside_the_day() {
    let cfg = QuietCfg {
        from_min: 5000,
        to_min: 1440,
        ..QuietCfg::default()
    };
    assert_eq!(cfg.from_min_norm(), 5000 % DAY_MINUTES);
    assert_eq!(cfg.to_min_norm(), 0);
}
