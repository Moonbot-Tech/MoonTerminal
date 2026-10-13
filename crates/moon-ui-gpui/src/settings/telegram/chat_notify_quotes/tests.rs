use super::{QuoteList, QuoteTexts, rows_from, stored_texts};
use moon_core::db::QuoteCurrency;
use moon_core::telegram::notify::{NotifySettings, QuoteThresholds};

fn rub() -> QuoteCurrency {
    QuoteCurrency::from_ticker("RUB").unwrap()
}

/// The fields of a trade row are volume, profit, loss in that order, and a chart row's are
/// profit, loss: reading them in another order would store a typed loss as a profit floor.
/// Empty is none, a comma is the decimal point, and a word names the error.
#[test]
fn typed_rows_become_the_stored_list() {
    let trades: QuoteTexts = vec![(rub(), vec!["5000".into(), "".into(), "1,5".into()])];
    assert_eq!(
        rows_from(QuoteList::Trades, &trades),
        Ok(vec![QuoteThresholds {
            quote: "RUB".to_string(),
            min_volume: Some(5000.0),
            profit_at_least: None,
            loss_at_least: Some(1.5),
        }])
    );
    let charts: QuoteTexts = vec![(rub(), vec!["100".into(), "".into()])];
    assert_eq!(
        rows_from(QuoteList::Charts, &charts),
        Ok(vec![QuoteThresholds {
            quote: "RUB".to_string(),
            min_volume: None,
            profit_at_least: Some(100.0),
            loss_at_least: None,
        }])
    );
    let bad: QuoteTexts = vec![(rub(), vec!["".into(), "abc".into(), "".into()])];
    assert_eq!(
        rows_from(QuoteList::Trades, &bad),
        Err("telegram.notify_editor.err_amount")
    );
}

/// What the editor fills from a stored row reads back as the same row, so an untouched editor
/// holds no edit; a row naming a currency this build does not know is not shown.
#[test]
fn stored_rows_fill_and_read_back_unchanged() {
    let mut settings = NotifySettings::default();
    settings.trades.by_quote = vec![
        QuoteThresholds {
            quote: "RUB".to_string(),
            min_volume: Some(5000.0),
            profit_at_least: Some(100.5),
            loss_at_least: None,
        },
        QuoteThresholds {
            quote: "KZT".to_string(),
            ..QuoteThresholds::default()
        },
    ];
    let texts = stored_texts(QuoteList::Trades, &settings);
    assert_eq!(texts.len(), 1);
    assert_eq!(
        rows_from(QuoteList::Trades, &texts),
        Ok(vec![settings.trades.by_quote[0].clone()])
    );
    assert!(stored_texts(QuoteList::Charts, &settings).is_empty());
}
