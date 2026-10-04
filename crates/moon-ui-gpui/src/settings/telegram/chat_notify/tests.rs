use super::{parse_amount, settings_from};
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

/// The fields' numbers go into the draft; a bad field names itself; no core picked is refused.
#[test]
fn the_fields_complete_the_draft() {
    let draft = NotifySettings::default();
    let settings = settings_from(&draft, ["50", "", "1,5", "10"]).unwrap();
    assert_eq!(settings.trades.min_volume_usd, Some(50.0));
    assert_eq!(settings.trades.profit_at_least_usd, None);
    assert_eq!(settings.trades.loss_at_least_usd, Some(1.5));
    assert_eq!(settings.down.after_minutes, 10);
    assert!(settings.validate().is_ok());
    assert_eq!(
        settings_from(&draft, ["", "", "", "0"]),
        Err("telegram.notify_editor.err_minutes")
    );
    let mut none_picked = draft.clone();
    none_picked.trades.cores = CoreScope::Only(Vec::new());
    assert_eq!(
        settings_from(&none_picked, ["", "", "", "5"]),
        Err("telegram.mini_settings_err_cores")
    );
}
