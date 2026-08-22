use super::*;

#[test]
fn a_fresh_slot_shows_nothing() {
    let slot = PopupSlot::default();
    for popup in [
        ChartPopup::Layout,
        ChartPopup::Candle,
        ChartPopup::Graphics,
        ChartPopup::Labels,
        ChartPopup::FigStyle,
        ChartPopup::Coin,
    ] {
        assert!(!slot.shows(popup));
    }
}

#[test]
fn showing_one_displaces_the_other_and_names_it() {
    let mut slot = PopupSlot::default();
    assert_eq!(slot.show(ChartPopup::Labels), None);
    // The labels popup keeps outside-click dismissal off, so this displacement is the ONLY thing
    // that closes it when the candle button is pressed.
    assert_eq!(slot.show(ChartPopup::Candle), Some(ChartPopup::Labels));
    assert!(slot.shows(ChartPopup::Candle));
    assert!(!slot.shows(ChartPopup::Labels));
}

#[test]
fn showing_the_same_popup_again_displaces_nothing() {
    let mut slot = PopupSlot::default();
    slot.show(ChartPopup::Graphics);
    assert_eq!(slot.show(ChartPopup::Graphics), None);
    assert!(slot.shows(ChartPopup::Graphics));
}

#[test]
fn hiding_reports_ownership_and_only_clears_its_own() {
    let mut slot = PopupSlot::default();
    slot.show(ChartPopup::Layout);
    assert!(slot.hide(ChartPopup::Layout));
    assert!(!slot.shows(ChartPopup::Layout));
    // The second close report for one press must not run the ⚙ popup's commit again.
    assert!(!slot.hide(ChartPopup::Layout));
}

#[test]
fn a_stale_close_cannot_shut_the_popup_that_replaced_it() {
    let mut slot = PopupSlot::default();
    slot.show(ChartPopup::Layout);
    // Pressing the candle button: the press opens candles, and the ⚙ popover then reports its own
    // close. That late report must not take candles down with it.
    slot.show(ChartPopup::Candle);
    assert!(!slot.hide(ChartPopup::Layout));
    assert!(slot.shows(ChartPopup::Candle));
}
