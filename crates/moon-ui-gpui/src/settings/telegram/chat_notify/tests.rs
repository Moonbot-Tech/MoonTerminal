use super::{parse_amount, parse_time, settings_from};
use moon_core::telegram::notify::{CoreScope, NotifySettings};

/// A threshold reads with a point or a comma; empty is none; a negative or a word is refused.
#[test]
fn amounts_read_as_typed() {
    assert_eq!(parse_amount(""), Ok(None));
    assert_eq!(parse_amount(" 12,5 "), Ok(Some(12.5)));
    assert_eq!(parse_amount("100"), Ok(Some(100.0)));
    assert!(parse_amount("-1").is_err());
    assert!(parse_amount("abc").is_err());
}

/// A time reads as HH:MM within the day.
#[test]
fn times_read_within_the_day() {
    assert_eq!(parse_time("21:00"), Ok((21, 0)));
    assert_eq!(parse_time(" 9:05 "), Ok((9, 5)));
    assert!(parse_time("24:00").is_err());
    assert!(parse_time("12:60").is_err());
    assert!(parse_time("1200").is_err());
}

/// The fields' numbers go into the draft; a bad field names itself; no core picked is refused.
#[test]
fn the_fields_complete_the_draft() {
    let draft = NotifySettings::default();
    let settings = settings_from(&draft, ["50", "", "1,5", "10", "08:30"]).unwrap();
    assert_eq!(settings.trades.min_volume_usd, Some(50.0));
    assert_eq!(settings.trades.profit_at_least_usd, None);
    assert_eq!(settings.trades.loss_at_least_usd, Some(1.5));
    assert_eq!(settings.down.after_minutes, 10);
    assert_eq!((settings.daily.hour, settings.daily.minute), (8, 30));
    assert!(settings.validate().is_ok());
    assert_eq!(
        settings_from(&draft, ["", "", "", "0", "08:30"]),
        Err("telegram.notify_editor.err_minutes")
    );
    assert_eq!(
        settings_from(&draft, ["", "", "", "5", "8"]),
        Err("telegram.notify_editor.err_time")
    );
    let mut none_picked = draft.clone();
    none_picked.trades.cores = CoreScope::Only(Vec::new());
    assert_eq!(
        settings_from(&none_picked, ["", "", "", "5", "08:00"]),
        Err("telegram.mini_settings_err_cores")
    );
}
