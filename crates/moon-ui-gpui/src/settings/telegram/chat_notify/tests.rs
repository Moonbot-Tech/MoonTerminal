use super::{Commit, QuoteTexts, commit_plan, may_switch, parse_amount, settings_from};
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

/// No per-currency row typed.
const NO_QUOTES: (QuoteTexts, QuoteTexts) = (Vec::new(), Vec::new());

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
    match commit_plan(
        Some(&row),
        7,
        &NotifySettings::default(),
        fields,
        &NO_QUOTES,
        false,
    ) {
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
        commit_plan(Some(&row), 7, &draft, F, &NO_QUOTES, false),
        Commit::Nothing
    );
    assert_eq!(
        commit_plan(
            Some(&row),
            8,
            &draft,
            ["1", "", "", "10", "", ""],
            &NO_QUOTES,
            false
        ),
        Commit::Nothing
    );
    assert_eq!(
        commit_plan(None, 7, &draft, F, &NO_QUOTES, false),
        Commit::Nothing
    );
}

/// A bad number or a busy station refuses visibly; the edits are not dropped silently.
#[test]
fn a_bad_field_or_busy_station_refuses() {
    let row = stored(3);
    let draft = NotifySettings::default();
    assert!(matches!(
        commit_plan(
            Some(&row),
            7,
            &draft,
            ["abc", "", "", "10", "", ""],
            &NO_QUOTES,
            false
        ),
        Commit::Refuse(_)
    ));
    assert_eq!(
        commit_plan(
            Some(&row),
            7,
            &draft,
            ["99", "", "", "10", "", ""],
            &NO_QUOTES,
            true
        ),
        Commit::Refuse("telegram.mini_error_busy_station")
    );
}

/// A currency row typed into the editor is part of what a commit saves: left out, the row's
/// fields would autosave nothing, and a row added or removed would never reach the bot.
#[test]
fn a_typed_currency_row_is_saved() {
    use moon_core::db::QuoteCurrency;
    let row = stored(5);
    let rub = QuoteCurrency::from_ticker("RUB").unwrap();
    let quotes = (
        vec![(rub, vec![String::new(), "100".to_string(), String::new()])],
        Vec::new(),
    );
    match commit_plan(Some(&row), 7, &NotifySettings::default(), F, &quotes, false) {
        Commit::Save(settings, revision) => {
            assert_eq!(settings.trades.by_quote.len(), 1);
            assert_eq!(settings.trades.by_quote[0].quote, "RUB");
            assert_eq!(settings.trades.by_quote[0].profit_at_least, Some(100.0));
            assert_eq!(revision, 5);
        }
        other => panic!("expected a save, got {other:?}"),
    }
    assert_eq!(
        commit_plan(
            Some(&row),
            7,
            &NotifySettings::default(),
            F,
            &NO_QUOTES,
            false
        ),
        Commit::Nothing,
        "no row typed and none stored: nothing to save"
    );
}

/// An untouched editor over a stored list in another shape — a lowercase ticker, an unknown
/// currency before a known one — has nothing to save: saving it would bump the revision on every
/// Blur and turn a Mini App tab stale for nothing.
#[test]
fn an_untouched_editor_over_an_odd_stored_list_saves_nothing() {
    use moon_core::db::QuoteCurrency;
    use moon_core::telegram::notify::QuoteThresholds;
    let mut settings = settings_from(&NotifySettings::default(), F).unwrap();
    settings.trades.by_quote = vec![
        QuoteThresholds {
            quote: "KZT".to_string(),
            ..QuoteThresholds::default()
        },
        QuoteThresholds {
            quote: "rub".to_string(),
            profit_at_least: Some(100.0),
            ..QuoteThresholds::default()
        },
    ];
    let row = (
        7,
        ChatNotifyRow {
            settings: settings.clone(),
            revision: 9,
        },
    );
    let rub = QuoteCurrency::from_ticker("RUB").unwrap();
    let typed = (
        vec![(rub, vec![String::new(), "100".to_string(), String::new()])],
        Vec::new(),
    );
    assert_eq!(
        commit_plan(Some(&row), 7, &settings, F, &typed, false),
        Commit::Nothing
    );
    let edited = (
        vec![(rub, vec![String::new(), "150".to_string(), String::new()])],
        Vec::new(),
    );
    assert!(matches!(
        commit_plan(Some(&row), 7, &settings, F, &edited, false),
        Commit::Save(_, 9)
    ));
}

/// The picker leaves a chat only once its edits were taken.
#[test]
fn the_picker_stays_on_a_refused_or_sending_chat() {
    assert!(may_switch(false, false));
    assert!(!may_switch(true, false));
    assert!(!may_switch(false, true));
}

/// A stored row of a currency this build does not know has no row in the editor; a save must
/// keep it as stored, or the editor would silently delete what a newer build wrote. The typed
/// rows replace the known ones.
#[test]
fn a_save_keeps_a_row_the_editor_cannot_show() {
    use super::settings_with_quotes;
    use moon_core::db::QuoteCurrency;
    use moon_core::telegram::notify::QuoteThresholds;
    let row = |quote: &str| QuoteThresholds {
        quote: quote.to_string(),
        ..QuoteThresholds::default()
    };
    let mut draft = NotifySettings::default();
    draft.trades.by_quote = vec![row("RUB"), row("KZT")];
    let rub = QuoteCurrency::from_ticker("RUB").unwrap();
    let typed = (
        vec![(rub, vec![String::new(), "100".to_string(), String::new()])],
        Vec::new(),
    );
    let saved = settings_with_quotes(&draft, ["", "", "", "5", "", ""], &typed).unwrap();
    let mut rub_row = row("RUB");
    rub_row.profit_at_least = Some(100.0);
    assert_eq!(saved.trades.by_quote, vec![rub_row, row("KZT")]);
    assert!(saved.validate().is_ok());
}
