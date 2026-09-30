use super::*;

const LATE: Duration = Duration::from_secs(600);
const EARLY: Duration = Duration::from_secs(3);

#[test]
fn another_chat_or_period_always_reads() {
    for age in [EARLY, LATE] {
        assert_eq!(reuse(false, age, true, Some(&1), Some(&1)), Reuse::Read);
        assert_eq!(reuse(false, age, false, Some(&1), Some(&1)), Reuse::Read);
    }
}

#[test]
fn within_the_ttl_the_inputs_are_not_compared() {
    assert_eq!(reuse(true, EARLY, true, Some(&1), Some(&2)), Reuse::Serve);
    assert_eq!(reuse::<u8>(true, EARLY, true, None, None), Reuse::Serve);
}

#[test]
fn another_grant_always_drops() {
    for age in [EARLY, LATE] {
        assert_eq!(reuse(true, age, false, Some(&1), Some(&1)), Reuse::Drop);
        assert_eq!(reuse(true, age, false, Some(&1), Some(&2)), Reuse::Drop);
    }
}

#[test]
fn past_the_ttl_only_unchanged_inputs_answer() {
    assert_eq!(reuse(true, LATE, true, Some(&1), Some(&1)), Reuse::Rebuild);
    assert_eq!(reuse(true, LATE, true, Some(&1), Some(&2)), Reuse::Read);
    assert_eq!(
        reuse(true, REPORT_CACHE_TTL, true, Some(&1), Some(&2)),
        Reuse::Read
    );
}

#[test]
fn past_the_ttl_an_unknown_revision_reads_as_before() {
    assert_eq!(reuse(true, LATE, true, None, Some(&1)), Reuse::Read);
    assert_eq!(reuse(true, LATE, true, Some(&1), None), Reuse::Read);
    assert_eq!(reuse::<u8>(true, LATE, true, None, None), Reuse::Read);
}

#[test]
fn a_window_ending_now_keys_on_its_day_not_its_end() {
    let zone = chrono_tz::UTC;
    // 2026-09-30 00:00:00 UTC and two instants later that day.
    let start = 1_790_726_400;
    let early = Window::of(start, start + 3_600, start + 3_600, zone);
    let later = Window::of(start, start + 7_200, start + 7_200, zone);
    assert_eq!(early, later);
    let tomorrow = Window::of(start, start + 90_000, start + 90_000, zone);
    assert_ne!(early, tomorrow);
    // A window whose end is not now keeps both ends.
    let fixed = Window::of(start - 86_400, start - 1, start + 3_600, zone);
    assert_eq!(
        fixed,
        Some(Window::Fixed {
            from: start - 86_400,
            to: start - 1
        })
    );
    assert_ne!(
        fixed,
        Window::of(start - 86_400, start - 2, start + 3_600, zone)
    );
}

#[test]
fn a_window_ending_now_is_filed_only_without_rows_past_its_end() {
    let zone = chrono_tz::UTC;
    let start = 1_790_726_400;
    let to_now = Window::of(start, start + 60, start + 60, zone).unwrap();
    assert!(to_now.ends_now());
    assert!(to_now.holds(Some(false)));
    assert!(!to_now.holds(Some(true)));
    assert!(!to_now.holds(None), "an unanswered check files nothing");
    let fixed = Window::of(start - 86_400, start - 1, start + 60, zone).unwrap();
    assert!(!fixed.ends_now());
    assert!(fixed.holds(Some(true)));
    assert!(fixed.holds(None));
}

#[test]
fn a_moved_revision_window_zone_or_language_is_a_change() {
    let day = Window::Fixed {
        from: 0,
        to: 86_399,
    };
    let inputs = |revision, window, zone, locale: &str| ReportInputs {
        revision,
        window,
        zone,
        locale: locale.to_string(),
        names: CoreNames::default(),
        venues: HashMap::new(),
        order: CoreOrder::new(&moon_core::config::AppConfig::headless(Vec::new())),
    };
    let rev = |a, b, c, d| ReportRevision::from_parts(a, b, c, d);
    let base = inputs(rev(1, 7, 3, 5), day, chrono_tz::UTC, "en");
    assert_eq!(base, inputs(rev(1, 7, 3, 5), day, chrono_tz::UTC, "en"));
    assert_ne!(base, inputs(rev(2, 7, 3, 5), day, chrono_tz::UTC, "en"));
    assert_ne!(base, inputs(rev(1, 8, 3, 5), day, chrono_tz::UTC, "en"));
    assert_ne!(base, inputs(rev(1, 7, 4, 5), day, chrono_tz::UTC, "en"));
    assert_ne!(base, inputs(rev(1, 7, 3, 6), day, chrono_tz::UTC, "en"));
    let next = Window::Fixed {
        from: 86_400,
        to: 172_799,
    };
    assert_ne!(base, inputs(rev(1, 7, 3, 5), next, chrono_tz::UTC, "en"));
    assert_ne!(
        base,
        inputs(rev(1, 7, 3, 5), day, chrono_tz::Europe::Moscow, "en")
    );
    assert_ne!(base, inputs(rev(1, 7, 3, 5), day, chrono_tz::UTC, "ru"));
}
