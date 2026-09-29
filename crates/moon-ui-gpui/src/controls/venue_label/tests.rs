use moon_core::feed::ExchangeId;
use moon_core::venue::CoreVenue;

use crate::controls::venue_label::{venue_id_label, venue_label, venue_section_label};

/// The terminal's captions come through its own dictionary, in the interface language.
///
/// Breakage: a wrapper that passed a translator other than this crate's `t!` (or none) would show
/// raw locale keys, or English in a Russian interface — visible on the unidentified caption, since
/// market kinds are untranslated industry terms in every language. The caption rules are tested
/// beside them, in `moon_core::venue::caption`.
#[test]
fn captions_are_translated_by_the_terminal_dictionary() {
    let _locale = crate::test_locale::force("ru");
    let quarterly = CoreVenue {
        id: ExchangeId::new(6),
        dex: String::new(),
        reported: "Binance Quarterly".to_string(),
    };
    assert_eq!(venue_label(&quarterly), "Binance Quarterly");
    assert_eq!(venue_id_label(ExchangeId::new(2)), "Bybit Futures");
    assert_eq!(venue_section_label(None), "Биржа не определена");
}
