use super::{parse_amount, settings_from};
use moon_core::telegram::notify::{CoreScope, NotifySettings};

/// Reverting a shared checkbox caption to a period alone hides the delivery schedule from
/// both terminal and station settings. The oracle is the reviewed English UI wording.
#[test]
fn telegram_report_checkboxes_name_the_schedule() {
    let _locale = crate::test_locale::force("en");
    use moon_core::telegram::notify::AutoReport;
    for (kind, expected) in [
        (AutoReport::Hourly, "Each hour · separate"),
        (AutoReport::Today, "Today · hourly"),
        (AutoReport::Month, "Month · midnight"),
    ] {
        assert_eq!(super::auto_label(kind), expected);
    }
}

/// A threshold reads with a point or a comma; empty is none; a negative or a word is refused.
#[test]
fn amounts_read_as_typed() {
    assert_eq!(parse_amount(""), Ok(None));
    assert_eq!(parse_amount(" 12,5 "), Ok(Some(12.5)));
    assert_eq!(parse_amount("100"), Ok(Some(100.0)));
    assert!(parse_amount("-1").is_err());
    assert!(parse_amount("abc").is_err());
}

/// The fields' numbers go into the draft; a bad field names itself; no core picked is refused.
#[test]
fn the_fields_complete_the_draft() {
    let draft = NotifySettings::default();
    let settings = settings_from(&draft, ["50", "", "1,5", "10", "100", "12"]).unwrap();
    assert_eq!(settings.trades.min_volume_usd, Some(50.0));
    assert_eq!(settings.trades.profit_at_least_usd, None);
    assert_eq!(settings.trades.loss_at_least_usd, Some(1.5));
    assert_eq!(settings.down.after_minutes, 10);
    assert_eq!(settings.charts.profit_at_least_usd, Some(100.0));
    assert_eq!(settings.charts.loss_at_least_usd, Some(12.0));
    assert!(settings.validate().is_ok());
    assert_eq!(
        settings_from(&draft, ["", "", "", "0", "", ""]),
        Err("telegram.notify_editor.err_minutes")
    );
    assert_eq!(
        settings_from(&draft, ["", "", "", "5", "", "-1"]),
        Err("telegram.notify_editor.err_amount"),
        "a chart threshold is held to the card's rule"
    );
    let mut none_picked = draft.clone();
    none_picked.trades.cores = CoreScope::Only(Vec::new());
    assert_eq!(
        settings_from(&none_picked, ["", "", "", "5", "", ""]),
        Err("telegram.mini_settings_err_cores")
    );
}
