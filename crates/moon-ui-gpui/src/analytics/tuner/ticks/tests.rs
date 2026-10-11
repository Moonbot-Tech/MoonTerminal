use std::time::Duration;

use super::fetch_wait_text;

/// A count ending in one must use the singular instrumental form, including twenty-one.
#[test]
fn usable_trade_caption_declines_russian_and_ukrainian_counts() {
    for (code, expected) in [
        (
            "ru",
            [
                "по 1 годной сделке",
                "по 2 годным сделкам",
                "по 5 годным сделкам",
                "по 21 годной сделке",
            ],
        ),
        (
            "uk",
            [
                "за 1 придатною угодою",
                "за 2 придатними угодами",
                "за 5 придатними угодами",
                "за 21 придатною угодою",
            ],
        ),
    ] {
        let _locale = crate::test_locale::force(code);
        for (n, expected) in [1, 2, 5, 21].into_iter().zip(expected) {
            assert_eq!(super::kpi_over_text(n), expected);
        }
    }
}

/// The wait before the fill is shown only where the core filed a placement that precedes the
/// entry fill: an absent, zero or later placement gives no line.
#[test]
fn order_wait_needs_a_placement_before_the_fill() {
    use super::order_wait_ms;
    assert_eq!(order_wait_ms(10_000, Some(7_600)), Some(2_400));
    assert_eq!(order_wait_ms(10_000, None), None);
    assert_eq!(order_wait_ms(10_000, Some(0)), None);
    assert_eq!(order_wait_ms(10_000, Some(-5)), None);
    assert_eq!(order_wait_ms(10_000, Some(12_000)), None);
    assert_eq!(order_wait_ms(10_000, Some(10_000)), None);
}

/// Printing a 90s pause as minutes would hide the seconds the next try honours, and printing
/// a 10 minute pause as raw seconds would read as a stuck counter. The one-second floor of
/// the search estimate must not turn a deadline that has just passed into "1 s".
#[test]
fn a_venue_wait_stays_in_seconds_through_ninety_then_reads_as_minutes() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(fetch_wait_text(Duration::from_secs(0)), "0 s");
    assert_eq!(fetch_wait_text(Duration::from_secs(30)), "30 s");
    assert_eq!(fetch_wait_text(Duration::from_secs(90)), "90 s");
    assert_eq!(fetch_wait_text(Duration::from_secs(91)), "2 min");
    assert_eq!(fetch_wait_text(Duration::from_secs(600)), "10 min");
    assert_eq!(
        fetch_wait_text(Duration::from_secs(3 * 3600 + 5 * 60)),
        "3 h 5 min"
    );
    assert_eq!(fetch_wait_text(Duration::from_millis(500)), "1 s");
}
