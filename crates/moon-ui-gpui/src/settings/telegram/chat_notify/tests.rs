use super::{Commit, commit_plan, may_switch, parse_amount, settings_from};
use moon_core::station_api::ChatNotifyRow;
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

const F: [&str; 6] = ["50", "", "", "10", "", ""];

fn stored(revision: u64) -> (i64, ChatNotifyRow) {
    let settings = settings_from(&NotifySettings::default(), F).unwrap();
    (7, ChatNotifyRow { settings, revision })
}

/// J7: an edit applies on change, against the revision the row was read at, so a row changed
/// elsewhere meanwhile is refused by the bot instead of overwritten.
#[test]
fn a_change_saves_at_once_against_the_read_revision() {
    let row = stored(41);
    let fields = ["75", "", "", "10", "", ""];
    match commit_plan(Some(&row), 7, &NotifySettings::default(), fields, false) {
        Commit::Save(settings, revision) => {
            assert_eq!(settings.trades.min_volume_usd, Some(75.0));
            assert_eq!(revision, 41);
        }
        other => panic!("expected a save, got {other:?}"),
    }
}

/// Nothing is sent when the editor holds the stored row, or belongs to another chat.
#[test]
fn an_unchanged_or_foreign_editor_sends_nothing() {
    let row = stored(3);
    let draft = NotifySettings::default();
    assert_eq!(
        commit_plan(Some(&row), 7, &draft, F, false),
        Commit::Nothing
    );
    assert_eq!(
        commit_plan(Some(&row), 8, &draft, ["1", "", "", "10", "", ""], false),
        Commit::Nothing
    );
    assert_eq!(commit_plan(None, 7, &draft, F, false), Commit::Nothing);
}

/// A bad number or a busy station refuses visibly; the edits are not dropped silently.
#[test]
fn a_bad_field_or_busy_station_refuses() {
    let row = stored(3);
    let draft = NotifySettings::default();
    assert!(matches!(
        commit_plan(Some(&row), 7, &draft, ["abc", "", "", "10", "", ""], false),
        Commit::Refuse(_)
    ));
    assert_eq!(
        commit_plan(Some(&row), 7, &draft, ["99", "", "", "10", "", ""], true),
        Commit::Refuse("telegram.mini_error_busy_station")
    );
}

/// The picker leaves a chat only once its edits were taken.
#[test]
fn the_picker_stays_on_a_refused_or_sending_chat() {
    assert!(may_switch(false, false));
    assert!(!may_switch(true, false));
    assert!(!may_switch(false, true));
}
