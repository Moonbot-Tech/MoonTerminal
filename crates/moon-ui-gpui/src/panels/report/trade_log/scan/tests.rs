use moon_core::applog::LogLine;

use super::{log_dates, only_in_live, trade_log_request, without_core_clock};

/// A stored core line with the given timestamp and text.
fn line(ts: &str, msg: &str) -> LogLine {
    LogLine {
        ts: ts.to_string(),
        level: log::Level::Info,
        target: String::new(),
        msg: msg.to_string(),
    }
}

/// The texts of the lines that survived, in order.
fn texts(lines: &[LogLine]) -> Vec<&str> {
    lines.iter().map(|line| line.msg.as_str()).collect()
}

/// The hours both copies hold must read once, and the hours only the live copy holds — after the
/// day was pulled — must not be lost.
#[test]
fn a_line_both_copies_hold_is_dropped_from_the_live_one() {
    let pulled = [
        line("2026-08-03 10:00:00.000", "ACE (4411) buy placed"),
        line("2026-08-03 10:00:05.000", "ACE (4411) buy filled"),
    ];
    // The live copy as the stream delivers it: the core's own clock is part of the message, and
    // the line's time is when the stream sent it.
    let live = vec![
        line(
            "2026-08-03 10:00:00.412",
            "10:00:00.000  ACE (4411) buy placed",
        ),
        line("2026-08-03 10:00:05.090", " ACE (4411) buy filled "),
        line(
            "2026-08-03 18:30:00.700",
            "18:30:00.250  ACE (4411) sell filled",
        ),
    ];

    assert_eq!(
        texts(&only_in_live(live, &pulled)),
        ["18:30:00.250  ACE (4411) sell filled"]
    );
}

/// The stream runs behind the core by up to ten seconds on a busy day, and no copy is a minute
/// late: equal text a minute apart is a second event, and equal text a day apart is another day's.
#[test]
fn copies_match_within_the_stream_lag_and_no_further() {
    let pulled = [line("2026-08-03 10:00:00.000", "ACE (4411) order moved")];
    for near in ["2026-08-03 10:00:10.900", "2026-08-03 09:59:59.998"] {
        let live = vec![line(near, "ACE (4411) order moved")];
        assert!(only_in_live(live, &pulled).is_empty(), "{near}");
    }

    let later = vec![
        line("2026-08-03 10:01:00.000", "ACE (4411) order moved"),
        line("2026-08-04 10:00:00.000", "ACE (4411) order moved"),
    ];
    assert_eq!(only_in_live(later, &pulled).len(), 2);
}

/// Pairing a live line with the FIRST pulled copy in range, instead of the nearest, would leave a
/// later live line without its own twin and show it twice.
#[test]
fn a_live_line_pairs_with_its_nearest_copy() {
    let pulled = [
        line("2026-08-03 10:00:00.000", "ACE (4411) order moved"),
        line("2026-08-03 10:00:12.000", "ACE (4411) order moved"),
        line("2026-08-03 10:00:30.000", "ACE (4411) order moved"),
    ];
    let live = vec![
        line("2026-08-03 10:00:12.300", "ACE (4411) order moved"),
        line("2026-08-03 10:00:30.300", "ACE (4411) order moved"),
        line("2026-08-03 10:00:00.300", "ACE (4411) order moved"),
    ];
    assert!(only_in_live(live, &pulled).is_empty());
}

/// Only a clock is cut from the front of a message: a number that is not a time is the message.
#[test]
fn only_a_leading_clock_is_ignored_in_the_comparison() {
    assert_eq!(without_core_clock("10:00:00.250  ACE buy"), "ACE buy");
    assert_eq!(without_core_clock(" 10:00:00   ACE buy "), "ACE buy");
    assert_eq!(without_core_clock("10:00:00"), "");
    assert_eq!(without_core_clock("10:00:00x ACE"), "10:00:00x ACE");
    assert_eq!(without_core_clock("10:00:00.25 ACE"), "10:00:00.25 ACE");
    assert_eq!(without_core_clock("100 lots ACE"), "100 lots ACE");
    assert_eq!(without_core_clock("привет"), "привет");
}

/// One pulled line accounts for one live line: a message really written twice must stay twice.
#[test]
fn a_repeated_message_is_matched_one_for_one() {
    let pulled = [line("2026-08-03 10:00:00.000", "ACE (4411) order moved")];
    let live = vec![
        line("2026-08-03 10:00:00.100", "ACE (4411) order moved"),
        line("2026-08-03 10:00:00.900", "ACE (4411) order moved"),
    ];
    assert_eq!(only_in_live(live, &pulled).len(), 1);
}

/// Without a pulled day nothing is compared and nothing is dropped; a line without a full
/// timestamp cannot be matched and is kept.
#[test]
fn nothing_is_dropped_without_a_counterpart() {
    let live = vec![
        line("2026-08-03 10:00:00.100", "ACE (4411) buy placed"),
        line("2026-08-03", "ACE (4411) a malformed stored line"),
    ];
    assert_eq!(only_in_live(live.clone(), &[]).len(), 2);
    let pulled = [line(
        "2026-08-03 10:00:00.000",
        "ACE (4411) a malformed stored line",
    )];
    assert_eq!(only_in_live(live, &pulled).len(), 2);
}

/// `2026-08-03 00:00:00` UTC in unix milliseconds, from `date -u -d 2026-08-03 +%s` = 1785715200.
const AUG_3: i64 = 1_785_715_200_000;
const DAY: i64 = 86_400_000;

#[test]
fn a_trade_inside_one_day_scans_that_day() {
    let days = log_dates(AUG_3 + 3_600_000, AUG_3 + 7_200_000, 8);

    assert_eq!(days, vec!["2026-08-03".to_string()]);
}

#[test]
fn a_trade_across_midnight_scans_both_days() {
    let days = log_dates(AUG_3 - 3_600_000, AUG_3 + 3_600_000, 8);

    assert_eq!(
        days,
        vec!["2026-08-02".to_string(), "2026-08-03".to_string()]
    );
}

#[test]
fn an_open_trade_ending_before_it_began_still_scans_the_entry_day() {
    let days = log_dates(AUG_3 + 3_600_000, 0, 8);

    assert_eq!(days, vec!["2026-08-03".to_string()]);
}

#[test]
fn a_long_trade_keeps_both_ends_and_drops_the_middle() {
    let days = log_dates(AUG_3, AUG_3 + 40 * DAY, 8);

    assert_eq!(days.len(), 8, "не больше восьми файлов за один запрос");
    assert_eq!(days[0], "2026-08-03", "день входа на месте");
    assert_eq!(days[7], "2026-09-12", "день выхода на месте");
    assert_eq!(
        days[3], "2026-08-06",
        "четыре дня от входа, дальше разрыв к хвосту"
    );
    assert_eq!(days[4], "2026-09-09", "хвост считается назад от выхода");
}

#[test]
fn the_configured_name_is_tried_before_the_recorded_one() {
    let request = trade_log_request(
        "BinF3 старое",
        Some("BinF3"),
        "UAI",
        17062,
        AUG_3,
        AUG_3 + 60_000,
        AUG_3 + 120_000,
    );

    assert_eq!(
        request.labels,
        vec!["BinF3".to_string(), "BinF3_старое".to_string()],
        "переименованное ядро писало файлы под обоими именами"
    );
    assert_eq!(
        request.core_name, "BinF3 старое",
        "в заголовке — имя строки"
    );
    assert_eq!(request.task_id, 17062);
}

#[test]
fn one_name_yields_one_label() {
    let request = trade_log_request("BinF3", Some("BinF3"), "UAI", 1, AUG_3, AUG_3, AUG_3);

    assert_eq!(request.labels, vec!["BinF3".to_string()]);
}

#[test]
fn a_core_missing_from_the_config_falls_back_to_the_recorded_name() {
    let request = trade_log_request("Gone", None, "UAI", 1, AUG_3, AUG_3, AUG_3);

    assert_eq!(request.labels, vec!["Gone".to_string()]);
    assert_eq!(request.core_name, "Gone");
}

#[test]
fn an_open_trade_scans_up_to_now() {
    let request = trade_log_request(
        "BinF3",
        None,
        "UAI",
        1,
        AUG_3,
        0, // still open: the report has no close date
        AUG_3 + DAY,
    );

    assert_eq!(
        request.dates,
        vec!["2026-08-03".to_string(), "2026-08-04".to_string()],
        "открытая сделка сканируется по сегодняшний день"
    );
}
