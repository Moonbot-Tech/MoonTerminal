use super::{log_dates, trade_log_request};

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
