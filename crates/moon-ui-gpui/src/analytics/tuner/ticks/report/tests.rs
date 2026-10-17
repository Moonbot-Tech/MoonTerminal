//! Diagnostic identities must not collapse unlisted exchanges into an unknown marker.

use super::venue_name;

/// Losing the exchange key in diagnostics would make unrelated core venues indistinguishable.
#[test]
fn unlisted_report_uses_the_exchange_code() {
    assert_eq!(
        venue_name(None, moon_core::fixture::unlisted::EXCHANGE_KEY),
        "#200"
    );
    assert_eq!(venue_name(None, "201:00000001"), "#201");
    assert_eq!(venue_name(None, "broken:key"), "broken:key");
    assert_eq!(
        venue_name(moon_core::venue::venue(4), "4:00000000"),
        "Binance Futures"
    );
}
